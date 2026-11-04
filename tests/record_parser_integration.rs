//! Parsing of `record` declarations (Issue #11, step 2). Records are parsed into
//! the AST only; the type checker and code generator reject them until later steps.
use chuchu::ast::{FieldType, RecordField, Stmt, StmtKind};
use chuchu::compiler::{CompileError, compile_source};
use chuchu::diagnostic::render;
use chuchu::lexer::tokenize;
use chuchu::parser::{ParseError, Parser};
use chuchu::symbol_table::SymbolTable;
use chuchu::token::Span;
use chuchu::type_checker::{TypeChecker, TypeError};
use chuchu::types::Type;
use std::path::Path;

fn parse(source: &str) -> Result<Vec<Stmt>, ParseError> {
    Parser::new(tokenize(source).unwrap()).parse_program()
}

fn field(name: &str, ty: FieldType, start: usize, end: usize) -> RecordField {
    RecordField {
        name: name.into(),
        ty,
        span: Span { start, end },
    }
}

#[test]
fn parses_a_record_with_every_builtin_field_type() {
    let source =
        "record Student {\n    name: string\n    age: int\n    gpa: float\n    active: bool\n}";
    let statements = parse(source).unwrap();
    assert_eq!(
        statements,
        vec![Stmt {
            kind: StmtKind::Record {
                name: "Student".into(),
                fields: vec![
                    field("name", FieldType::Builtin(Type::String), 21, 33),
                    field("age", FieldType::Builtin(Type::Int), 38, 46),
                    field("gpa", FieldType::Builtin(Type::Float), 51, 61),
                    field("active", FieldType::Builtin(Type::Bool), 66, 78),
                ],
            },
            span: Span {
                start: 0,
                end: source.len()
            },
        }]
    );
}

#[test]
fn user_defined_type_names_are_recognised_syntactically() {
    let statements = parse("record Course {\n    teacher: Student\n    title: string\n}").unwrap();
    let StmtKind::Record { fields, .. } = &statements[0].kind else {
        panic!("expected a record");
    };
    assert_eq!(fields[0].ty, FieldType::Named("Student".into()));
    assert_eq!(fields[1].ty, FieldType::Builtin(Type::String));
    // Names are not resolved at parse time: an unknown type is still accepted.
    assert!(parse("record A {\n    b: DoesNotExist\n}").is_ok());
}

#[test]
fn follows_existing_newline_and_block_conventions() {
    // CRLF, blank lines, comments, a one-line record, and surrounding statements.
    for source in [
        "record P {\r\n    x: int\r\n\r\n    y: int\r\n}\r\n",
        "record P {\n\n    x: int\n\n}\n",
        "record P { x: int }",
        "record P {\n    x: int // the x coordinate\n    /* y */ y: int\n}",
        "record P {\n    x: int\n    y: int }",
        "show 1\nrecord P {\n    x: int\n}\nshow 2",
    ] {
        let statements = parse(source).unwrap_or_else(|error| panic!("{source:?}: {error:?}"));
        assert!(
            statements
                .iter()
                .any(|statement| matches!(statement.kind, StmtKind::Record { .. })),
            "{source:?}"
        );
    }
    // Records sit among other statements without disturbing them.
    let statements = parse("show 1\nrecord P {\n    x: int\n}\nshow 2").unwrap();
    assert_eq!(statements.len(), 3);
    assert!(matches!(statements[0].kind, StmtKind::Show(_)));
    assert!(matches!(statements[2].kind, StmtKind::Show(_)));
}

#[test]
fn field_names_may_reuse_type_and_function_words() {
    // Only `record` itself is reserved; type names are ordinary identifiers.
    let statements = parse("record R {\n    int: string\n    string: int\n}").unwrap();
    let StmtKind::Record { fields, .. } = &statements[0].kind else {
        panic!("expected a record");
    };
    assert_eq!(fields[0].name, "int");
    assert_eq!(fields[1].ty, FieldType::Builtin(Type::Int));
}

#[test]
fn malformed_declarations_report_the_offending_token() {
    let expected_token = |expected: &'static str, start: usize, end: usize| {
        Err(ParseError::ExpectedToken {
            expected,
            span: Span { start, end },
        })
    };
    // (source, expected error)
    let cases: Vec<(&str, Result<Vec<Stmt>, ParseError>)> = vec![
        // Missing or invalid record name.
        (
            "record {\n    x: int\n}",
            Err(ParseError::ExpectedIdentifier {
                span: Span { start: 7, end: 8 },
            }),
        ),
        (
            "record 5 {\n    x: int\n}",
            Err(ParseError::ExpectedIdentifier {
                span: Span { start: 7, end: 8 },
            }),
        ),
        (
            "record record {\n    x: int\n}",
            Err(ParseError::ExpectedIdentifier {
                span: Span { start: 7, end: 13 },
            }),
        ),
        // The opening brace must follow the name on the same line.
        ("record P\n{\n    x: int\n}", expected_token("'{'", 8, 9)),
        ("record P", expected_token("'{'", 8, 8)),
        // Empty records are not allowed.
        ("record P {}", expected_token("at least one field", 10, 11)),
        (
            "record P {\n\n}",
            expected_token("at least one field", 12, 13),
        ),
        // Unterminated declaration.
        ("record P {\n    x: int\n", expected_token("'}'", 22, 22)),
        // Field syntax.
        ("record P {\n    x int\n}", expected_token("':'", 17, 20)),
        (
            "record P {\n    : int\n}",
            Err(ParseError::ExpectedIdentifier {
                span: Span { start: 15, end: 16 },
            }),
        ),
        (
            "record P {\n    x:\n}",
            expected_token("int, float, bool, string, or a record type name", 17, 18),
        ),
        (
            "record P {\n    x: 5\n}",
            expected_token("int, float, bool, string, or a record type name", 18, 19),
        ),
        (
            "record P {\n    x: [1]\n}",
            expected_token("int, float, bool, string, or a record type name", 18, 19),
        ),
        (
            "record P {\n    x: result<int>\n}",
            expected_token("int, float, bool, string, or a record type name", 18, 24),
        ),
        // Fields are separated by newlines only.
        (
            "record P {\n    x: int y: int\n}",
            expected_token("newline between fields", 22, 23),
        ),
        (
            "record P {\n    x: int,\n    y: int\n}",
            expected_token("newline between fields", 21, 22),
        ),
        (
            "record P {\n    x: int = 5\n}",
            expected_token("newline between fields", 22, 23),
        ),
        // Nothing may follow the closing brace on the same line.
        (
            "record P {\n    x: int\n} show 1",
            Err(ParseError::ExpectedStatementSeparator {
                span: Span { start: 24, end: 28 },
            }),
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(parse(source), expected, "{source:?}");
    }
}

#[test]
fn record_is_not_valid_where_an_expression_is_expected() {
    assert!(parse("assign x as record").is_err());
    assert!(parse("show record").is_err());
}

#[test]
fn declarations_are_accepted_by_the_checker() {
    for source in [
        "record P {\n    s: string\n}",
        "show 1\nrecord P {\n    s: string\n}",
        "record P {\n    x: int\n}",
    ] {
        let statements = parse(source).unwrap();
        assert_eq!(
            TypeChecker::new(SymbolTable::new()).check_program(&statements),
            Ok(()),
            "{source:?}"
        );
    }
}

#[test]
fn compiling_an_invalid_record_produces_a_clear_error_and_no_executable() {
    let directory = std::env::temp_dir().join(format!("cheeku record {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join("program");
    let source = "show 1\nrecord Student {\n    name: string\n    name: int\n}\nshow 2";
    let error = compile_source(source, &output).unwrap_err();
    assert!(matches!(
        error,
        CompileError::Type(TypeError::DuplicateField { .. })
    ));
    assert!(!output.exists());
    let diagnostic = render(&error, Path::new("students.chuchu"), source);
    assert!(
        diagnostic.contains("Duplicate field 'name' in record 'Student'"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("students.chuchu:4:5"), "{diagnostic}");
    std::fs::remove_dir_all(&directory).unwrap();
}
