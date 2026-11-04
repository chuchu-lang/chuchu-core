//! Human-facing source diagnostics. Structured compiler errors retain byte spans.
use std::fmt::Write;
use std::path::Path;

use crate::codegen::CodegenError;
use crate::compiler::CompileError;
use crate::lexer::tokenize;
use crate::parser::ParseError;
use crate::source::SourceFile;
use crate::token::{Span, Token, TokenKind};
use crate::type_checker::TypeError;
use crate::types::Type;

struct Diagnostic {
    span: Span,
    message: String,
    label: String,
    help: Option<String>,
}

/// Render errors with a location, source excerpt and repair guidance. Errors
/// without a source location (filesystem, linker, etc.) keep their normal text.
pub fn render(error: &CompileError, path: &Path, source: &str) -> String {
    if let CompileError::Located { error, source } = error {
        let Some(mut diagnostic) = describe(error, source.text()) else {
            return format!("{}: {error}", source.path().display());
        };
        let Some((file, local)) = source.origin(diagnostic.span.start) else {
            return format!("{}: {error}", source.path().display());
        };
        let width = diagnostic.span.end.saturating_sub(diagnostic.span.start);
        diagnostic.span = Span {
            start: local,
            end: (local + width).min(file.text().len()),
        };
        let mut rendered = render_diagnostic(diagnostic, file.path(), file.text());
        for importer in file.import_chain().iter().rev() {
            write!(
                rendered,
                "\n  = imported from {}",
                display_text(&importer.display().to_string())
            )
            .unwrap();
        }
        return rendered;
    }
    let Some(diagnostic) = describe(error, source) else {
        return format!("{}: {error}", path.display());
    };
    render_diagnostic(diagnostic, path, source)
}

fn render_diagnostic(diagnostic: Diagnostic, path: &Path, source: &str) -> String {
    let file = SourceFile::new(path.to_owned(), source.to_owned());
    let span = diagnostic.span;
    let Some(position) = file.position(span.start) else {
        return format!("{}: {}", path.display(), diagnostic.message);
    };
    let start = source[..span.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let end = source[start..]
        .find('\n')
        .map_or(source.len(), |offset| start + offset);
    let line = source[start..end]
        .strip_suffix('\r')
        .unwrap_or(&source[start..end]);
    let caret_byte = (span.start - start).min(line.len());
    let prefix = display_text(&line[..caret_byte]);
    let column = prefix.chars().count() + 1;
    let highlight_end = span.end.max(span.start).min(start + line.len());
    let highlight = source.get(span.start..highlight_end).unwrap_or("");
    let width = display_text(&format!("{}{highlight}", &line[..caret_byte]))
        .chars()
        .count()
        .saturating_sub(prefix.chars().count())
        .max(1);
    let gutter = position.line.to_string().len();
    let mut text = format!(
        "error: {}\n{:gutter$} --> {}:{}:{}\n{:gutter$} |\n",
        diagnostic.message,
        "",
        path.display(),
        position.line,
        column,
        ""
    );
    // Include the previous line when the error is at EOF or on a later line.
    if start > 0 {
        let previous_end = start - 1;
        let previous_start = source[..previous_end]
            .rfind('\n')
            .map_or(0, |offset| offset + 1);
        let previous = source[previous_start..previous_end]
            .strip_suffix('\r')
            .unwrap_or(&source[previous_start..previous_end]);
        writeln!(
            text,
            "{:>gutter$} | {}",
            position.line - 1,
            display_text(previous)
        )
        .unwrap();
    }
    writeln!(text, "{:>gutter$} | {}", position.line, display_text(line)).unwrap();
    write!(
        text,
        "{:gutter$} | {}{} {}",
        "",
        " ".repeat(column - 1),
        "^".repeat(width),
        diagnostic.label
    )
    .unwrap();
    if let Some(help) = diagnostic.help {
        write!(text, "\n{:gutter$} = help: {help}", "").unwrap();
    }
    text
}

// Expand tabs consistently in both the excerpt and caret, and escape terminal
// control characters instead of letting malformed source control the terminal.
fn display_text(text: &str) -> String {
    let mut output = String::new();
    let mut column = 0;
    for character in text.chars() {
        if character == '\t' {
            let count = 4 - column % 4;
            output.push_str(&" ".repeat(count));
            column += count;
        } else if character.is_control() {
            let escaped = character.escape_default().to_string();
            column += escaped.chars().count();
            output.push_str(&escaped);
        } else {
            output.push(character);
            column += 1;
        }
    }
    output
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Int => "int".into(),
        Type::Float => "float".into(),
        Type::Bool => "bool".into(),
        Type::String => "string".into(),
        Type::Result(payload) => format!("result<{}>", type_name(&payload.ty())),
        Type::Array { element, len } => format!("{}[{len}]", type_name(&element.scalar())),
        Type::Record(_) => "record".into(),
    }
}

fn describe(error: &CompileError, source: &str) -> Option<Diagnostic> {
    let (span, message, label, help) = match error {
        CompileError::Module { message, span } => (*span, message.clone(), "module resolution failed".into(), None),
        CompileError::Lex(error) => {
            use crate::lexer::LexErrorKind;
            let (message, label, help) = match error.kind {
                LexErrorKind::UnexpectedCharacter => (
                    format!("Invalid source character {:?}", error.character),
                    "this character is not supported here",
                    "Remove this character or replace it with a supported Chuchu token.",
                ),
                LexErrorKind::UnterminatedString => (
                    "Unterminated string literal".into(),
                    "missing closing double quote",
                    r#"Close the string with a double quote. Use \n for a newline inside a string."#,
                ),
                LexErrorKind::UnterminatedComment => (
                    "Unterminated block comment".into(),
                    "missing closing */",
                    "Close the block comment with */.",
                ),
                LexErrorKind::InvalidEscape => (
                    format!(
                        "Invalid string escape \\{}",
                        error.character.escape_default()
                    ),
                    "unsupported escape sequence",
                    r#"Supported escapes are \n, \r, \t, \\, and \"."#,
                ),
                LexErrorKind::NulInString => (
                    "NUL characters are not supported in strings".into(),
                    "a string cannot contain a NUL byte",
                    "Remove the NUL character or use printable text instead.",
                ),
            };
            (error.span, message, label.into(), Some(help.into()))
        }
        CompileError::Parse(error) => return Some(parse_diagnostic(error, source)),
        CompileError::Codegen(CodegenError::InvalidIntegerLiteral { literal, span }) => (
            *span,
            format!("Invalid or out-of-range signed 64-bit integer literal '{literal}'"),
            "this value does not fit in an int".into(),
            Some("Use an integer between -9223372036854775808 and 9223372036854775807.".into()),
        ),
        CompileError::Codegen(CodegenError::InvalidFloatLiteral { literal, span }) => (
            *span,
            format!("Invalid or non-finite float literal '{literal}'"),
            "this value does not fit in a finite float".into(),
            None,
        ),
        CompileError::Type(error) => match error {
            TypeError::UndefinedVariable { name, span } => (
                *span,
                format!("Undefined variable '{name}'"),
                "not declared in this scope".into(),
                Some(format!(
                    "Declare '{name}' with 'assign {name} as <value>' before using it, or check its spelling."
                )),
            ),
            TypeError::DuplicateDeclaration { name, span } => (
                *span,
                format!("Duplicate declaration of '{name}'"),
                "already declared in this scope".into(),
                Some(format!(
                    "Use 'reassign {name} as <value>' to update the existing variable, or choose a different name."
                )),
            ),
            TypeError::InvalidOperands { left, right, span } => (
                *span,
                format!(
                    "Unsupported operand types {} and {}",
                    type_name(left),
                    type_name(right)
                ),
                "these values cannot be combined with this operator".into(),
                None,
            ),
            TypeError::Mismatch {
                expected,
                actual,
                span,
            } => (
                *span,
                format!(
                    "Type mismatch: expected {}, found {}",
                    type_name(expected),
                    type_name(actual)
                ),
                format!("this expression has type {}", type_name(actual)),
                None,
            ),
            TypeError::InvalidCondition { span } => (
                *span,
                "Condition must be bool".into(),
                "expected true, false, or a comparison".into(),
                Some("Use a comparison such as 'counter < 5' as the condition.".into()),
            ),
            TypeError::InvalidFunction { name, message, span }
                if name == "show" && message.starts_with("unwrap a successful result") => (
                *span,
                "Cannot print a result directly".into(),
                "this is a result, not its contained value".into(),
                Some("Check is_ok(value), then use show unwrap(value); on failure use show error_message(value).".into()),
            ),
            TypeError::InvalidFunction {
                name,
                message,
                span,
            } if name == "len" && *message == "undefined function" => (
                *span,
                format!("Function '{name}': {message}"),
                "invalid function use or declaration".into(),
                Some("`len` was renamed to `length`. Did you mean `length`?".into()),
            ),
            TypeError::InvalidFunction {
                name,
                message,
                span,
            } => (
                *span,
                format!("Function '{name}': {message}"),
                "invalid function use or declaration".into(),
                None,
            ),
            TypeError::InvalidArray { message, span } => (
                *span,
                format!("Invalid array: {message}"),
                "invalid array expression".into(),
                None,
            ),
            TypeError::InvalidLiteral { literal, span } => (
                *span,
                format!("Invalid or out-of-range numeric literal '{literal}'"),
                "not a representable finite number".into(),
                None,
            ),
            TypeError::InvalidReturn { span } => (
                *span,
                "Return is only valid inside a function".into(),
                "return used outside a function".into(),
                None,
            ),
            TypeError::MissingReturn { name, span } => (
                *span,
                format!("Function '{name}' must return a value on all paths"),
                "a path through this function has no return value".into(),
                Some("Add a return statement for every possible path through the function.".into()),
            ),
            TypeError::UnsupportedRecordUse { feature, span } => (
                *span,
                format!("Records do not support {feature} yet"),
                "this operation on a record cannot be compiled yet".into(),
                Some("Print the fields of the record instead, such as 'show student.age'.".into()),
            ),
            TypeError::DuplicateRecord { name, span } => (
                *span,
                format!("Record '{name}' is already declared"),
                "duplicate record declaration".into(),
                Some("Rename this record or remove the earlier declaration.".into()),
            ),
            TypeError::DuplicateField {
                record,
                field,
                span,
            } => (
                *span,
                format!("Duplicate field '{field}' in record '{record}'"),
                "this field name is already used in the record".into(),
                Some("Give each field in a record a different name.".into()),
            ),
            TypeError::RecordNotTopLevel { name, span } => (
                *span,
                format!("Record '{name}' must be declared at the top level"),
                "records cannot be declared inside functions or blocks".into(),
                Some("Move the record declaration outside every function, if, and while block.".into()),
            ),
            TypeError::ReservedRecordName { name, span } => (
                *span,
                format!("'{name}' is a built-in type name and cannot name a record"),
                "reserved type name".into(),
                Some("Choose a different name, for example a capitalized one.".into()),
            ),
            TypeError::UndefinedRecordType {
                record,
                type_name,
                span,
            } => (
                *span,
                format!("Unknown type '{type_name}' in record '{record}'"),
                "no record with this name is declared".into(),
                Some(format!(
                    "Declare 'record {type_name}' at the top level, or use int, float, bool, or string."
                )),
            ),
            TypeError::RecursiveRecord { path, span } => (
                *span,
                format!("Record '{}' contains itself: {}", path[0], path.join(" -> ")),
                "this field makes the record infinitely large".into(),
                Some("Records are stored by value, so a record cannot contain itself, even through other records. Break the cycle.".into()),
            ),
            TypeError::UndefinedRecord { name, span } => (
                *span,
                format!("Unknown record '{name}'"),
                "no record with this name is declared".into(),
                Some(format!("Declare 'record {name}' at the top level before using it.")),
            ),
            TypeError::UnknownField { record, field, span } => (
                *span,
                format!("Record '{record}' has no field '{field}'"),
                "this field is not declared in the record".into(),
                Some(format!("Use one of the fields declared in 'record {record}'.")),
            ),
            TypeError::FieldAccessOnNonRecord { field, actual, span } => (
                *span,
                format!("Cannot read field '{field}' of a value of type {actual}"),
                format!("this has type {actual}, not a record"),
                Some("Only record values have fields; check the name before the '.'.".into()),
            ),
            TypeError::FieldAssignOnNonRecord { field, actual, span } => (
                *span,
                format!("Cannot assign field '{field}' of a value of type {actual}"),
                format!("this has type {actual}, not a record"),
                Some("Only record values have fields; check the name before the '.'.".into()),
            ),
            TypeError::DuplicateInitializer { record, field, span } => (
                *span,
                format!("Field '{field}' of record '{record}' is set more than once"),
                "this field was already given a value".into(),
                Some("Give each field exactly one value.".into()),
            ),
            TypeError::MissingFields { record, fields, span } => (
                *span,
                format!(
                    "Record '{record}' is missing {}",
                    if let [only] = fields.as_slice() {
                        format!("field '{only}'")
                    } else {
                        format!(
                            "fields {}",
                            fields.iter().map(|f| format!("'{f}'")).collect::<Vec<_>>().join(", ")
                        )
                    }
                ),
                "every field must be given a value".into(),
                Some(format!("Add a line such as '{}: <value>' for each missing field.", fields[0])),
            ),
            TypeError::FieldMismatch { detail, span } => (
                *span,
                format!(
                    "Field '{}' of record '{}' expects {}, found {}",
                    detail.field, detail.record, detail.expected, detail.actual
                ),
                format!("this expression has type {}", detail.actual),
                None,
            ),
            TypeError::RecordMismatch { expected, actual, span } => (
                *span,
                format!("Type mismatch: expected {expected}, found {actual}"),
                format!("this expression has type {actual}"),
                None,
            ),
            TypeError::UndefinedField { record, field, span } => (
                *span,
                format!("Record '{record}' has no field '{field}'"),
                "field not found in record definition".into(),
                Some(format!("Check the definition of 'record {record}' for available fields.")),
            ),
            TypeError::MissingField { record, field, span } => (
                *span,
                format!("Missing required field '{field}' in '{record}' instantiation"),
                format!("field '{field}' must be specified"),
                Some(format!("Provide '{field}: <value>' in the record instantiation.")),
            ),
            TypeError::NotARecord { ty, span } => (
                *span,
                format!("Type '{}' is not a record", type_name(ty)),
                "field access requires a record value".into(),
                None,
            ),
            TypeError::InvalidRecord { name, message, span } => (
                *span,
                format!("Invalid record '{name}': {message}"),
                message.to_string(),
                None,
            ),
        },
        _ => return None,
    };
    Some(Diagnostic {
        span,
        message,
        label,
        help,
    })
}

fn parse_diagnostic(error: &ParseError, source: &str) -> Diagnostic {
    let span = match error {
        ParseError::ExpectedExpression { span }
        | ParseError::ExpectedStatement { span }
        | ParseError::ExpectedIdentifier { span }
        | ParseError::ExpectedStatementSeparator { span }
        | ParseError::InvalidAssignmentTarget { span }
        | ParseError::ExpectedToken { span, .. } => *span,
        ParseError::UnexpectedEnd => Span {
            start: source.len(),
            end: source.len(),
        },
    };
    let tokens = tokenize(source).unwrap_or_default();
    let previous = tokens.iter().rev().find(|token| {
        token.span.end <= span.start && !matches!(token.kind, TokenKind::Newline | TokenKind::Eof)
    });
    let found = if span.start >= source.len() {
        "end of file".into()
    } else if source
        .get(span.start..)
        .is_some_and(|rest| rest.starts_with(['\r', '\n']))
    {
        "end of line".into()
    } else {
        format!("{:?}", source.get(span.start..span.end).unwrap_or("token"))
    };
    let (message, label, help) = match error {
        ParseError::ExpectedExpression { .. } => {
            let operator = previous
                .filter(|token| {
                    matches!(
                        token.kind,
                        TokenKind::Plus
                            | TokenKind::Minus
                            | TokenKind::Star
                            | TokenKind::Slash
                            | TokenKind::Equal
                            | TokenKind::NotEqual
                            | TokenKind::Less
                            | TokenKind::LessEqual
                            | TokenKind::Greater
                            | TokenKind::GreaterEqual
                    )
                })
                .and_then(|token| source.get(token.span.start..token.span.end));
            if let Some(operator) = operator {
                (
                    format!("Expected expression after '{operator}'"),
                    "missing operand".into(),
                    Some(format!(
                        "Add a number, variable, or expression after '{operator}'."
                    )),
                )
            } else {
                (
                    format!("Expected expression, found {found}"),
                    "a value is required here".into(),
                    Some(
                        "Use a number, string, boolean, variable, array, or function call.".into(),
                    ),
                )
            }
        }
        ParseError::ExpectedIdentifier { .. } => (
            format!("Expected identifier, found {found}"),
            "expected a variable or parameter name".into(),
            Some("Names must begin with a letter or underscore, for example 'counter'.".into()),
        ),
        ParseError::InvalidAssignmentTarget { .. } => (
            "Invalid assignment target".into(),
            "only a variable, or a field of a variable, can be reassigned".into(),
            Some("Assign to a variable or its field, such as 'reassign student.age as 25'.".into()),
        ),
        ParseError::ExpectedStatement { .. } => (
            format!("Expected statement, found {found}"),
            "not a valid start of a statement".into(),
            Some("Start with assign, reassign, show, if, while, function, or return.".into()),
        ),
        ParseError::ExpectedStatementSeparator { .. } => (
            format!("Expected newline between statements, found {found}"),
            "start the next statement on a new line".into(),
            None,
        ),
        ParseError::ExpectedToken { expected, .. } => {
            let close = match *expected {
                "'}'" => Some((TokenKind::LeftBrace, TokenKind::RightBrace, "block")),
                "')'" => Some((TokenKind::LeftParen, TokenKind::RightParen, "parentheses")),
                "']'" => Some((
                    TokenKind::LeftBracket,
                    TokenKind::RightBracket,
                    "array or index",
                )),
                _ => None,
            };
            let help = close.and_then(|(open, close, name)| {
                unmatched_open(&tokens, span.start, &open, &close).map(|opening| {
                    let line = source[..opening.start]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count()
                        + 1;
                    format!("Add {expected} to close the {name} opened on line {line}.")
                })
            });
            (
                format!("Expected {expected}, found {found}"),
                format!("{expected} is required here"),
                help,
            )
        }
        ParseError::UnexpectedEnd => (
            "Unexpected end of file".into(),
            "the program is incomplete".into(),
            None,
        ),
    };
    Diagnostic {
        span,
        message,
        label,
        help,
    }
}

fn unmatched_open(
    tokens: &[Token],
    offset: usize,
    open: &TokenKind,
    close: &TokenKind,
) -> Option<Span> {
    let mut stack = Vec::new();
    for token in tokens.iter().take_while(|token| token.span.start < offset) {
        if &token.kind == open {
            stack.push(token.span);
        } else if &token.kind == close {
            stack.pop();
        }
    }
    stack.pop()
}
