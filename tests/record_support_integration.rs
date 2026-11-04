//! Which record programs the checker accepts (Issue #11, steps 7B to 7F). Records
//! can be created, copied, read, written, reassigned, passed to functions and
//! returned from them. The one thing left that is a valid program but cannot be
//! compiled is printing a whole record, which the checker refuses with an explicit
//! error so it can never reach code generation.
use chuchu::compiler::{CompileError, compile_source};
use chuchu::diagnostic::render;
use chuchu::lexer::tokenize;
use chuchu::parser::Parser;
use chuchu::symbol_table::SymbolTable;
use chuchu::token::Span;
use chuchu::type_checker::{TypeChecker, TypeError};
use std::path::Path;

/// A record with a string field, and a variable of it.
const PRELUDE: &str = "record Sample {\n    count: int\n    ratio: float\n    ready: bool\n    label: string\n}\nassign s as Sample {\n    count: 1\n    ratio: 2.5\n    ready: true\n    label: \"a\"\n}\nassign n as 0\n";

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
fn creating_reading_copying_writing_and_reassigning_records_is_supported() {
    for body in [
        "",
        "show s.count\nshow s.ratio\nshow s.ready\nshow s.label",
        // Copies and reassignment, including self-assignment.
        "assign t as s\nreassign t.count as 2\nshow t.count + s.count",
        "reassign s as s",
        "assign t as s\nreassign t as s\nreassign s as t",
        // Writes.
        "reassign s.label as s.label + \"b\"",
        "reassign s.ratio as 3",
        // Reads take part in ordinary expressions and statements.
        "assign x as s.count + 1\nreassign n as s.count * 2\nshow x + n",
        "if s.ready {\n    show s.label\n}",
        "while n < s.count {\n    reassign n as n + 1\n}",
        // Initializers: temporaries, in blocks, and values read from other records.
        "show Sample {\n    count: 5\n    ratio: 0.5\n    ready: false\n    label: \"b\"\n}.label",
        "assign t as Sample {\n    count: s.count\n    ratio: s.count\n    ready: s.ready\n    label: s.label\n}\nshow t.ratio",
        "if true {\n    assign t as s\n    show t.count\n}",
    ] {
        assert_eq!(check_body(body), Ok(()), "{body:?}");
    }
}

#[test]
fn functions_may_take_and_return_records() {
    for body in [
        "function f(r: Sample) be int {\n    return r.count\n}\nshow f(s)",
        "function g() be Sample {\n    return Sample {\n        count: 1\n        ratio: 1.0\n        ready: true\n        label: \"g\"\n    }\n}\nshow g().label\nassign t as g()\nreassign s as g()",
        "function h(r: Sample) be Sample {\n    reassign r.count as r.count + 1\n    return r\n}\nassign t as h(s)\nshow t.count",
        "function two(a: Sample, b: Sample) be int {\n    return a.count + b.count\n}\nshow two(s, s)",
        // A record parameter next to others, and a record built from parameters.
        "function make(c: int, l: string) be Sample {\n    assign r as Sample {\n        count: c\n        ratio: 0.0\n        ready: false\n        label: l\n    }\n    return r\n}\nshow make(3, \"x\").count",
    ] {
        assert_eq!(check_body(body), Ok(()), "{body:?}");
    }
    // The function may be declared after its use, and the record after both.
    assert_eq!(
        check(
            "show f(P {\n    x: 1\n}).x\nfunction f(p: P) be P {\n    return p\n}\nrecord P {\n    x: int\n}"
        ),
        Ok(())
    );
}

#[test]
fn a_record_may_be_used_before_its_declaration() {
    assert_eq!(
        check("assign p as P {\n    x: 1\n}\nshow p.x\nrecord P {\n    x: int\n}"),
        Ok(())
    );
}

#[test]
fn printing_a_whole_record_is_not_supported_yet() {
    let source = format!("{PRELUDE}show s");
    assert_eq!(
        check(&source),
        unsupported_use("printing a record", last_span_of(&source, "s"))
    );
    let source = format!(
        "{PRELUDE}show Sample {{\n    count: 1\n    ratio: 1.0\n    ready: true\n    label: \"x\"\n}}"
    );
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
    // Inside functions and blocks too.
    for body in [
        "if true {\n    show s\n}",
        "function f(r: Sample) be int {\n    show r\n    return 1\n}",
    ] {
        let error = check_body(body).unwrap_err();
        assert!(
            matches!(
                &error,
                TypeError::UnsupportedRecordUse {
                    feature: "printing a record",
                    ..
                }
            ),
            "{body:?}: {error:?}"
        );
    }
}

#[test]
fn real_errors_take_precedence_over_unsupported_printing() {
    // Printing is remembered, and an actual error after it wins.
    for (body, expected) in [
        ("show s\nshow missing", "UndefinedVariable"),
        ("show s\nshow s.nope", "UnknownField"),
        ("show s\nshow s.count.x", "FieldAccessOnNonRecord"),
        ("show s\nreassign n as \"text\"", "Mismatch"),
    ] {
        let error = check_body(body).unwrap_err();
        assert!(
            format!("{error:?}").starts_with(expected),
            "{body:?}: {error:?}"
        );
    }
    // The first unsupported print in source order is reported.
    let source = format!("{PRELUDE}show s\nassign t as s\nshow t");
    assert_eq!(
        check(&source),
        unsupported_use(
            "printing a record",
            Span {
                start: source.find("show s").unwrap() + 5,
                end: source.find("show s").unwrap() + 6
            }
        )
    );
}

#[test]
fn a_statement_checked_on_its_own_is_never_silently_accepted() {
    // Outside `check_program` there is no end of program to report at, so the
    // unsupported print is returned at once rather than remembered.
    let mut checker = TypeChecker::new(SymbolTable::new());
    checker.check_program(&parse(PRELUDE)).unwrap();
    let statements = parse("show s");
    let result = checker.check_statement(&statements[0]);
    assert!(
        matches!(
            &result,
            Err(TypeError::UnsupportedRecordUse {
                feature: "printing a record",
                ..
            })
        ),
        "{result:?}"
    );
    // Supported statements are accepted.
    for text in [
        "show s.count",
        "assign t as s",
        "reassign s.count as 1",
        "reassign s as s",
    ] {
        let statements = parse(text);
        assert_eq!(checker.check_statement(&statements[0]), Ok(()), "{text:?}");
    }
}

#[test]
fn an_unsupported_print_does_not_leak_into_the_next_program() {
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
fn the_record_metadata_is_available_for_programs_with_unsupported_prints() {
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
fn printing_a_record_is_explained_clearly_and_produces_no_executable() {
    let directory =
        std::env::temp_dir().join(format!("cheeku record support {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join("program");
    let line = PRELUDE.lines().count() + 1;
    let source = format!("{PRELUDE}show s");
    let error = compile_source(&source, &output).unwrap_err();
    assert!(
        matches!(
            &error,
            CompileError::Type(TypeError::UnsupportedRecordUse { .. })
        ),
        "{error:?}"
    );
    let diagnostic = render(&error, Path::new("p.chuchu"), &source);
    for fragment in [
        "Records do not support printing a record yet".to_owned(),
        format!("p.chuchu:{line}:6"),
        "this operation on a record cannot be compiled yet".to_owned(),
        "help: Print the fields of the record instead, such as 'show student.age'".to_owned(),
    ] {
        assert!(diagnostic.contains(&fragment), "{fragment}\n{diagnostic}");
    }
    assert!(!output.exists());
    // The plain message, used where there is no source to render.
    assert!(
        error
            .to_string()
            .starts_with("Records do not support printing a record yet at "),
        "{error}"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}
