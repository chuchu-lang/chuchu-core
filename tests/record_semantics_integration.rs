//! Semantic validation of `record` declarations (Issue #11, step 3A). A record that
//! passes validation is still reported as unsupported until code generation exists.
use chuchu::compiler::{CompileError, compile_source};
use chuchu::diagnostic::render;
use chuchu::lexer::tokenize;
use chuchu::parser::Parser;
use chuchu::symbol_table::SymbolTable;
use chuchu::token::Span;
use chuchu::type_checker::{TypeChecker, TypeError};
use std::path::Path;

fn check(source: &str) -> Result<(), TypeError> {
    let statements = Parser::new(tokenize(source).unwrap())
        .parse_program()
        .unwrap_or_else(|error| panic!("{source:?} should parse: {error:?}"));
    TypeChecker::new(SymbolTable::new()).check_program(&statements)
}

/// The span of the `occurrence`-th (0-based) match of `text` in `source`.
fn span_of(source: &str, text: &str, occurrence: usize) -> Span {
    let start = source
        .match_indices(text)
        .nth(occurrence)
        .unwrap_or_else(|| panic!("{text:?} #{occurrence} not in {source:?}"))
        .0;
    Span {
        start,
        end: start + text.len(),
    }
}

fn unsupported(name: &str, span: Span) -> Result<(), TypeError> {
    Err(TypeError::UnsupportedRecord {
        name: name.into(),
        span,
    })
}

#[test]
fn valid_records_are_still_reported_as_unsupported() {
    for (source, name, declaration) in [
        (
            "record Student {\n    name: string\n    age: int\n}",
            "Student",
            "record Student {\n    name: string\n    age: int\n}",
        ),
        // Same field names in different records, and names that differ only in case.
        (
            "record A {\n    x: int\n}\nrecord B {\n    x: int\n}",
            "A",
            "record A {\n    x: int\n}",
        ),
        (
            "record Student {\n    x: int\n}\nrecord student {\n    x: int\n}",
            "Student",
            "record Student {\n    x: int\n}",
        ),
        (
            "record P {\n    x: int\n    X: int\n}",
            "P",
            "record P {\n    x: int\n    X: int\n}",
        ),
        // Named types that resolve (the type is declared) are valid.
        (
            "record Course {\n    teacher: Student\n}\nrecord Student {\n    name: string\n}",
            "Course",
            "record Course {\n    teacher: Student\n}",
        ),
        // A record after other valid statements.
        (
            "show 1\nrecord P {\n    x: int\n}\nshow 2",
            "P",
            "record P {\n    x: int\n}",
        ),
    ] {
        assert_eq!(
            check(source),
            unsupported(name, span_of(source, declaration, 0)),
            "{source:?}"
        );
    }
}

#[test]
fn duplicate_record_declarations_are_rejected_at_the_second_declaration() {
    for source in [
        "record P {\n    x: int\n}\nrecord P {\n    x: int\n}",
        // The second declaration need not match the first.
        "record P {\n    x: int\n}\nrecord P {\n    y: string\n}",
        // Intervening statements and other records do not matter.
        "record P {\n    x: int\n}\nrecord Q {\n    x: int\n}\nshow 1\nrecord P {\n    z: bool\n}",
    ] {
        let second = source.rmatch_indices("record P").next().unwrap().0;
        let end = source[second..].find('}').unwrap() + second + 1;
        assert_eq!(
            check(source),
            Err(TypeError::DuplicateRecord {
                name: "P".into(),
                span: Span { start: second, end },
            }),
            "{source:?}"
        );
    }
}

#[test]
fn duplicate_field_names_are_rejected_at_the_second_field() {
    // (source, record, field, text of the duplicate field, which match of it is the duplicate)
    for (source, record, field, text, occurrence) in [
        (
            "record P {\n    x: int\n    x: int\n}",
            "P",
            "x",
            "x: int",
            1,
        ),
        // Same name with a different type is still a duplicate.
        (
            "record P {\n    x: int\n    x: string\n}",
            "P",
            "x",
            "x: string",
            0,
        ),
        (
            "record P {\n    name: string\n    age: int\n    gpa: float\n    age: bool\n}",
            "P",
            "age",
            "age: bool",
            0,
        ),
        // Reported for the record that contains it.
        (
            "record A {\n    x: int\n}\nrecord B {\n    y: int\n    y: int\n}",
            "B",
            "y",
            "y: int",
            1,
        ),
    ] {
        assert_eq!(
            check(source),
            Err(TypeError::DuplicateField {
                record: record.into(),
                field: field.into(),
                span: span_of(source, text, occurrence),
            }),
            "{source:?}"
        );
    }
}

#[test]
fn records_inside_functions_and_blocks_are_rejected() {
    for (source, name) in [
        (
            "function f() be int {\n    record P {\n        x: int\n    }\n    return 1\n}",
            "P",
        ),
        ("if true {\n    record P {\n        x: int\n    }\n}", "P"),
        (
            "if false {\n    show 1\n} else {\n    record P {\n        x: int\n    }\n}",
            "P",
        ),
        (
            "while false {\n    record P {\n        x: int\n    }\n}",
            "P",
        ),
        (
            "function f() be int {\n    while true {\n        if true {\n            record Deep {\n                x: int\n            }\n        }\n    }\n    return 1\n}",
            "Deep",
        ),
    ] {
        let start = source.find("record").unwrap();
        let end = start + source[start..].find("}\n").unwrap() + 1;
        assert_eq!(
            check(source),
            Err(TypeError::RecordNotTopLevel {
                name: name.into(),
                span: Span { start, end },
            }),
            "{source:?}"
        );
    }
}

#[test]
fn nested_records_are_not_registered_and_never_count_as_duplicates() {
    // A nested record named like a top-level one is a placement error, not a duplicate.
    let source = "record P {\n    x: int\n}\nfunction f() be int {\n    record P {\n        y: int\n    }\n    return 1\n}";
    let nested = span_of(source, "record P {\n        y: int\n    }", 0);
    assert_eq!(
        check(source),
        Err(TypeError::RecordNotTopLevel {
            name: "P".into(),
            span: nested,
        })
    );
    // Two nested records with one name: the first placement error wins.
    let source = "if true {\n    record P {\n        x: int\n    }\n    record P {\n        x: int\n    }\n}";
    assert!(matches!(
        check(source),
        Err(TypeError::RecordNotTopLevel { .. })
    ));
}

#[test]
fn declaration_errors_are_reported_before_the_unsupported_error() {
    // An earlier, valid record does not hide a later invalid declaration.
    let source = "record A {\n    x: int\n}\nrecord A {\n    x: int\n}";
    assert!(matches!(
        check(source),
        Err(TypeError::DuplicateRecord { .. })
    ));
    let source = "record A {\n    x: int\n}\nrecord B {\n    y: int\n    y: int\n}";
    assert!(matches!(
        check(source),
        Err(TypeError::DuplicateField { .. })
    ));
    let source = "record A {\n    x: int\n}\nfunction f() be int {\n    record B {\n        y: int\n    }\n    return 1\n}";
    assert!(matches!(
        check(source),
        Err(TypeError::RecordNotTopLevel { .. })
    ));
    // Duplicate record is found before duplicate fields in the later record.
    let source = "record A {\n    x: int\n}\nrecord A {\n    y: int\n    y: int\n}";
    assert!(matches!(
        check(source),
        Err(TypeError::DuplicateRecord { .. })
    ));
}

#[test]
fn other_type_errors_before_a_record_keep_their_precedence() {
    // Statements are checked in source order after record validation.
    let source = "show missing\nrecord P {\n    x: int\n}";
    assert!(matches!(
        check(source),
        Err(TypeError::UndefinedVariable { .. })
    ));
    // A record never changes the behaviour of programs without one.
    assert_eq!(check("assign x as 1\nshow x"), Ok(()));
    assert_eq!(
        check("function f(a: int) be int { return a }\nshow f(1)"),
        Ok(())
    );
}

#[test]
fn diagnostics_render_with_clear_messages_and_no_executable() {
    let directory = std::env::temp_dir().join(format!("cheeku record sem {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join("program");
    for (source, fragments) in [
        (
            "record P {\n    x: int\n}\nrecord P {\n    y: int\n}",
            vec![
                "Record 'P' is already declared",
                "p.chuchu:4:1",
                "duplicate record declaration",
                "help: Rename this record or remove the earlier declaration.",
            ],
        ),
        (
            "record P {\n    x: int\n    x: string\n}",
            vec![
                "Duplicate field 'x' in record 'P'",
                "p.chuchu:3:5",
                "this field name is already used in the record",
            ],
        ),
        (
            "function f() be int {\n    record P {\n        x: int\n    }\n    return 1\n}",
            vec![
                "Record 'P' must be declared at the top level",
                "p.chuchu:2:5",
                "records cannot be declared inside functions or blocks",
            ],
        ),
        (
            "record P {\n    x: int\n}",
            vec!["Record 'P' cannot be compiled yet", "p.chuchu:1:1"],
        ),
    ] {
        let error = compile_source(source, &output).unwrap_err();
        assert!(matches!(error, CompileError::Type(_)), "{source:?}");
        let diagnostic = render(&error, Path::new("p.chuchu"), source);
        for fragment in fragments {
            assert!(diagnostic.contains(fragment), "{fragment}\n{diagnostic}");
        }
        assert!(!output.exists(), "{source:?}");
    }
    // The library error text (used by non-rendering callers) names the problem too.
    let source = "record P {\n    x: int\n    x: int\n}";
    let text = compile_source(source, &output).unwrap_err().to_string();
    assert!(
        text.starts_with("Duplicate field \"x\" in record \"P\" at "),
        "{text}"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn check_statement_applies_the_same_placement_rule_when_called_directly() {
    // `check_statement` is public and skips the whole-program validation pass.
    let direct = |source: &str| {
        let statements = Parser::new(tokenize(source).unwrap())
            .parse_program()
            .unwrap();
        TypeChecker::new(SymbolTable::new()).check_statement(&statements[0])
    };
    assert!(matches!(
        direct("record P {\n    x: int\n}"),
        Err(TypeError::UnsupportedRecord { .. })
    ));
    assert!(matches!(
        direct("function f() be int {\n    record P {\n        x: int\n    }\n    return 1\n}"),
        Err(TypeError::RecordNotTopLevel { .. })
    ));
    assert!(matches!(
        direct("if true {\n    record P {\n        x: int\n    }\n}"),
        Err(TypeError::RecordNotTopLevel { .. })
    ));
}
