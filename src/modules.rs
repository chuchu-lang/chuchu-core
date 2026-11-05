//! File discovery and module-scoped name resolution before typed lowering.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ast::{ModuleSyntax, StmtKind};
use crate::compiler::CompileError;
use crate::lexer::tokenize;
use crate::parser::Parser;
use crate::source::SourceFile;
use crate::token::Span;

#[derive(Debug)]
pub struct ModuleFile {
    pub source: SourceFile,
    pub offset: usize,
    pub syntax: ModuleSyntax,
    /// Import order matches `syntax.imports`.
    pub dependencies: Vec<usize>,
}

#[derive(Debug)]
pub struct ModuleGraph {
    /// The entry file is always index zero. IDs follow deterministic import order.
    pub files: Vec<ModuleFile>,
    pub(crate) lex_time: std::time::Duration,
    pub(crate) parse_time: std::time::Duration,
}

fn error(message: impl Into<String>, span: Span) -> CompileError {
    CompileError::Module {
        message: message.into(),
        span,
    }
}

impl ModuleGraph {
    pub fn load(entry: &Path) -> Result<Self, CompileError> {
        if entry
            .extension()
            .is_none_or(|extension| extension != "chuchu")
        {
            return Err(CompileError::UnsupportedExtension(entry.to_owned()));
        }
        Self::load_named(entry)
    }

    /// The named-source API historically accepts any entry filename. Imported
    /// dependencies still require `.chuchu`, just like the file-based CLI.
    pub(crate) fn load_named(entry: &Path) -> Result<Self, CompileError> {
        let mut loader = Loader::default();
        let result = loader.load(entry, None);
        match result {
            Ok(_) => Ok(Self {
                files: loader.files,
                lex_time: loader.lex_time,
                parse_time: loader.parse_time,
            }),
            Err(error) if !loader.files.is_empty() => Err(CompileError::Located {
                error: Box::new(error),
                source: Box::new(SourceFile::bundle(
                    loader.files.into_iter().map(|file| file.source).collect(),
                )),
            }),
            Err(error) => Err(error),
        }
    }

    /// Parse unnamed in-memory source without implicit filesystem access.
    pub fn parse_source(source: SourceFile) -> Result<Self, CompileError> {
        let start = std::time::Instant::now();
        let tokens = tokenize(source.text()).map_err(CompileError::Lex)?;
        let lex_time = start.elapsed();
        let start = std::time::Instant::now();
        let syntax = Parser::new(tokens)
            .parse_module()
            .map_err(CompileError::Parse)?;
        let parse_time = start.elapsed();
        if let Some(import) = syntax.imports.first() {
            return Err(error(
                "Relative imports require a source file; use compile_file or compile_named_source_with_options",
                import.span,
            ));
        }
        Ok(Self {
            files: vec![ModuleFile {
                source,
                offset: 0,
                syntax,
                dependencies: Vec::new(),
            }],
            lex_time,
            parse_time,
        })
    }

    pub fn sources(&self) -> SourceFile {
        SourceFile::bundle(self.files.iter().map(|file| file.source.clone()).collect())
    }
}

#[derive(Default)]
struct Loader {
    files: Vec<ModuleFile>,
    identities: HashMap<PathBuf, usize>,
    active: Vec<usize>,
    bytes: usize,
    lex_time: std::time::Duration,
    parse_time: std::time::Duration,
}

impl Loader {
    fn load(&mut self, path: &Path, import: Option<Span>) -> Result<usize, CompileError> {
        if import.is_some()
            && path
                .extension()
                .is_none_or(|extension| extension != "chuchu")
        {
            return Err(match import {
                Some(span) => error(
                    format!("Import must name a .chuchu file: {}", path.display()),
                    span,
                ),
                None => CompileError::UnsupportedExtension(path.to_owned()),
            });
        }
        let identity = path.canonicalize().map_err(|source| match import {
            Some(span) => error(
                format!("Cannot load imported module {}: {source}", path.display()),
                span,
            ),
            None => CompileError::SourceRead {
                path: path.to_owned(),
                source,
            },
        })?;
        if let Some(&id) = self.identities.get(&identity) {
            if let Some(start) = self.active.iter().position(|&active| active == id) {
                let cycle = self.active[start..]
                    .iter()
                    .copied()
                    .chain(std::iter::once(id))
                    .map(|id| self.files[id].source.path().display().to_string())
                    .collect::<Vec<_>>()
                    .join(" -> ");
                return Err(error(
                    format!("Circular import: {cycle}"),
                    import.unwrap_or(Span { start: 0, end: 0 }),
                ));
            }
            return Ok(id);
        }
        let text = std::fs::read_to_string(&identity).map_err(|source| match import {
            Some(span) => error(
                format!(
                    "Cannot read imported module {}: {source}",
                    identity.display()
                ),
                span,
            ),
            None => CompileError::SourceRead {
                path: identity.clone(),
                source,
            },
        })?;
        let offset = self.bytes;
        self.bytes += text.len() + 1;
        let id = self.files.len();
        self.identities.insert(identity.clone(), id);
        self.files.push(ModuleFile {
            source: SourceFile::new(identity.clone(), text).with_import_chain(
                self.active
                    .iter()
                    .map(|&id| self.files[id].source.path().to_owned())
                    .collect(),
            ),
            offset,
            syntax: ModuleSyntax::default(),
            dependencies: Vec::new(),
        });
        self.active.push(id);
        let start = std::time::Instant::now();
        let mut tokens = tokenize(self.files[id].source.text()).map_err(|mut error| {
            error.span.start += offset;
            error.span.end += offset;
            CompileError::Lex(error)
        })?;
        self.lex_time += start.elapsed();
        for token in &mut tokens {
            token.span.start += offset;
            token.span.end += offset;
        }
        let start = std::time::Instant::now();
        let syntax = Parser::new(tokens)
            .parse_module()
            .map_err(CompileError::Parse)?;
        self.parse_time += start.elapsed();
        if id != 0 {
            if syntax.name.is_none() {
                return Err(error(
                    "Imported files must start with 'module name'",
                    Span {
                        start: offset,
                        end: offset,
                    },
                ));
            }
            if let Some(statement) = syntax.statements.iter().find(|statement| {
                !matches!(
                    statement.kind,
                    StmtKind::Function { .. } | StmtKind::Record { .. }
                )
            }) {
                return Err(error(
                    "Imported modules may contain only functions, records, and imports; executable statements belong in the entry file",
                    statement.span,
                ));
            }
        }
        self.files[id].syntax = syntax;
        let imports = self.files[id].syntax.imports.clone();
        for import in imports {
            let path = Path::new(&import.path);
            let target = identity
                .parent()
                .expect("canonical source has a parent")
                .join(path);
            let dependency = self.load(&target, Some(import.span))?;
            self.files[id].dependencies.push(dependency);
        }
        self.active.pop();
        Ok(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolKind {
    Function,
    Record,
}

struct Symbol {
    kind: SymbolKind,
    exported: bool,
    lowered: String,
}

struct Scope {
    symbols: HashMap<String, Symbol>,
    imports: HashMap<String, usize>,
}

impl ModuleGraph {
    /// Resolve names into unique module-qualified identities. LLVM lowering and
    /// nominal record typing can then share the existing whole-program pipeline.
    pub fn resolve(&self) -> Result<Vec<crate::ast::Stmt>, CompileError> {
        let result = self.resolve_inner();
        result.map_err(|error| CompileError::Located {
            error: Box::new(error),
            source: Box::new(self.sources()),
        })
    }

    fn resolve_inner(&self) -> Result<Vec<crate::ast::Stmt>, CompileError> {
        if self.files.len() == 1 {
            let syntax = &self.files[0].syntax;
            if syntax.name.is_none() && syntax.imports.is_empty() && syntax.exports.is_empty() {
                return Ok(syntax.statements.clone());
            }
        }
        let mut scopes = Vec::new();
        for (id, file) in self.files.iter().enumerate() {
            let mut scope = Scope {
                symbols: HashMap::new(),
                imports: HashMap::new(),
            };
            for (import, &target) in file.syntax.imports.iter().zip(&file.dependencies) {
                if scope.imports.insert(import.alias.clone(), target).is_some() {
                    return Err(error(
                        format!("Duplicate module name {:?}", import.alias),
                        import.span,
                    ));
                }
                if crate::type_checker::reserved_function(&import.alias)
                    || builtin_type(&import.alias)
                {
                    return Err(error(
                        format!("Module name {:?} is a reserved name", import.alias),
                        import.span,
                    ));
                }
            }
            let mut declared = std::collections::HashSet::new();
            for statement in &file.syntax.statements {
                let (name, kind) = match &statement.kind {
                    StmtKind::Function { name, .. } => (name, Some(SymbolKind::Function)),
                    StmtKind::Record { name, .. } => (name, Some(SymbolKind::Record)),
                    StmtKind::Set { name, .. } => (name, None),
                    _ => continue,
                };
                if scope.imports.contains_key(name) || !declared.insert(name.clone()) {
                    return Err(error(
                        format!(
                            "Duplicate name {name:?} in module; declarations and used module names must be unique"
                        ),
                        statement.span,
                    ));
                }
                if kind == Some(SymbolKind::Function)
                    && crate::type_checker::reserved_function(name)
                {
                    return Err(CompileError::Type(
                        crate::type_checker::TypeError::InvalidFunction {
                            name: name.clone(),
                            message: "duplicate or reserved function name",
                            span: statement.span,
                        },
                    ));
                }
                if kind == Some(SymbolKind::Record) && builtin_type(name) {
                    return Err(CompileError::Type(
                        crate::type_checker::TypeError::ReservedRecordName {
                            name: name.clone(),
                            span: statement.span,
                        },
                    ));
                }
                if let Some(kind) = kind {
                    let label = file
                        .syntax
                        .name
                        .as_ref()
                        .map_or("entry", |(name, _)| name.as_str());
                    scope.symbols.insert(
                        name.clone(),
                        Symbol {
                            kind,
                            exported: file.syntax.exports.iter().any(|(export, _)| export == name),
                            // Brackets cannot occur in a source identifier. The file ID
                            // also distinguishes modules that use the same label.
                            lowered: format!("{label}[{id}].{name}"),
                        },
                    );
                }
            }
            scopes.push(scope);
        }
        let resolver = Resolver { scopes: &scopes };
        let mut statements = Vec::new();
        for (id, file) in self.files.iter().enumerate() {
            for original in &file.syntax.statements {
                let mut statement = original.clone();
                resolver.statement(&mut statement, id, true)?;
                statements.push(statement);
            }
        }
        Ok(statements)
    }
}

fn builtin_type(name: &str) -> bool {
    matches!(name, "int" | "float" | "bool" | "string" | "result")
}

struct Resolver<'a> {
    scopes: &'a [Scope],
}

impl Resolver<'_> {
    fn name(
        &self,
        name: &str,
        module: usize,
        kind: SymbolKind,
        span: Span,
        public: bool,
    ) -> Result<String, CompileError> {
        let (owner, member, imported) = if let Some((alias, member)) = name.split_once('.') {
            let owner = self.scopes[module].imports.get(alias).ok_or_else(|| {
                error(
                    format!(
                        "Unknown module name {alias:?}; add 'use module {alias}' before using {name:?}"
                    ),
                    span,
                )
            })?;
            (*owner, member, true)
        } else {
            (module, name, false)
        };
        let symbol = self.scopes[owner].symbols.get(member);
        let Some(symbol) = symbol else {
            // Existing built-ins and undefined unqualified calls retain the
            // established type-checker diagnostics and suggestions.
            if !imported && kind == SymbolKind::Function {
                return Ok(name.to_owned());
            }
            return Err(error(
                format!(
                    "Unknown {} {name:?}",
                    if kind == SymbolKind::Record {
                        "record type"
                    } else {
                        "function"
                    }
                ),
                span,
            ));
        };
        if imported && !symbol.exported {
            return Err(error(
                format!("{name:?} is private; add 'export' to its declaration to import it"),
                span,
            ));
        }
        if symbol.kind != kind {
            return Err(error(
                format!(
                    "{name:?} is not a {}",
                    if kind == SymbolKind::Record {
                        "record type"
                    } else {
                        "function"
                    }
                ),
                span,
            ));
        }
        if public && !symbol.exported {
            return Err(error(
                format!(
                    "Exported declaration exposes private record type {name:?}; export the record or keep the declaration private"
                ),
                span,
            ));
        }
        Ok(symbol.lowered.clone())
    }

    fn annotation(
        &self,
        ty: &mut crate::ast::FieldType,
        module: usize,
        span: Span,
        public: bool,
    ) -> Result<(), CompileError> {
        if let crate::ast::FieldType::Named(name) = ty {
            *name = self.name(name, module, SymbolKind::Record, span, public)?;
        }
        Ok(())
    }

    fn statement(
        &self,
        statement: &mut crate::ast::Stmt,
        module: usize,
        top_level: bool,
    ) -> Result<(), CompileError> {
        use crate::ast::StmtKind::*;
        let span = statement.span;
        match &mut statement.kind {
            Function {
                name,
                parameters,
                return_type,
                return_span,
                body,
            } => {
                if !top_level {
                    return Err(error("Functions must be declared at the top level", span));
                }
                let public = self.scopes[module].symbols[name].exported;
                *name = self.name(name, module, SymbolKind::Function, span, false)?;
                for parameter in parameters {
                    self.annotation(&mut parameter.ty, module, parameter.ty_span, public)?;
                }
                self.annotation(return_type, module, *return_span, public)?;
                for statement in body {
                    self.statement(statement, module, false)?;
                }
            }
            Record { name, fields } => {
                if !top_level {
                    return Err(error("Records must be declared at the top level", span));
                }
                let public = self.scopes[module].symbols[name].exported;
                *name = self.name(name, module, SymbolKind::Record, span, false)?;
                for field in fields {
                    self.annotation(&mut field.ty, module, field.span, public)?;
                }
            }
            Set { value, .. }
            | Assign { value, .. }
            | AssignField { value, .. }
            | Show(value)
            | Return(value) => self.expression(value, module)?,
            If {
                condition,
                then_body,
                else_body,
            } => {
                self.expression(condition, module)?;
                for statement in then_body.iter_mut().chain(else_body) {
                    self.statement(statement, module, false)?;
                }
            }
            While { condition, body } => {
                self.expression(condition, module)?;
                for statement in body {
                    self.statement(statement, module, false)?;
                }
            }
        }
        Ok(())
    }

    fn expression(&self, expr: &mut crate::ast::Expr, module: usize) -> Result<(), CompileError> {
        use crate::ast::ExprKind::*;
        match &mut expr.kind {
            Call { name, arguments } => {
                *name = self.name(name, module, SymbolKind::Function, expr.span, false)?;
                for argument in arguments {
                    self.expression(argument, module)?;
                }
            }
            RecordInit { name, fields } => {
                *name = self.name(name, module, SymbolKind::Record, expr.span, false)?;
                for field in fields {
                    self.expression(&mut field.value, module)?;
                }
            }
            Array(items) => {
                for item in items {
                    self.expression(item, module)?;
                }
            }
            Error { code, .. } | Negate(code) | FieldAccess { target: code, .. } => {
                self.expression(code, module)?
            }
            Index { array, index } => {
                self.expression(array, module)?;
                self.expression(index, module)?;
            }
            Binary { left, right, .. } => {
                self.expression(left, module)?;
                self.expression(right, module)?;
            }
            _ => {}
        }
        Ok(())
    }
}
