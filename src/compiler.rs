use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use inkwell::context::Context;

use crate::codegen::{CodegenError, generate_program_with_records};
use crate::lexer::{LexError, tokenize};
use crate::native::{NativeError, NativeTimings, build_executable_with_options};
use crate::optimization::BuildOptions;
use crate::parser::{ParseError, Parser};
use crate::symbol_table::SymbolTable;
use crate::type_checker::{TypeChecker, TypeError};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum CompileError {
    UnsupportedExtension(PathBuf),
    MissingDebugSource,
    SourceRead { path: PathBuf, source: io::Error },
    InputOutputConflict(PathBuf),
    ConflictingOutputs(PathBuf),
    Lex(LexError),
    Parse(ParseError),
    Type(TypeError),
    Codegen(CodegenError),
    Native(NativeError),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDebugSource => write!(
                f,
                "Debug compilation requires a source file; use compile_file_with_options or compile_named_source_with_options"
            ),
            Self::UnsupportedExtension(path) => {
                write!(f, "Expected a .chuchu source file: {}", path.display())
            }
            Self::SourceRead { path, source } => {
                write!(f, "Cannot read source {}: {source}", path.display())
            }
            Self::ConflictingOutputs(path) => {
                write!(f, "Conflicting artifact paths: {}", path.display())
            }
            Self::InputOutputConflict(path) => {
                write!(f, "Output would overwrite source {}", path.display())
            }
            Self::Lex(error) => {
                use crate::lexer::LexErrorKind;
                let message = match error.kind {
                    LexErrorKind::UnexpectedCharacter => {
                        format!("Invalid source character {:?}", error.character)
                    }
                    LexErrorKind::UnterminatedString => "Unterminated string literal".into(),
                    LexErrorKind::UnterminatedComment => "Unterminated block comment".into(),
                    LexErrorKind::InvalidEscape => format!(
                        "Invalid string escape \\{}",
                        error.character.escape_default()
                    ),
                    LexErrorKind::NulInString => {
                        "NUL characters are not supported in strings".into()
                    }
                };
                write!(f, "{message} at {}..{}", error.span.start, error.span.end)
            }
            Self::Parse(error) => {
                let (expected, span) = match error {
                    ParseError::UnexpectedEnd => {
                        return write!(f, "Unexpected end of token stream");
                    }
                    ParseError::InvalidAssignmentTarget { span } => {
                        return write!(
                            f,
                            "Invalid assignment target at {}..{}",
                            span.start, span.end
                        );
                    }
                    ParseError::ExpectedExpression { span } => ("expression", span),
                    ParseError::ExpectedStatement { span } => ("statement", span),
                    ParseError::ExpectedIdentifier { span } => ("identifier", span),
                    ParseError::ExpectedStatementSeparator { span } => {
                        ("newline between statements", span)
                    }
                    ParseError::ExpectedToken { expected, span } => (*expected, span),
                };
                write!(f, "Expected {expected} at {}..{}", span.start, span.end)
            }
            Self::Type(error) => match error {
                TypeError::UndefinedVariable { name, span } => write!(
                    f,
                    "Undefined variable {name:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::DuplicateDeclaration { name, span } => write!(
                    f,
                    "Duplicate declaration of {name:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::InvalidOperands { left, right, span } => write!(
                    f,
                    "Unsupported operand types {left:?} and {right:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::Mismatch {
                    expected,
                    actual,
                    span,
                } => write!(
                    f,
                    "Type mismatch: expected {expected:?}, found {actual:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::InvalidCondition { span } => {
                    write!(f, "Condition must be Bool at {}..{}", span.start, span.end)
                }
                TypeError::InvalidFunction {
                    name,
                    message,
                    span,
                } => write!(
                    f,
                    "Function {name:?}: {message} at {}..{}",
                    span.start, span.end
                ),
                TypeError::InvalidArray { message, span } => write!(
                    f,
                    "Invalid array: {message} at {}..{}",
                    span.start, span.end
                ),
                TypeError::InvalidLiteral { literal, span } => write!(
                    f,
                    "Invalid or non-finite numeric literal {literal:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::InvalidReturn { span } => write!(
                    f,
                    "Return is only valid inside a function at {}..{}",
                    span.start, span.end
                ),
                TypeError::MissingReturn { name, span } => write!(
                    f,
                    "Function {name:?} must return a value on all paths at {}..{}",
                    span.start, span.end
                ),
                TypeError::UnsupportedRecordUse { feature, span } => write!(
                    f,
                    "Records do not support {feature} yet at {}..{}",
                    span.start, span.end
                ),
                TypeError::DuplicateRecord { name, span } => write!(
                    f,
                    "Duplicate record declaration {name:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::DuplicateField {
                    record,
                    field,
                    span,
                } => write!(
                    f,
                    "Duplicate field {field:?} in record {record:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::RecordNotTopLevel { name, span } => write!(
                    f,
                    "Record {name:?} must be declared at the top level at {}..{}",
                    span.start, span.end
                ),
                TypeError::ReservedRecordName { name, span } => write!(
                    f,
                    "Record name {name:?} is a built-in type name at {}..{}",
                    span.start, span.end
                ),
                TypeError::UndefinedRecordType {
                    record,
                    type_name,
                    span,
                } => write!(
                    f,
                    "Undefined type {type_name:?} in record {record:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::RecursiveRecord { path, span } => write!(
                    f,
                    "Recursive record definition {} at {}..{}",
                    path.join(" -> "),
                    span.start,
                    span.end
                ),
                TypeError::UndefinedRecord { name, span } => write!(
                    f,
                    "Undefined record {name:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::UndefinedField {
                    record,
                    field,
                    span,
                } => write!(
                    f,
                    "Record {record:?} has no field {field:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::UnknownField {
                    record,
                    field,
                    span,
                } => write!(
                    f,
                    "Record {record:?} has no field {field:?} at {}..{}",
                    span.start, span.end
                ),
                TypeError::FieldAccessOnNonRecord {
                    field,
                    actual,
                    span,
                } => write!(
                    f,
                    "Cannot read field {field:?} of a value of type {actual} at {}..{}",
                    span.start, span.end
                ),
                TypeError::FieldAssignOnNonRecord {
                    field,
                    actual,
                    span,
                } => write!(
                    f,
                    "Cannot assign field {field:?} of a value of type {actual} at {}..{}",
                    span.start, span.end
                ),
                TypeError::DuplicateInitializer {
                    record,
                    field,
                    span,
                } => write!(
                    f,
                    "Field {field:?} of record {record:?} is set more than once at {}..{}",
                    span.start, span.end
                ),
                TypeError::MissingField {
                    record,
                    field,
                    span,
                } => write!(
                    f,
                    "Missing field {field:?} in {record:?} instantiation at {}..{}",
                    span.start, span.end
                ),
                TypeError::MissingFields {
                    record,
                    fields,
                    span,
                } => write!(
                    f,
                    "Record {record:?} is missing {} at {}..{}",
                    fields
                        .iter()
                        .map(|field| format!("{field:?}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    span.start,
                    span.end
                ),
                TypeError::FieldMismatch { detail, span } => write!(
                    f,
                    "Field {:?} of record {:?} expects {}, found {} at {}..{}",
                    detail.field,
                    detail.record,
                    detail.expected,
                    detail.actual,
                    span.start,
                    span.end
                ),
                TypeError::RecordMismatch {
                    expected,
                    actual,
                    span,
                } => write!(
                    f,
                    "Type mismatch: expected {expected}, found {actual} at {}..{}",
                    span.start, span.end
                ),
                TypeError::NotARecord { ty, span } => write!(
                    f,
                    "Type {ty:?} is not a record at {}..{}",
                    span.start, span.end
                ),
                TypeError::InvalidRecord {
                    name,
                    message,
                    span,
                } => write!(
                    f,
                    "Invalid record {name:?}: {message} at {}..{}",
                    span.start, span.end
                ),
            },
            Self::Codegen(error) => error.fmt(f),
            Self::Native(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CompileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SourceRead { source, .. } => Some(source),
            Self::Codegen(error) => Some(error),
            Self::Native(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct CompilationTimings {
    pub lex: Duration,
    pub parse: Duration,
    pub type_check: Duration,
    pub llvm_generation: Duration,
    pub native: NativeTimings,
    pub total: Duration,
}
impl CompilationTimings {
    /// Stable machine-readable seconds; only emitted by the CLI on explicit request.
    pub fn json(&self) -> String {
        format!(
            "{{\"lex_s\":{},\"parse_s\":{},\"type_s\":{},\"ir_s\":{},\"target_setup_s\":{},\"optimization_s\":{},\"object_s\":{},\"link_s\":{},\"total_s\":{}}}",
            self.lex.as_secs_f64(),
            self.parse.as_secs_f64(),
            self.type_check.as_secs_f64(),
            self.llvm_generation.as_secs_f64(),
            self.native.target_setup.as_secs_f64(),
            self.native.optimization.as_secs_f64(),
            self.native.object.as_secs_f64(),
            self.native.link.as_secs_f64(),
            self.total.as_secs_f64()
        )
    }
}

fn identity(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    absolute.canonicalize().unwrap_or_else(|_| {
        absolute
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .and_then(|parent| absolute.file_name().map(|name| parent.join(name)))
            .unwrap_or_else(|| absolute.clone())
    })
}
fn validate_artifacts(
    output: &Path,
    options: &BuildOptions,
    input: Option<&Path>,
) -> Result<(), CompileError> {
    let sidecar = (options.debug_info == crate::optimization::DebugInfo::Full)
        .then(|| crate::native::debug_symbol_path(output))
        .flatten();
    let mut paths = vec![output];
    paths.extend(sidecar.as_deref());
    paths.extend(options.emit_ir.as_deref());
    paths.extend(options.emit_assembly.as_deref());
    let mut seen = Vec::new();
    for path in paths {
        let normalized = identity(path);
        if input.is_some_and(|input| {
            let input = identity(input);
            input == normalized || input.starts_with(&normalized)
        }) {
            return Err(CompileError::InputOutputConflict(path.to_owned()));
        }
        if seen.iter().any(|previous: &PathBuf| {
            previous == &normalized
                || previous.starts_with(&normalized)
                || normalized.starts_with(previous)
        }) {
            return Err(CompileError::ConflictingOutputs(path.to_owned()));
        }
        seen.push(normalized);
    }
    Ok(())
}

/// Validates the entire source before constructing LLVM IR or writing artifacts.
pub fn compile_source(source: &str, output: &Path) -> Result<(), CompileError> {
    compile_source_with_options(source, output, &BuildOptions::default()).map(|_| ())
}

pub fn compile_source_with_options(
    source: &str,
    output: &Path,
    options: &BuildOptions,
) -> Result<CompilationTimings, CompileError> {
    compile_input(source, None, output, options)
}

/// Compile source with a real, stable debugger-visible file. The file must contain
/// exactly this source, so the debugger never displays unrelated source text.
pub fn compile_named_source_with_options(
    source: &crate::source::SourceFile,
    output: &Path,
    options: &BuildOptions,
) -> Result<CompilationTimings, CompileError> {
    let path = identity(source.path());
    let disk = std::fs::read_to_string(&path).map_err(|source| CompileError::SourceRead {
        path: path.clone(),
        source,
    })?;
    if disk != source.text() {
        return Err(CompileError::SourceRead {
            path,
            source: io::Error::new(
                io::ErrorKind::InvalidInput,
                "Named debug source differs from file contents",
            ),
        });
    }
    let source = crate::source::SourceFile::new(path, disk);
    compile_input(source.text(), Some(&source), output, options)
}

fn compile_input(
    source: &str,
    named: Option<&crate::source::SourceFile>,
    output: &Path,
    options: &BuildOptions,
) -> Result<CompilationTimings, CompileError> {
    let total = Instant::now();
    if options.debug_info == crate::optimization::DebugInfo::Full && named.is_none() {
        return Err(CompileError::MissingDebugSource);
    }
    validate_artifacts(output, options, named.map(|s| s.path()))?;
    let start = Instant::now();
    let tokens = tokenize(source).map_err(CompileError::Lex)?;
    let lex = start.elapsed();
    let start = Instant::now();
    let statements = Parser::new(tokens)
        .parse_program()
        .map_err(CompileError::Parse)?;
    let parse = start.elapsed();
    let start = Instant::now();
    let mut checker = TypeChecker::new(SymbolTable::new());
    checker
        .check_program(&statements)
        .map_err(CompileError::Type)?;
    let records = checker.record_table().unwrap_or_default();
    let type_check = start.elapsed();
    let start = Instant::now();
    let context = Context::create();
    let module = generate_program_with_records(&context, &statements, &records, named, options)
        .map_err(CompileError::Codegen)?;
    let llvm_generation = start.elapsed();
    let native =
        build_executable_with_options(&module, output, options).map_err(CompileError::Native)?;
    #[cfg(target_os = "windows")]
    {
        std::mem::forget(module);
        std::mem::forget(context);
    }
    Ok(CompilationTimings {
        lex,
        parse,
        type_check,
        llvm_generation,
        native,
        total: total.elapsed(),
    })
}

pub fn compile_file(input: &Path, output: &Path) -> Result<(), CompileError> {
    compile_file_with_options(input, output, &BuildOptions::default()).map(|_| ())
}

pub fn compile_file_with_options(
    input: &Path,
    output: &Path,
    options: &BuildOptions,
) -> Result<CompilationTimings, CompileError> {
    let total = Instant::now();
    if input
        .extension()
        .is_none_or(|extension| extension != "chuchu")
    {
        return Err(CompileError::UnsupportedExtension(input.to_owned()));
    }
    let source = std::fs::read_to_string(input).map_err(|source| CompileError::SourceRead {
        path: input.to_owned(),
        source,
    })?;
    validate_artifacts(output, options, Some(input))?;
    let named = crate::source::SourceFile::new(identity(input), source);
    let mut timing = compile_input(named.text(), Some(&named), output, options)?;
    timing.total = total.elapsed();
    Ok(timing)
}
