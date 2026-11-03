//! Record type resolution (Issue #11, step 3B): named field types, forward
//! references, undefined types, recursive records and reserved names. Records that
//! pass every check are still reported as unsupported until code generation exists.
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

/// The span of the type name at the end of `field` (for example `"b: B"`), which
/// must appear exactly once in `source`.
fn type_span(source: &str, field: &str) -> Span {
    assert_eq!(source.matches(field).count(), 1, "{field:?} in {source:?}");
    let end = source.find(field).unwrap() + field.len();
    let name = field.rsplit(' ').next().unwrap();
    Span {
        start: end - name.len(),
        end,
    }
}

fn is_valid(source: &str) -> bool {
    check(source) == Ok(())
}

fn recursive(source: &str, path: &[&str], span: Span) {
    assert_eq!(
        check(source),
        Err(TypeError::RecursiveRecord {
            path: path.iter().map(|name| (*name).to_owned()).collect(),
            span,
        }),
        "{source:?}"
    );
}

#[test]
fn valid_nesting_resolves() {
    for source in [
        // One level, two levels, and several fields of different kinds.
        "record Address {\n    city: string\n}\nrecord Person {\n    name: string\n    home: Address\n}",
        "record A {\n    x: int\n}\nrecord B {\n    a: A\n}\nrecord C {\n    b: B\n    a: A\n}",
        // The same record used by several fields and several records (a diamond).
        "record Leaf {\n    v: int\n}\nrecord L {\n    leaf: Leaf\n}\nrecord R {\n    leaf: Leaf\n}\nrecord Top {\n    l: L\n    r: R\n    again: Leaf\n}",
        // The same type twice in one record is fine.
        "record P {\n    x: int\n}\nrecord Line {\n    from: P\n    to: P\n}",
        // Mixed with ordinary code.
        "show 1\nrecord P {\n    x: int\n}\nrecord Q {\n    p: P\n}\nshow 2",
    ] {
        assert!(is_valid(source), "{source:?}: {:?}", check(source));
    }
}

#[test]
fn forward_references_between_records_are_supported() {
    for source in [
        "record Course {\n    teacher: Student\n}\nrecord Student {\n    name: string\n}",
        "record C {\n    b: B\n}\nrecord B {\n    a: A\n}\nrecord A {\n    x: int\n}",
        // A function between the declarations does not matter.
        "record Course {\n    teacher: Student\n}\nfunction f() be int {\n    return 1\n}\nrecord Student {\n    n: int\n}",
    ] {
        assert!(is_valid(source), "{source:?}: {:?}", check(source));
    }
}

#[test]
fn undefined_record_types_are_rejected_at_the_type_name() {
    for (source, record, field, missing) in [
        (
            "record Course {\n    teacher: Missing\n}",
            "Course",
            "teacher",
            "Missing",
        ),
        // Case matters.
        (
            "record Student {\n    x: int\n}\nrecord Course {\n    teacher: student\n}",
            "Course",
            "teacher",
            "student",
        ),
        // The first unresolved field in source order is reported.
        (
            "record A {\n    x: int\n}\nrecord B {\n    ok: A\n    bad: Nope\n    worse: Nada\n}",
            "B",
            "bad",
            "Nope",
        ),
        // A function name or variable is not a type.
        (
            "function Thing() be int {\n    return 1\n}\nrecord R {\n    t: Thing\n}",
            "R",
            "t",
            "Thing",
        ),
    ] {
        assert_eq!(
            check(source),
            Err(TypeError::UndefinedRecordType {
                record: record.into(),
                type_name: missing.into(),
                span: type_span(source, &format!("{field}: {missing}")),
            }),
            "{source:?}"
        );
    }
}

#[test]
fn a_record_containing_itself_directly_is_rejected() {
    let source = "record Node {\n    value: int\n    next: Node\n}";
    recursive(source, &["Node", "Node"], span_of(source, "Node", 1));
    // Even as its only field.
    let source = "record Loop {\n    again: Loop\n}";
    recursive(source, &["Loop", "Loop"], span_of(source, "Loop", 1));
}

#[test]
fn indirect_cycles_are_rejected_with_the_whole_path() {
    // Two records.
    let source = "record A {\n    b: B\n}\nrecord B {\n    a: A\n}";
    recursive(source, &["A", "B", "A"], type_span(source, "a: A"));
    // Three records, declared out of order.
    let source = "record C {\n    a: A\n}\nrecord A {\n    b: B\n}\nrecord B {\n    c: C\n}";
    recursive(source, &["C", "A", "B", "C"], type_span(source, "c: C"));
    // A cycle reached through a record that is not part of it: the cycle is
    // reported, not the entry point.
    let source = "record Entry {\n    x: X\n}\nrecord X {\n    y: Y\n}\nrecord Y {\n    back: X\n}";
    recursive(source, &["X", "Y", "X"], type_span(source, "back: X"));
    // A cycle among later records is found even when earlier records are fine.
    let source = "record Fine {\n    n: int\n}\nrecord P {\n    q: Q\n}\nrecord Q {\n    p: P\n}";
    recursive(source, &["P", "Q", "P"], type_span(source, "p: P"));
}

#[test]
fn shared_and_repeated_dependencies_are_not_cycles() {
    // A diamond and a chain reuse the same finished records many times.
    let mut source = String::from("record R0 {\n    v: int\n}\n");
    for i in 1..=20 {
        source.push_str(&format!(
            "record R{i} {{\n    a: R{p}\n    b: R{p}\n}}\n",
            p = i - 1
        ));
    }
    assert!(is_valid(&source), "{:?}", check(&source));
}

#[test]
fn very_long_chains_do_not_overflow_the_stack() {
    // Declared in reverse so every reference is a forward reference.
    let count = 20_000;
    let mut source = String::new();
    for i in (1..count).rev() {
        source.push_str(&format!("record R{i} {{\n    next: R{}\n}}\n", i + 1));
    }
    source.push_str(&format!("record R{count} {{\n    v: int\n}}\n"));
    assert!(is_valid(&source));
    // Close the loop at the far end.
    let closed = source.replace(
        &format!("record R{count} {{\n    v: int\n}}\n"),
        &format!("record R{count} {{\n    back: R1\n}}\n"),
    );
    match check(&closed) {
        Err(TypeError::RecursiveRecord { path, .. }) => {
            assert_eq!(path.len(), count + 1);
            assert_eq!(path.first(), path.last());
        }
        other => panic!("expected a cycle, got {other:?}"),
    }
}

#[test]
fn builtin_type_names_cannot_name_a_record() {
    for name in ["int", "float", "bool", "string", "result"] {
        let source = format!("record {name} {{\n    x: int\n}}");
        assert_eq!(
            check(&source),
            Err(TypeError::ReservedRecordName {
                name: name.into(),
                span: Span {
                    start: 0,
                    end: source.len()
                },
            }),
            "{source:?}"
        );
    }
    // Other words, and names that merely contain or differ in case from a built-in.
    for name in ["Int", "String", "results", "integer", "int2", "Result"] {
        let source = format!("record {name} {{\n    x: int\n}}");
        assert_eq!(check(&source), Ok(()), "{source:?}");
    }
}

#[test]
fn earlier_validation_rules_still_apply_and_come_first() {
    // Duplicate record, duplicate field and placement errors are unchanged...
    assert!(matches!(
        check("record A {\n    x: int\n}\nrecord A {\n    x: int\n}"),
        Err(TypeError::DuplicateRecord { .. })
    ));
    assert!(matches!(
        check("record A {\n    x: int\n    x: int\n}"),
        Err(TypeError::DuplicateField { .. })
    ));
    assert!(matches!(
        check("function f() be int {\n    record A {\n        x: int\n    }\n    return 1\n}"),
        Err(TypeError::RecordNotTopLevel { .. })
    ));
    // ...and are reported before resolution problems, wherever they appear.
    assert!(matches!(
        check("record A {\n    b: Missing\n}\nrecord A {\n    x: int\n}"),
        Err(TypeError::DuplicateRecord { .. })
    ));
    assert!(matches!(
        check("record A {\n    b: A\n}\nrecord int {\n    x: int\n}"),
        Err(TypeError::ReservedRecordName { .. })
    ));
    // Undefined types are reported before cycles.
    assert!(matches!(
        check("record A {\n    a: A\n    m: Missing\n}"),
        Err(TypeError::UndefinedRecordType { .. })
    ));
    // A nested record is a placement error and never joins the registry, so it
    // cannot satisfy a reference.
    assert!(matches!(
        check(
            "record A {\n    b: B\n}\nfunction f() be int {\n    record B {\n        x: int\n    }\n    return 1\n}"
        ),
        Err(TypeError::RecordNotTopLevel { .. })
    ));
}

#[test]
fn programs_without_records_are_unaffected() {
    assert_eq!(check("assign x as 1\nshow x"), Ok(()));
    assert!(matches!(
        check("show missing"),
        Err(TypeError::UndefinedVariable { .. })
    ));
}

#[test]
fn diagnostics_render_clearly_and_produce_no_executable() {
    let directory = std::env::temp_dir().join(format!("cheeku record res {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join("program");
    for (source, fragments) in [
        (
            "record Course {\n    teacher: Missing\n}",
            vec![
                "Unknown type 'Missing' in record 'Course'",
                "p.chuchu:2:14",
                "no record with this name is declared",
                "help: Declare 'record Missing' at the top level",
            ],
        ),
        (
            "record Node {\n    next: Node\n}",
            vec![
                "Record 'Node' contains itself: Node -> Node",
                "p.chuchu:2:11",
                "this field makes the record infinitely large",
                "stored by value",
            ],
        ),
        (
            "record A {\n    b: B\n}\nrecord B {\n    a: A\n}",
            vec!["Record 'A' contains itself: A -> B -> A", "p.chuchu:5:8"],
        ),
        (
            "record string {\n    x: int\n}",
            vec![
                "'string' is a built-in type name and cannot name a record",
                "p.chuchu:1:1",
            ],
        ),
        (
            "record A {\n    x: Missing\n}",
            vec!["Unknown type 'Missing' in record 'A'"],
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
    let plain = |source: &str| compile_source(source, &output).unwrap_err().to_string();
    assert!(
        plain("record A {\n    b: Z\n}").starts_with("Undefined type \"Z\" in record \"A\" at "),
        "undefined"
    );
    assert!(
        plain("record A {\n    a: A\n}").starts_with("Recursive record definition A -> A at "),
        "recursive"
    );
    assert!(
        plain("record int {\n    a: int\n}")
            .starts_with("Record name \"int\" is a built-in type name at "),
        "reserved"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}
