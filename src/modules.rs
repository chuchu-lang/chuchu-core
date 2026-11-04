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
}

fn error(message: impl Into<String>, span: Span) -> CompileError {
    CompileError::Module {
        message: message.into(),
        span,
    }
}

impl ModuleGraph {
    pub fn load(entry: &Path) -> Result<Self, CompileError> {
        let mut loader = Loader::default();
        let result = loader.load(entry, None);
        match result {
            Ok(_) => Ok(Self {
                files: loader.files,
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
}

impl Loader {
    fn load(&mut self, path: &Path, import: Option<Span>) -> Result<usize, CompileError> {
        if path
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
            source: SourceFile::new(identity.clone(), text),
            offset,
            syntax: ModuleSyntax::default(),
            dependencies: Vec::new(),
        });
        self.active.push(id);
        let mut tokens = tokenize(self.files[id].source.text()).map_err(|mut error| {
            error.span.start += offset;
            error.span.end += offset;
            CompileError::Lex(error)
        })?;
        for token in &mut tokens {
            token.span.start += offset;
            token.span.end += offset;
        }
        let syntax = Parser::new(tokens)
            .parse_module()
            .map_err(CompileError::Parse)?;
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
            if import.path.is_empty()
                || path.is_absolute()
                || import.path.contains(['\\', ':', '\0'])
            {
                return Err(error(
                    "Import paths must be nonempty relative paths using '/' separators",
                    import.span,
                ));
            }
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
