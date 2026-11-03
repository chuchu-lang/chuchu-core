//! Which record programs can be compiled (Issue #11, step 7B). Records with `int`,
//! `float`, and `bool` fields can be created and read; everything else about records
//! is a valid program that the checker refuses with an explicit error, so it can
//! never reach code generation.
use chuchu::compiler::{CompileError, compile_source};
use chuchu::diagnostic::render;
use chuchu::lexer::tokenize;
use chuchu::parser::Parser;
use chuchu::symbol_table::SymbolTable;
use chuchu::token::Span;
use chuchu::type_checker::{TypeChecker, TypeError};
use std::path::Path;

/// A supported record and a variable of it.
const PRELUDE: &str = "record Sample {\n    count: int\n    ratio: float\n    ready: bool\n}\nassign s as Sample {\n    count: 1\n    ratio: 2.5\n    ready: true\n}\nassign n as 0\n";

fn parse(source: &str) -> Vec<chuchu::ast::Stmt> {
    Parser::new(tokenize(source).unwrap())
        .parse_program()
        .unwrap_or_else(|error| panic!("{source:?} should parse: {error:?}"))
}

fn check(source: &str) -> Result<(), TypeError> {
    TypeChecker::new(SymbolTable::new()).check_program(&parse(source))
}

fn check_body(body: &str) -> Result<(), TypeError> {
    check(&format!("{PRELUDE}{body}"))
}

/// The span of the last match of `text`, which is the one in an appended body.
fn last_span_of(source: &str, text: &str) -> Span {
    let start = source
        .rmatch_indices(text)
        .next()
        .unwrap_or_else(|| panic!("{text:?} not in {source:?}"))
        .0;
    Span {
        start,
        end: start + text.len(),
    }
}

fn unsupported_use(feature: &'static str, span: Span) -> Result<(), TypeError> {
    Err(TypeError::UnsupportedRecordUse { feature, span })
}

#[test]
fn creating_and_reading_records_is_supported() {
    for body in [
        "",
        "show s.count\nshow s.ratio\nshow s.ready",
        // Reads take part in ordinary expressions and statements.
        "assign x as s.count + 1\nreassign n as s.count * 2\nshow x + n",
        "if s.ready {\n    show s.count\n}",
        "while n < s.count {\n    reassign n as n + 1\n}",
        "assign xs as [s.count, 2, 3]\nshow xs[s.count]",
        "function twice(k: int) be int {\n    return k * 2\n}\nshow twice(s.count)",
        // Initializers: temporaries, in blocks and functions, and values read from
        // other records.
        "show Sample {\n    count: 5\n    ratio: 0.5\n    ready: false\n}.count",
        "assign t as Sample {\n    count: s.count\n    ratio: s.count\n    ready: s.ready\n}\nshow t.ratio",
        "if true {\n    assign t as Sample {\n        count: 1\n        ratio: 1.0\n        ready: true\n    }\n    show t.count\n}",
        "function f() be int {\n    assign t as Sample {\n        count: 4\n        ratio: 1.0\n        ready: true\n    }\n    return t.count\n}\nshow f()",
    ] {
        assert_eq!(check_body(body), Ok(()), "{body:?}");
    }
}

#[test]
fn a_record_may_be_used_before_its_declaration() {
    assert_eq!(
        check("assign p as P {\n    x: 1\n}\nshow p.x\nrecord P {\n    x: int\n}"),
        Ok(())
    );
}

#[test]
fn fields_other_than_int_float_and_bool_are_not_supported() {
    for (source, name, declaration) in [
        (
            "record N {\n    name: string\n    n: int\n}",
            "N",
            "record N {\n    name: string\n    n: int\n}",
        ),
        (
            "record Inner {\n    n: int\n}\nrecord Outer {\n    inner: Inner\n}",
            "Outer",
            "record Outer {\n    inner: Inner\n}",
        ),
        // The first unsupported declaration is reported, not the first record.
        (
            "record A {\n    x: int\n}\nrecord B {\n    s: string\n}\nrecord C {\n    t: string\n}",
            "B",
            "record B {\n    s: string\n}",
        ),
        // A record that is never used is refused all the same.
        (
            "show 1\nrecord N {\n    s: string\n}\nshow 2",
            "N",
            "record N {\n    s: string\n}",
        ),
    ] {
        let start = source.find(declaration).unwrap();
        assert_eq!(
            check(source),
            Err(TypeError::UnsupportedRecord {
                name: name.into(),
                span: Span {
                    start,
                    end: start + declaration.len()
                },
            }),
            "{source:?}"
        );
    }
}

#[test]
fn operations_on_whole_records_are_not_supported_yet() {
    let source = format!("{PRELUDE}assign t as s");
    assert_eq!(
        check(&source),
        unsupported_use("copying a record", last_span_of(&source, "s"))
    );
    let source = format!(
        "{PRELUDE}reassign s as Sample {{\n    count: 2\n    ratio: 1.0\n    ready: false\n}}"
    );
    assert_eq!(
        check(&source),
        unsupported_use(
            "reassigning a record variable",
            Span {
                start: source.rfind("reassign").unwrap(),
                end: source.len()
            }
        )
    );
    let source = format!("{PRELUDE}reassign s.count as 5");
    assert_eq!(
        check(&source),
        unsupported_use("assigning a record field", last_span_of(&source, "s.count"))
    );
    let source = format!("{PRELUDE}show s");
    assert_eq!(
        check(&source),
        unsupported_use("printing a record", last_span_of(&source, "s"))
    );
    let source =
        format!("{PRELUDE}show Sample {{\n    count: 1\n    ratio: 1.0\n    ready: true\n}}");
    assert_eq!(
        check(&source),
        unsupported_use(
            "printing a record",
            Span {
                start: source.rfind("Sample").unwrap(),
                end: source.len()
            }
        )
    );
}

#[test]
fn unsupported_operations_are_found_inside_functions_and_blocks() {
    for (body, feature) in [
        ("if true {\n    assign t as s\n}", "copying a record"),
        (
            "while false {\n    reassign s.count as 1\n}",
            "assigning a record field",
        ),
        (
            "function f() be int {\n    assign t as Sample {\n        count: 1\n        ratio: 1.0\n        ready: true\n    }\n    show t\n    return 1\n}",
            "printing a record",
        ),
        (
            "function f() be int {\n    assign t as Sample {\n        count: 1\n        ratio: 1.0\n        ready: true\n    }\n    reassign t.count as 2\n    return 1\n}",
            "assigning a record field",
        ),
    ] {
        let error = check_body(body).unwrap_err();
        assert!(
            matches!(&error, TypeError::UnsupportedRecordUse { feature: found, .. } if *found == feature),
            "{body:?}: {error:?}"
        );
    }
}

#[test]
fn real_errors_take_precedence_over_unsupported_operations() {
    // The unsupported operation is remembered, and an actual error after it wins.
    for (body, expected) in [
        ("assign t as s\nshow missing", "UndefinedVariable"),
        ("show s\nshow s.nope", "UnknownField"),
        (
            "reassign s.count as 1\nshow s.count.x",
            "FieldAccessOnNonRecord",
        ),
        ("assign t as s\nreassign n as \"text\"", "Mismatch"),
    ] {
        let error = check_body(body).unwrap_err();
        assert!(
            format!("{error:?}").starts_with(expected),
            "{body:?}: {error:?}"
        );
    }
    // An error inside the unsupported statement itself is reported as that error.
    assert!(matches!(
        check_body("reassign s.count as \"text\""),
        Err(TypeError::FieldMismatch { .. })
    ));
    assert!(matches!(
        check_body("reassign s.nope as 1"),
        Err(TypeError::UnknownField { .. })
    ));
    assert!(matches!(
        check_body("reassign s as 5"),
        Err(TypeError::RecordMismatch { .. })
    ));
}

#[test]
fn the_first_unsupported_feature_in_source_order_is_reported() {
    // A use before a later unsupported declaration.
    let source = format!("{PRELUDE}show s\nrecord N {{\n    name: string\n}}");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnsupportedRecordUse {
            feature: "printing a record",
            ..
        })
    ));
    // A declaration before a later unsupported use.
    let source = format!("record N {{\n    name: string\n}}\n{PRELUDE}show s");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnsupportedRecord { name, .. }) if name == "N"
    ));
    // Of several unsupported uses, the first.
    assert!(matches!(
        check_body("assign t as s\nshow s"),
        Err(TypeError::UnsupportedRecordUse {
            feature: "copying a record",
            ..
        })
    ));
}

#[test]
fn a_statement_checked_on_its_own_is_never_silently_accepted() {
    // Outside `check_program` there is no end of program to report at, so an
    // unsupported operation is returned at once rather than remembered.
    let mut checker = TypeChecker::new(SymbolTable::new());
    checker.check_program(&parse(PRELUDE)).unwrap();
    for (text, feature) in [
        ("show s", "printing a record"),
        ("assign t as s", "copying a record"),
        ("reassign s.count as 1", "assigning a record field"),
    ] {
        let statements = parse(text);
        let result = checker.check_statement(&statements[0]);
        assert!(
            matches!(&result, Err(TypeError::UnsupportedRecordUse { feature: found, .. })
                if *found == feature),
            "{text:?}: {result:?}"
        );
    }
    // A supported statement is accepted.
    let statements = parse("show s.count");
    assert_eq!(checker.check_statement(&statements[0]), Ok(()));
}

#[test]
fn an_unsupported_use_does_not_leak_into_the_next_program() {
    let mut checker = TypeChecker::new(SymbolTable::new());
    assert!(
        checker
            .check_program(&parse(&format!("{PRELUDE}show s")))
            .is_err()
    );
    // The pending error belongs to that run only.
    let mut fresh = TypeChecker::new(SymbolTable::new());
    assert_eq!(fresh.check_program(&parse("show 1")), Ok(()));
}

#[test]
fn the_record_metadata_is_available_for_programs_with_unsupported_uses() {
    // Validation succeeded, so the table exists even though the program is refused.
    let mut checker = TypeChecker::new(SymbolTable::new());
    assert!(
        checker
            .check_program(&parse(&format!("{PRELUDE}show s")))
            .is_err()
    );
    assert_eq!(checker.record_table().unwrap().len(), 1);
}

#[test]
fn unsupported_operations_are_explained_clearly_and_produce_no_executable() {
    let directory =
        std::env::temp_dir().join(format!("cheeku record support {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join("program");
    let line = PRELUDE.lines().count() + 1;
    for (body, fragments) in [
        (
            "assign t as s",
            vec![
                "Records do not support copying a record yet".to_owned(),
                format!("p.chuchu:{line}:13"),
                "this operation on a record cannot be compiled yet".to_owned(),
                "help: Records can be created with an initializer and read with 'variable.field'"
                    .to_owned(),
            ],
        ),
        (
            "show s",
            vec![
                "Records do not support printing a record yet".to_owned(),
                format!("p.chuchu:{line}:6"),
            ],
        ),
        (
            "reassign s.count as 5",
            vec![
                "Records do not support assigning a record field yet".to_owned(),
                format!("p.chuchu:{line}:10"),
            ],
        ),
        (
            "reassign s as s",
            vec!["Records do not support reassigning a record variable yet".to_owned()],
        ),
    ] {
        let source = format!("{PRELUDE}{body}");
        let error = compile_source(&source, &output).unwrap_err();
        assert!(
            matches!(
                &error,
                CompileError::Type(TypeError::UnsupportedRecordUse { .. })
            ),
            "{body:?}: {error:?}"
        );
        let diagnostic = render(&error, Path::new("p.chuchu"), &source);
        for fragment in fragments {
            assert!(diagnostic.contains(&fragment), "{fragment}\n{diagnostic}");
        }
        assert!(!output.exists(), "{body:?}");
    }
    // The plain message, used where there is no source to render.
    let error = compile_source(&format!("{PRELUDE}show s"), &output).unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("Records do not support printing a record yet at "),
        "{error}"
    );
    // A record with fields that are not supported.
    let source = "record N {\n    name: string\n}";
    let error = compile_source(source, &output).unwrap_err();
    let diagnostic = render(&error, Path::new("p.chuchu"), source);
    for fragment in [
        "Record 'N' cannot be compiled yet",
        "this record has a field type that is not supported yet",
        "help: Records currently support only int, float, and bool fields.",
    ] {
        assert!(diagnostic.contains(fragment), "{fragment}\n{diagnostic}");
    }
    assert!(
        error
            .to_string()
            .starts_with("Record \"N\" has fields that are not supported yet at ")
    );
    assert!(!output.exists());
    std::fs::remove_dir_all(&directory).unwrap();
}
