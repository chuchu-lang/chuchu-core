//! Semantic validation of record field access, `object.field` (Issue #11, step 5).
//! A program whose accesses are valid is still reported as unsupported until code
//! generation exists, and no executable is ever produced.
use chuchu::compiler::{CompileError, compile_source};
use chuchu::diagnostic::render;
use chuchu::lexer::tokenize;
use chuchu::parser::Parser;
use chuchu::symbol_table::SymbolTable;
use chuchu::token::Span;
use chuchu::type_checker::{TypeChecker, TypeError};
use chuchu::types::{RecordId, Type};
use std::path::Path;

/// Two records, one nested in the other, and a variable of each.
const PRELUDE: &str = "record Address {\n    city: string\n    zip: int\n}\nrecord Student {\n    name: string\n    age: int\n    gpa: float\n    active: bool\n    address: Address\n}\nassign student as Student {\n    name: \"Ali\"\n    age: 22\n    gpa: 3.5\n    active: true\n    address: Address {\n        city: \"Lahore\"\n        zip: 54000\n    }\n}\nassign home as Address {\n    city: \"Karachi\"\n    zip: 75000\n}\n";

fn parse(source: &str) -> Vec<chuchu::ast::Stmt> {
    Parser::new(tokenize(source).unwrap())
        .parse_program()
        .unwrap_or_else(|error| panic!("{source:?} should parse: {error:?}"))
}

fn check(source: &str) -> Result<(), TypeError> {
    TypeChecker::new(SymbolTable::new()).check_program(&parse(source))
}

/// Checks `PRELUDE` followed by `body`.
fn check_body(body: &str) -> Result<(), TypeError> {
    check(&format!("{PRELUDE}{body}"))
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

/// The error a program whose statements are all valid ends with: the first record
/// declaration, here always `Address`.
fn unsupported() -> Result<(), TypeError> {
    Err(TypeError::UnsupportedRecord {
        name: "Address".into(),
        span: span_of(
            PRELUDE,
            "record Address {\n    city: string\n    zip: int\n}",
            0,
        ),
    })
}

/// A checker that has seen `PRELUDE`, to infer types of single expressions.
fn prelude_checker() -> TypeChecker {
    let mut checker = TypeChecker::new(SymbolTable::new());
    assert_eq!(checker.check_program(&parse(PRELUDE)), unsupported());
    checker
}

fn type_of(checker: &TypeChecker, expression: &str) -> Result<Type, TypeError> {
    let expression = Parser::new(tokenize(expression).unwrap())
        .parse_expression()
        .unwrap_or_else(|error| panic!("{expression:?} should parse: {error:?}"));
    checker.infer_expression(&expression)
}

#[test]
fn each_field_has_its_declared_type() {
    let checker = prelude_checker();
    for (expression, expected) in [
        ("student.name", Type::String),
        ("student.age", Type::Int),
        ("student.gpa", Type::Float),
        ("student.active", Type::Bool),
        ("student.address.city", Type::String),
        ("student.address.zip", Type::Int),
        ("home.city", Type::String),
        ("home.zip", Type::Int),
    ] {
        assert_eq!(type_of(&checker, expression), Ok(expected), "{expression}");
    }
}

#[test]
fn a_record_field_has_that_records_type() {
    let checker = prelude_checker();
    let address = type_of(&checker, "student.address").unwrap();
    assert!(matches!(address, Type::Record(_)));
    // It is the same type as another value of that record, and not Student's.
    assert_eq!(type_of(&checker, "home"), Ok(address));
    assert_ne!(type_of(&checker, "student"), Ok(address));
    assert_eq!(type_of(&checker, "(student).address"), Ok(address));
}

#[test]
fn access_results_take_part_in_expressions() {
    let checker = prelude_checker();
    for (expression, expected) in [
        ("student.age + 1", Type::Int),
        ("student.age * student.address.zip", Type::Int),
        ("-student.age", Type::Int),
        ("student.gpa * 2.0", Type::Float),
        ("student.age < 30", Type::Bool),
        ("student.name == \"Ali\"", Type::Bool),
        ("(student.age + 1) * 2", Type::Int),
    ] {
        assert_eq!(type_of(&checker, expression), Ok(expected), "{expression}");
    }
}

#[test]
fn valid_access_is_still_reported_as_unsupported() {
    for body in [
        "show student.name\nshow student.age + 1\nshow student.address.city",
        "assign n as student.age\nassign c as student.address.city",
        // An int can fill a float variable, as in reassignment.
        "assign f as 1.5\nreassign f as student.age",
        "assign n as 0\nreassign n as student.age",
        // Parentheses, negation, comparison.
        "show (student).age\nshow -student.age\nshow student.age < student.address.zip",
        // A whole nested record can be copied into another variable.
        "assign a as student.address\nshow a.city",
        "assign a as home\nshow a.zip",
        // An initializer can be read directly.
        "show Address {\n    city: \"x\"\n    zip: 1\n}.zip",
    ] {
        assert_eq!(check_body(body), unsupported(), "{body:?}");
    }
}

#[test]
fn access_works_in_every_expression_position() {
    for body in [
        "show student.age",
        "assign n as student.age",
        "assign n as 0\nreassign n as student.age",
        "assign n as student.age + student.address.zip * 2",
        "if student.active {\n    show 1\n} else {\n    show 2\n}",
        "while student.age < 0 {\n    show student.age\n}",
        "if student.age < student.address.zip {\n    show 1\n}",
        "assign xs as [student.age, 3, student.address.zip]",
        "assign xs as [1, 2, 3]\nshow xs[student.age - 20]",
        "function twice(n: int) be int {\n    return n * 2\n}\nshow twice(student.age)",
        "function f(n: int) be int {\n    assign s as Student {\n        name: \"x\"\n        age: n\n        gpa: 1.0\n        active: false\n        address: Address {\n            city: \"c\"\n            zip: 1\n        }\n    }\n    return s.age + s.address.zip\n}\nshow f(1)",
        // As values in other initializers, including a whole record.
        "assign other as Address {\n    city: student.name\n    zip: student.age\n}",
        "assign other as Student {\n    name: \"x\"\n    age: 1\n    gpa: student.gpa\n    active: student.active\n    address: student.address\n}",
        // An int reads into a float field.
        "assign other as Student {\n    name: \"x\"\n    age: 1\n    gpa: student.age\n    active: true\n    address: home\n}",
    ] {
        assert_eq!(check_body(body), unsupported(), "{body:?}");
    }
}

#[test]
fn unknown_fields_are_rejected_at_the_field_name() {
    let source = format!("{PRELUDE}show student.height");
    assert_eq!(
        check(&source),
        Err(TypeError::UnknownField {
            record: "Student".into(),
            field: "height".into(),
            span: span_of(&source, "height", 0),
        })
    );
    // Field names are case-sensitive.
    let source = format!("{PRELUDE}show student.Name");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnknownField { field, .. }) if field == "Name"
    ));
    // Each link of a chain is checked against the record before it.
    let source = format!("{PRELUDE}show student.address.country");
    assert_eq!(
        check(&source),
        Err(TypeError::UnknownField {
            record: "Address".into(),
            field: "country".into(),
            span: span_of(&source, "country", 0),
        })
    );
    // A field of the inner record is not a field of the outer one.
    let source = format!("{PRELUDE}show student.city");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnknownField { record, field, .. }) if record == "Student" && field == "city"
    ));
    let source = format!("{PRELUDE}show home.age");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnknownField { record, field, .. }) if record == "Address" && field == "age"
    ));
    // The same error for an access on an initializer.
    let source = format!("{PRELUDE}show Address {{\n    city: \"x\"\n    zip: 1\n}}.nope");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnknownField { field, .. }) if field == "nope"
    ));
}

#[test]
fn the_span_of_an_unknown_field_is_exact_with_spaces_around_the_dot() {
    let source = format!("{PRELUDE}show student . height");
    assert_eq!(
        check(&source),
        Err(TypeError::UnknownField {
            record: "Student".into(),
            field: "height".into(),
            span: span_of(&source, "height", 0),
        })
    );
}

#[test]
fn access_on_a_non_record_is_rejected_at_the_object() {
    for (body, object, label) in [
        ("assign x as 5\nshow x.y", "x", "int"),
        ("assign f as 1.5\nshow f.y", "f", "float"),
        ("assign b as true\nshow b.y", "b", "bool"),
        ("assign s as \"text\"\nshow s.length", "s", "string"),
        ("assign xs as [1, 2, 3]\nshow xs.len", "xs", "int[3]"),
        ("assign xs as [1.5, 2.5]\nshow xs.len", "xs", "float[2]"),
        // Scalar fields are not records either.
        ("show student.name.length", "student.name", "string"),
        ("show student.age.value", "student.age", "int"),
        (
            "show student.address.zip.digits",
            "student.address.zip",
            "int",
        ),
        // Results of expressions.
        ("show (1 + 2).x", "(1 + 2)", "int"),
        ("show (student.age + 1).x", "(student.age + 1)", "int"),
        ("assign xs as [1, 2]\nshow xs[0].x", "xs[0]", "int"),
        ("assign r as err<int>(1)\nshow r.code", "r", "result<int>"),
    ] {
        let source = format!("{PRELUDE}{body}");
        // The object is the last occurrence in the source, which is the access.
        let occurrences = source.matches(object).count();
        let error = check(&source).unwrap_err();
        assert_eq!(
            error,
            TypeError::FieldAccessOnNonRecord {
                field: body.rsplit('.').next().unwrap().into(),
                actual: label.into(),
                span: span_of(&source, object, occurrences - 1),
            },
            "{body:?}"
        );
    }
}

#[test]
fn a_function_result_has_no_fields_because_functions_cannot_return_records() {
    // Function signatures only name built-in types, so `f().x` can never read a
    // field today; the error is the same as for any other non-record value.
    let source = format!("{PRELUDE}function f() be int {{\n    return 1\n}}\nshow f().name");
    assert_eq!(
        check(&source),
        Err(TypeError::FieldAccessOnNonRecord {
            field: "name".into(),
            actual: "int".into(),
            span: span_of(&source, "f()", 1),
        })
    );
    assert!(
        chuchu::parser::Parser::new(
            tokenize("function f() be Student {\n    return 1\n}").unwrap()
        )
        .parse_program()
        .is_err()
    );
}

#[test]
fn the_object_is_checked_before_the_field() {
    // An undefined object, a bad index, or a failing call is reported as itself.
    for (body, expected) in [
        ("show nope.x", "UndefinedVariable"),
        ("show nope.x.y", "UndefinedVariable"),
        ("show missing().x", "InvalidFunction"),
        ("assign xs as [1]\nshow xs[\"a\"].x", "InvalidArray"),
    ] {
        let error = check_body(body).unwrap_err();
        assert!(
            format!("{error:?}").starts_with(expected),
            "{body:?}: {error:?}"
        );
    }
}

#[test]
fn access_results_are_checked_against_what_uses_them() {
    // Reassigning needs the same type.
    let source = format!("{PRELUDE}assign n as 1\nreassign n as student.name");
    assert_eq!(
        check(&source),
        Err(TypeError::Mismatch {
            expected: Type::Int,
            actual: Type::String,
            span: span_of(&source, "student.name", 0),
        })
    );
    // A float field cannot fill an int variable.
    assert!(matches!(
        check_body("assign n as 1\nreassign n as student.gpa"),
        Err(TypeError::Mismatch {
            expected: Type::Int,
            actual: Type::Float,
            ..
        })
    ));
    // A record field that is itself a record keeps its record type.
    let source = format!("{PRELUDE}reassign student as student.address");
    assert_eq!(
        check(&source),
        Err(TypeError::RecordMismatch {
            expected: "Student".into(),
            actual: "Address".into(),
            span: span_of(&source, "student.address", 0),
        })
    );
    // Operators, conditions, and initializer fields.
    assert!(matches!(
        check_body("show student.name + 1"),
        Err(TypeError::InvalidOperands { .. })
    ));
    assert!(matches!(
        check_body("if student.name {\n    show 1\n}"),
        Err(TypeError::InvalidCondition { .. })
    ));
    assert!(matches!(
        check_body("while student.age {\n    show 1\n}"),
        Err(TypeError::InvalidCondition { .. })
    ));
    let source =
        format!("{PRELUDE}assign other as Address {{\n    city: \"x\"\n    zip: student.name\n}}");
    assert!(matches!(
        check(&source),
        Err(TypeError::FieldMismatch { detail, span })
            if detail.record == "Address"
                && detail.field == "zip"
                && detail.expected == "int"
                && detail.actual == "string"
                && span == span_of(&source, "student.name", 0)
    ));
    let source = format!(
        "{PRELUDE}assign other as Student {{\n    name: \"x\"\n    age: 1\n    gpa: 1.0\n    active: true\n    address: student.name\n}}"
    );
    assert!(matches!(
        check(&source),
        Err(TypeError::FieldMismatch { detail, .. })
            if detail.expected == "Address" && detail.actual == "string"
    ));
    assert!(matches!(
        check_body(
            "function twice(n: int) be int {\n    return n * 2\n}\nshow twice(student.name)"
        ),
        Err(TypeError::InvalidFunction { .. } | TypeError::Mismatch { .. })
    ));
    assert!(matches!(
        check_body("function f() be int {\n    return student.name\n}"),
        Err(TypeError::UndefinedVariable { .. })
    ));
}

#[test]
fn access_errors_come_in_source_order_with_the_rest_of_the_program() {
    // The first problem wins, whether it is an access or something else.
    let source = format!("{PRELUDE}show student.nope\nshow missing");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnknownField { .. })
    ));
    let source = format!("{PRELUDE}show missing\nshow student.nope");
    assert!(matches!(
        check(&source),
        Err(TypeError::UndefinedVariable { .. })
    ));
    // A bad access after the declarations is reported instead of the deferred
    // unsupported error, and a good one is not.
    assert!(matches!(
        check_body("show student.nope"),
        Err(TypeError::UnknownField { .. })
    ));
    assert_eq!(check_body("show student.name"), unsupported());
}

#[test]
fn earlier_record_validation_still_applies_with_access_present() {
    for (source, expected) in [
        (
            "record A {\n    x: int\n}\nrecord A {\n    y: int\n}\nshow 1\nassign a as A {\n    x: 1\n}\nshow a.x",
            "DuplicateRecord",
        ),
        (
            "record A {\n    x: int\n    x: int\n}\nassign a as A {\n    x: 1\n}\nshow a.x",
            "DuplicateField",
        ),
        (
            "record A {\n    b: Missing\n}\nassign a as A {\n    b: 1\n}\nshow a.b",
            "UndefinedRecordType",
        ),
        (
            "record A {\n    a: A\n}\nassign a as A {\n    a: 1\n}\nshow a.a.a",
            "RecursiveRecord",
        ),
        (
            "record A {\n    b: B\n}\nrecord B {\n    a: A\n}\nshow 1",
            "RecursiveRecord",
        ),
        ("record int {\n    x: int\n}\nshow 1", "ReservedRecordName"),
        (
            "function f() be int {\n    record A {\n        x: int\n    }\n    return 1\n}\nshow 1",
            "RecordNotTopLevel",
        ),
        // Initializer validation from step 4 is unchanged.
        (
            "record A {\n    x: int\n}\nassign a as A {\n    y: 1\n}\nshow a.x",
            "UnknownField",
        ),
        (
            "record A {\n    x: int\n}\nassign a as A {\n}\nshow a.x",
            "MissingFields",
        ),
        (
            "record A {\n    x: int\n}\nassign a as A {\n    x: \"no\"\n}\nshow a.x",
            "FieldMismatch",
        ),
        (
            "assign a as Nope {\n    x: 1\n}\nshow a.x",
            "UndefinedRecord",
        ),
    ] {
        let error = check(source).unwrap_err();
        assert!(
            format!("{error:?}").starts_with(expected),
            "{source:?}: {error:?}"
        );
    }
}

#[test]
fn a_record_that_is_never_accessed_is_unaffected() {
    assert_eq!(check_body("show 1"), unsupported());
    assert_eq!(
        check("record A {\n    x: int\n}"),
        Err(TypeError::UnsupportedRecord {
            name: "A".into(),
            span: Span { start: 0, end: 23 },
        })
    );
    // Programs without records behave as before, including floats with dots.
    assert_eq!(
        check("assign a as 1.5\nassign b as 2.\nshow a + b * 3.25e1"),
        Ok(())
    );
}

#[test]
fn check_statement_and_infer_expression_work_without_a_whole_program() {
    // `check_statement` skips record collection, so no record can be found.
    let statements = parse("show s.x");
    assert!(matches!(
        TypeChecker::new(SymbolTable::new()).check_statement(&statements[0]),
        Err(TypeError::UndefinedVariable { .. })
    ));
    // A record type that this checker never declared is reported, not a panic.
    let mut symbols = SymbolTable::new();
    symbols
        .declare("s".into(), Type::Record(RecordId(7)))
        .unwrap();
    let checker = TypeChecker::new(symbols);
    assert_eq!(
        type_of(&checker, "s.x"),
        Err(TypeError::FieldAccessOnNonRecord {
            field: "x".into(),
            actual: "record".into(),
            span: Span { start: 0, end: 1 },
        })
    );
}

#[test]
fn long_chains_of_access_are_checked() {
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
    let path = "v".to_owned() + &".next".repeat(depth - 1);
    let valid = format!("{source}\nshow {path}.n");
    assert!(matches!(
        check(&valid),
        Err(TypeError::UnsupportedRecord { name, .. }) if name == "R0"
    ));
    let invalid = format!("{source}\nshow {path}.n.n");
    assert!(matches!(
        check(&invalid),
        Err(TypeError::FieldAccessOnNonRecord { actual, .. }) if actual == "int"
    ));
    let invalid = format!("{source}\nshow {path}.next");
    assert!(matches!(
        check(&invalid),
        Err(TypeError::UnknownField { record, .. }) if record == format!("R{}", depth - 1)
    ));
}

#[test]
fn diagnostics_render_clearly_and_produce_no_executable() {
    let directory =
        std::env::temp_dir().join(format!("cheeku record access {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join("program");
    let line = |text: &str| PRELUDE.lines().count() + text.lines().count();
    for (body, fragments) in [
        (
            "show student.height",
            vec![
                "Record 'Student' has no field 'height'".to_owned(),
                format!("p.chuchu:{}:14", line("x")),
                "this field is not declared in the record".to_owned(),
                "help: Use one of the fields declared in 'record Student'".to_owned(),
            ],
        ),
        (
            "show student.address.zap",
            vec![
                "Record 'Address' has no field 'zap'".to_owned(),
                format!("p.chuchu:{}:22", line("x")),
            ],
        ),
        (
            "show student.name.length",
            vec![
                "Cannot read field 'length' of a value of type string".to_owned(),
                format!("p.chuchu:{}:6", line("x")),
                "this has type string, not a record".to_owned(),
                "help: Only record values have fields".to_owned(),
            ],
        ),
        (
            "assign xs as [1, 2]\nshow xs.len",
            vec![
                "Cannot read field 'len' of a value of type int[2]".to_owned(),
                "this has type int[2], not a record".to_owned(),
            ],
        ),
        (
            "show student.",
            vec![
                "Expected identifier".to_owned(),
                format!("p.chuchu:{}:14", line("x")),
            ],
        ),
        (
            "show student.age",
            vec!["Record 'Address' cannot be compiled yet".to_owned()],
        ),
    ] {
        let source = format!("{PRELUDE}{body}");
        let error = compile_source(&source, &output).unwrap_err();
        let diagnostic = render(&error, Path::new("p.chuchu"), &source);
        for fragment in fragments {
            assert!(diagnostic.contains(&fragment), "{fragment}\n{diagnostic}");
        }
        assert!(!output.exists(), "{body:?}");
    }
    let plain = |body: &str| {
        compile_source(&format!("{PRELUDE}{body}"), &output)
            .unwrap_err()
            .to_string()
    };
    assert!(
        plain("show student.height").starts_with("Record \"Student\" has no field \"height\" at "),
        "unknown"
    );
    assert!(
        plain("show student.name.length")
            .starts_with("Cannot read field \"length\" of a value of type string at "),
        "non-record"
    );
    assert!(matches!(
        compile_source(&format!("{PRELUDE}show student.name.length"), &output),
        Err(CompileError::Type(TypeError::FieldAccessOnNonRecord { .. }))
    ));
    std::fs::remove_dir_all(&directory).unwrap();
}
