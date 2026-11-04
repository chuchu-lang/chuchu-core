//! Semantic validation of record initializers (Issue #11, step 4). A program whose
//! initializers are valid is still reported as unsupported until code generation
//! exists, and no executable is ever produced.
use chuchu::compiler::{CompileError, compile_source};
use chuchu::diagnostic::render;
use chuchu::lexer::tokenize;
use chuchu::parser::Parser;
use chuchu::symbol_table::SymbolTable;
use chuchu::token::Span;
use chuchu::type_checker::{FieldMismatch, TypeChecker, TypeError};
use std::path::Path;

const STUDENT: &str = "record Student {\n    name: string\n    age: int\n}\n";

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

fn field_mismatch(
    record: &str,
    field: &str,
    expected: &str,
    actual: &str,
    span: Span,
) -> Result<(), TypeError> {
    Err(TypeError::FieldMismatch {
        detail: Box::new(FieldMismatch {
            record: record.into(),
            field: field.into(),
            expected: expected.into(),
            actual: actual.into(),
        }),
        span,
    })
}

/// What checking a program whose statements are all valid gives.
fn accepted() -> Result<(), TypeError> {
    Ok(())
}

#[test]
fn valid_initializers_check_successfully() {
    for body in [
        "assign s as Student {\n    name: \"Ali\"\n    age: 22\n}",
        // Fields may be given in any order.
        "assign s as Student {\n    age: 22\n    name: \"Ali\"\n}",
        "assign s as Student { name: \"Ali\" }",
    ] {
        // `Student` has two fields, so the single-field form is the odd one out.
        let source = format!("{STUDENT}{body}");
        let expected = if body.contains("age") {
            accepted()
        } else {
            Err(TypeError::MissingFields {
                record: "Student".into(),
                fields: vec!["age".into()],
                span: span_of(&source, "Student { name: \"Ali\" }", 0),
            })
        };
        assert_eq!(check(&source), expected, "{source:?}");
    }
    // Field values are arbitrary expressions of the right type.
    let source = "record P {\n    x: int\n    y: int\n}\nassign a as 1\nassign p as P {\n    x: a + 2 * 3\n    y: -a\n}";
    // `P` has only int fields, so the whole program compiles.
    assert_eq!(check(source), Ok(()));
}

#[test]
fn an_int_value_widens_to_a_float_field_but_not_the_reverse() {
    let declaration = "record M {\n    f: float\n    i: int\n}\n";
    let ok = format!("{declaration}assign m as M {{\n    f: 1\n    i: 2\n}}");
    assert_eq!(check(&ok), Ok(()));
    let bad = format!("{declaration}assign m as M {{\n    f: 1.5\n    i: 2.5\n}}");
    assert_eq!(
        check(&bad),
        field_mismatch("M", "i", "int", "float", span_of(&bad, "2.5", 0))
    );
}

#[test]
fn nested_initializers_are_validated_at_every_level() {
    let records = "record Person {\n    name: string\n}\nrecord Course {\n    title: string\n    teacher: Person\n}\n";
    let valid = format!(
        "{records}assign c as Course {{\n    title: \"Math\"\n    teacher: Person {{\n        name: \"Sam\"\n    }}\n}}"
    );
    assert_eq!(check(&valid), accepted());

    // An error inside the nested initializer is reported at that level.
    let bad = format!(
        "{records}assign c as Course {{\n    title: \"Math\"\n    teacher: Person {{\n        name: 5\n    }}\n}}"
    );
    assert_eq!(
        check(&bad),
        field_mismatch("Person", "name", "string", "int", span_of(&bad, "5", 0))
    );
    let missing = format!(
        "{records}assign c as Course {{\n    title: \"Math\"\n    teacher: Person {{\n    }}\n}}"
    );
    assert!(matches!(
        check(&missing),
        Err(TypeError::MissingFields { record, fields, .. })
            if record == "Person" && fields == ["name"]
    ));
    // A record-typed field needs that record, not another or a scalar.
    let scalar = format!("{records}assign c as Course {{\n    title: \"Math\"\n    teacher: 5\n}}");
    assert_eq!(
        check(&scalar),
        field_mismatch(
            "Course",
            "teacher",
            "Person",
            "int",
            span_of(&scalar, "5", 0)
        )
    );
    let other = format!(
        "record Other {{\n    name: string\n}}\n{records}assign c as Course {{\n    title: \"Math\"\n    teacher: Other {{\n        name: \"Sam\"\n    }}\n}}"
    );
    assert!(matches!(
        check(&other),
        Err(TypeError::FieldMismatch { detail, .. })
            if detail.expected == "Person" && detail.actual == "Other"
    ));
    // A record value held in a variable can fill a record-typed field.
    let variable = format!(
        "{records}assign p as Person {{\n    name: \"Sam\"\n}}\nassign c as Course {{\n    title: \"Math\"\n    teacher: p\n}}"
    );
    assert_eq!(check(&variable), accepted());
    // Three levels deep.
    let deep = "record A {\n    b: B\n}\nrecord B {\n    c: C\n}\nrecord C {\n    n: int\n}\nassign a as A {\n    b: B {\n        c: C {\n            n: 1\n        }\n    }\n}";
    assert_eq!(check(deep), accepted());
    let deep_bad = deep.replace("n: 1", "n: true");
    assert!(matches!(
        check(&deep_bad),
        Err(TypeError::FieldMismatch { detail, .. })
            if detail.record == "C" && detail.field == "n" && detail.actual == "bool"
    ));
}

#[test]
fn forward_references_work_for_declarations_and_initializers() {
    // The initializer comes before the declarations it uses.
    let source = "assign c as Course {\n    title: \"Math\"\n    teacher: Person {\n        name: \"Sam\"\n    }\n}\nrecord Course {\n    title: string\n    teacher: Person\n}\nrecord Person {\n    name: string\n}";
    assert_eq!(check(source), accepted());
    // A forward-referenced field type declared after its user.
    let source = "record Course {\n    teacher: Person\n}\nrecord Person {\n    name: string\n}\nassign c as Course {\n    teacher: Person {\n        name: \"Sam\"\n    }\n}";
    assert_eq!(check(source), accepted());
    // Errors in a forward-used initializer are still found.
    let source = "assign p as Person {\n    name: 1\n}\nrecord Person {\n    name: string\n}";
    assert!(matches!(
        check(source),
        Err(TypeError::FieldMismatch { detail, .. }) if detail.record == "Person"
    ));
}

#[test]
fn undefined_records_are_rejected_at_the_name() {
    let source = "assign s as Nope {\n    x: 1\n}";
    assert_eq!(
        check(source),
        Err(TypeError::UndefinedRecord {
            name: "Nope".into(),
            span: span_of(source, "Nope", 0),
        })
    );
    // Names are case-sensitive and a variable is not a record.
    let source = format!("{STUDENT}assign s as student {{\n    name: \"a\"\n    age: 1\n}}");
    assert!(matches!(
        check(&source),
        Err(TypeError::UndefinedRecord { name, span })
            if name == "student" && span == span_of(&source, "student", 0)
    ));
    // The record is looked up before its fields, so field errors do not hide it.
    let source = "assign s as Nope {\n    x: undefined_variable\n}";
    assert!(matches!(
        check(source),
        Err(TypeError::UndefinedRecord { .. })
    ));
    // A record declared only inside a function does not exist for initializers.
    let source = "function f() be int {\n    record P {\n        x: int\n    }\n    return 1\n}\nassign p as P {\n    x: 1\n}";
    assert!(matches!(
        check(source),
        Err(TypeError::RecordNotTopLevel { .. })
    ));
}

#[test]
fn unknown_duplicate_and_missing_fields_are_rejected() {
    let source =
        format!("{STUDENT}assign s as Student {{\n    name: \"a\"\n    age: 1\n    gpa: 4\n}}");
    assert_eq!(
        check(&source),
        Err(TypeError::UnknownField {
            record: "Student".into(),
            field: "gpa".into(),
            span: span_of(&source, "gpa", 0),
        })
    );
    // Field names are case-sensitive.
    let source = format!("{STUDENT}assign s as Student {{\n    Name: \"a\"\n    age: 1\n}}");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnknownField { field, .. }) if field == "Name"
    ));
    let source = format!(
        "{STUDENT}assign s as Student {{\n    name: \"a\"\n    name: \"b\"\n    age: 1\n}}"
    );
    assert_eq!(
        check(&source),
        Err(TypeError::DuplicateInitializer {
            record: "Student".into(),
            field: "name".into(),
            span: span_of(&source, "name", 2),
        })
    );
    // Missing fields are listed in declaration order, whatever order was written.
    let record = "record Big {\n    a: int\n    b: int\n    c: int\n    d: int\n}\n";
    let source = format!("{record}assign v as Big {{\n    d: 1\n    b: 2\n}}");
    assert_eq!(
        check(&source),
        Err(TypeError::MissingFields {
            record: "Big".into(),
            fields: vec!["a".into(), "c".into()],
            span: Span {
                start: source.find("Big {\n    d").unwrap(),
                end: source.len(),
            },
        })
    );
    let source = format!("{record}assign v as Big {{}}");
    assert!(matches!(
        check(&source),
        Err(TypeError::MissingFields { fields, .. }) if fields == ["a", "b", "c", "d"]
    ));
}

#[test]
fn errors_are_reported_in_source_order_within_an_initializer() {
    // An unknown field is found before a later value's type error.
    let source = format!("{STUDENT}assign s as Student {{\n    zzz: 1\n    age: \"x\"\n}}");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnknownField { .. })
    ));
    // A wrong value is found before a missing field.
    let source = format!("{STUDENT}assign s as Student {{\n    name: 5\n}}");
    assert!(matches!(
        check(&source),
        Err(TypeError::FieldMismatch { .. })
    ));
    // A value with its own error is reported as that error.
    let source = format!("{STUDENT}assign s as Student {{\n    name: missing\n    age: 1\n}}");
    assert!(matches!(
        check(&source),
        Err(TypeError::UndefinedVariable { .. })
    ));
}

#[test]
fn initializer_errors_after_a_declaration_are_not_hidden() {
    // The unsupported error is deferred until the whole program has been checked.
    let source = format!("{STUDENT}assign s as Student {{\n    name: 1\n    age: 2\n}}");
    assert!(matches!(
        check(&source),
        Err(TypeError::FieldMismatch { .. })
    ));
    let source = format!("{STUDENT}show missing");
    assert!(matches!(
        check(&source),
        Err(TypeError::UndefinedVariable { .. })
    ));
    // With every statement valid, the whole program is accepted.
    let source = "record A {\n    s: string\n}\nrecord B {\n    s: string\n}\nassign a as A {\n    s: \"x\"\n}";
    assert_eq!(check(source), accepted());
}

#[test]
fn earlier_record_validation_still_comes_first() {
    for (source, matches_expected) in [
        (
            "record A {\n    x: int\n}\nrecord A {\n    y: int\n}\nassign a as A {\n    x: 1\n}",
            "DuplicateRecord",
        ),
        (
            "record A {\n    x: int\n    x: int\n}\nassign a as A {\n    x: 1\n}",
            "DuplicateField",
        ),
        (
            "record A {\n    b: Missing\n}\nassign a as A {\n    b: 1\n}",
            "UndefinedRecordType",
        ),
        (
            "record A {\n    a: A\n}\nassign a as A {\n    a: 1\n}",
            "RecursiveRecord",
        ),
        (
            "record int {\n    x: int\n}\nassign a as int {\n    x: 1\n}",
            "ReservedRecordName",
        ),
        (
            "assign a as A {\n    x: 1\n}\nfunction f() be int {\n    record A {\n        x: int\n    }\n    return 1\n}",
            "RecordNotTopLevel",
        ),
    ] {
        let error = check(source).unwrap_err();
        assert!(
            format!("{error:?}").starts_with(matches_expected),
            "{source:?}: {error:?}"
        );
    }
}

#[test]
fn a_record_is_a_value_of_its_own_type() {
    let source =
        format!("{STUDENT}assign s as Student {{\n    name: \"a\"\n    age: 1\n}}\nassign t as s");
    assert_eq!(check(&source), accepted());
    // Reassigning needs the same record type.
    let source = format!(
        "{STUDENT}assign s as Student {{\n    name: \"a\"\n    age: 1\n}}\nreassign s as 5"
    );
    assert_eq!(
        check(&source),
        Err(TypeError::RecordMismatch {
            expected: "Student".into(),
            actual: "int".into(),
            span: span_of(&source, "5", 0),
        })
    );
    let source = format!(
        "{STUDENT}assign x as 1\nreassign x as Student {{\n    name: \"a\"\n    age: 1\n}}"
    );
    assert!(matches!(
        check(&source),
        Err(TypeError::RecordMismatch { expected, actual, .. })
            if expected == "int" && actual == "Student"
    ));
    let two = "record P {\n    x: int\n}\nrecord Q {\n    x: int\n}\nassign p as P {\n    x: 1\n}\nreassign p as Q {\n    x: 1\n}";
    assert!(matches!(
        check(two),
        Err(TypeError::RecordMismatch { expected, actual, .. })
            if expected == "P" && actual == "Q"
    ));
    // Records take no part in arithmetic or conditions.
    let source =
        format!("{STUDENT}assign s as Student {{\n    name: \"a\"\n    age: 1\n}}\nshow s + 1");
    assert!(matches!(
        check(&source),
        Err(TypeError::InvalidOperands { .. })
    ));
    let source = format!(
        "{STUDENT}assign s as Student {{\n    name: \"a\"\n    age: 1\n}}\nif s {{\n    show 1\n}}"
    );
    assert!(matches!(
        check(&source),
        Err(TypeError::InvalidCondition { .. })
    ));
}

#[test]
fn initializers_inside_functions_and_blocks_are_validated() {
    let record = "record P {\n    x: int\n}\n";
    for body in [
        "function f() be int {\n    assign p as P {\n        x: \"no\"\n    }\n    return 1\n}",
        "if true {\n    assign p as P {\n        x: \"no\"\n    }\n}",
        "while false {\n    assign p as P {\n        x: \"no\"\n    }\n}",
    ] {
        let source = format!("{record}{body}");
        assert!(
            matches!(check(&source), Err(TypeError::FieldMismatch { .. })),
            "{source:?}"
        );
    }
    let ok = format!(
        "{record}function f() be int {{\n    assign p as P {{\n        x: 1\n    }}\n    return 1\n}}"
    );
    assert_eq!(check(&ok), Ok(()));
}

#[test]
fn braces_after_a_condition_do_not_change_programs_without_records() {
    assert_eq!(
        check(
            "assign ready as true\nif ready {\n    show 1\n}\nwhile ready {\n    reassign ready as false\n}"
        ),
        Ok(())
    );
    assert_eq!(
        check("assign a as 1\nassign b as 2\nif a < b {\n    show a\n} else {\n    show b\n}"),
        Ok(())
    );
}

#[test]
fn check_statement_reports_initializers_without_declarations() {
    // `check_statement` is public and skips whole-program record collection.
    let statements = Parser::new(tokenize("assign s as P {\n    x: 1\n}").unwrap())
        .parse_program()
        .unwrap();
    assert!(matches!(
        TypeChecker::new(SymbolTable::new()).check_statement(&statements[0]),
        Err(TypeError::UndefinedRecord { .. })
    ));
}

#[test]
fn long_chains_of_nested_initializers_are_checked() {
    let depth = 40;
    let mut source = String::new();
    for index in 0..depth {
        if index + 1 < depth {
            source += &format!("record R{index} {{\n    next: R{}\n}}\n", index + 1);
        } else {
            source += &format!("record R{index} {{\n    n: int\n}}\n");
        }
    }
    source += "assign v as ";
    for index in 0..depth {
        if index + 1 < depth {
            source += &format!("R{index} {{\n    next: ");
        } else {
            source += &format!("R{index} {{\n    n: 1\n}}");
        }
    }
    for _ in 0..depth - 1 {
        source += "\n}";
    }
    assert_eq!(check(&source), accepted());
    assert!(matches!(
        check(&source.replace("n: 1", "n: \"x\"")),
        Err(TypeError::FieldMismatch { .. })
    ));
}

#[test]
fn diagnostics_render_clearly_and_produce_no_executable() {
    let directory = std::env::temp_dir().join(format!("cheeku record init {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join("program");
    let student = STUDENT;
    for (body, fragments) in [
        (
            "assign s as Nope {\n    x: 1\n}".to_owned(),
            vec![
                "Unknown record 'Nope'",
                "p.chuchu:1:13",
                "no record with this name is declared",
                "help: Declare 'record Nope' at the top level",
            ],
        ),
        (
            format!("{student}assign s as Student {{\n    name: \"a\"\n    age: 1\n    gpa: 4\n}}"),
            vec![
                "Record 'Student' has no field 'gpa'",
                "p.chuchu:8:5",
                "this field is not declared in the record",
                "help: Use one of the fields declared in 'record Student'",
            ],
        ),
        (
            format!(
                "{student}assign s as Student {{\n    name: \"a\"\n    name: \"b\"\n    age: 1\n}}"
            ),
            vec![
                "Field 'name' of record 'Student' is set more than once",
                "p.chuchu:7:5",
                "this field was already given a value",
            ],
        ),
        (
            format!("{student}assign s as Student {{\n    name: \"a\"\n}}"),
            vec![
                "Record 'Student' is missing field 'age'",
                "p.chuchu:5:13",
                "every field must be given a value",
                "help: Add a line such as 'age: <value>'",
            ],
        ),
        (
            "record Big {\n    a: int\n    b: int\n    c: int\n}\nassign v as Big {\n    b: 1\n}"
                .to_owned(),
            vec!["Record 'Big' is missing fields 'a', 'c'"],
        ),
        (
            format!("{student}assign s as Student {{\n    name: \"a\"\n    age: \"old\"\n}}"),
            vec![
                "Field 'age' of record 'Student' expects int, found string",
                "p.chuchu:7:10",
                "this expression has type string",
            ],
        ),
        (
            format!(
                "{student}assign s as Student {{\n    name: \"a\"\n    age: 1\n}}\nreassign s as 5"
            ),
            vec![
                "Type mismatch: expected Student, found int",
                "p.chuchu:9:15",
            ],
        ),
        (
            format!("{student}assign s as Student {{\n    name: \"a\"\n    age: 1\n}}\nshow s"),
            vec!["Records do not support printing a record yet"],
        ),
    ] {
        let error = compile_source(&body, &output).unwrap_err();
        assert!(matches!(error, CompileError::Type(_)), "{body:?}");
        let diagnostic = render(&error, Path::new("p.chuchu"), &body);
        for fragment in fragments {
            assert!(diagnostic.contains(fragment), "{fragment}\n{diagnostic}");
        }
        assert!(!output.exists(), "{body:?}");
    }
    let plain = |source: &str| compile_source(source, &output).unwrap_err().to_string();
    assert!(plain("assign s as Nope {\n    x: 1\n}").starts_with("Undefined record \"Nope\" at "));
    assert!(
        plain(&format!(
            "{STUDENT}assign s as Student {{\n    name: \"a\"\n    age: 1\n    z: 1\n}}"
        ))
        .starts_with("Record \"Student\" has no field \"z\" at ")
    );
    assert!(
        plain(&format!(
            "{STUDENT}assign s as Student {{\n    name: \"a\"\n    name: \"b\"\n    age: 1\n}}"
        ))
        .starts_with("Field \"name\" of record \"Student\" is set more than once at ")
    );
    assert!(
        plain(&format!("{STUDENT}assign s as Student {{\n}}"))
            .starts_with("Record \"Student\" is missing \"name\", \"age\" at ")
    );
    assert!(
        plain(&format!(
            "{STUDENT}assign s as Student {{\n    name: 1\n    age: 1\n}}"
        ))
        .starts_with("Field \"name\" of record \"Student\" expects string, found int at ")
    );
    std::fs::remove_dir_all(&directory).unwrap();
}
