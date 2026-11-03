//! Semantic validation of record field mutation, `reassign variable.field as value`
//! (Issue #11, step 6). A program whose mutations are valid is still reported as
//! unsupported until code generation exists, and no executable is ever produced.
use chuchu::compiler::{CompileError, compile_source};
use chuchu::diagnostic::render;
use chuchu::lexer::tokenize;
use chuchu::parser::{ParseError, Parser};
use chuchu::symbol_table::SymbolTable;
use chuchu::token::Span;
use chuchu::type_checker::{FieldMismatch, TypeChecker, TypeError};
use chuchu::types::{RecordId, Type};
use std::path::Path;

/// Two records, one nested in the other, and some variables.
const PRELUDE: &str = "record Address {\n    city: string\n    zip: int\n}\nrecord Student {\n    name: string\n    age: int\n    gpa: float\n    active: bool\n    address: Address\n}\nassign student as Student {\n    name: \"Ali\"\n    age: 22\n    gpa: 3.5\n    active: true\n    address: Address {\n        city: \"Lahore\"\n        zip: 54000\n    }\n}\nassign home as Address {\n    city: \"Karachi\"\n    zip: 75000\n}\nassign n as 1\nassign xs as [1, 2, 3]\nfunction f() be int {\n    return 1\n}\n";

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

/// The span of the last match of `text`, which is the one in the appended body.
fn last_span_of(source: &str, text: &str) -> Span {
    span_of(source, text, source.matches(text).count() - 1)
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

#[test]
fn valid_mutations_are_still_reported_as_unsupported() {
    for body in [
        "reassign student.age as 25",
        "reassign student.name as \"Sara\"",
        "reassign student.active as false",
        "reassign student.gpa as 3.9",
        // Nested fields.
        "reassign student.address.city as \"Lahore\"",
        "reassign student.address.zip as 1",
        // An int fills a float field; a float never fills an int field.
        "reassign student.gpa as 4",
        "reassign student.gpa as student.age",
        // The value can read the same record.
        "reassign student.age as student.age + 1",
        "reassign student.address.zip as student.address.zip * 2",
        "reassign student.age as student.address.zip",
        // Whole records, by variable, field, or temporary value.
        "reassign student.address as home",
        "reassign student.address as student.address",
        "reassign student.address as Address {\n    city: \"x\"\n    zip: 1\n}",
        // Several mutations in a row, and mutation followed by reads.
        "reassign student.age as 1\nreassign student.age as 2\nshow student.age",
        "reassign home.zip as 5\nshow home.zip + 1",
        // Values from calls, arrays, and other variables.
        "reassign student.age as n",
        "reassign student.age as f()",
        "reassign student.age as xs[0] + 1",
    ] {
        assert_eq!(check_body(body), unsupported(), "{body:?}");
    }
}

#[test]
fn mutation_works_inside_blocks_and_functions() {
    for body in [
        "if student.active {\n    reassign student.age as 1\n} else {\n    reassign student.age as 2\n}",
        "while student.age < 30 {\n    reassign student.age as student.age + 1\n}",
        "if true {\n    if true {\n        reassign student.address.zip as 0\n    }\n}",
        // A record created in the function can be mutated there.
        "function g(k: int) be int {\n    assign a as Address {\n        city: \"c\"\n        zip: k\n    }\n    reassign a.zip as k + 1\n    return a.zip\n}\nshow g(1)",
        // A record created in a block is visible in nested blocks.
        "if true {\n    assign a as Address {\n        city: \"c\"\n        zip: 1\n    }\n    while a.zip < 3 {\n        reassign a.zip as a.zip + 1\n    }\n}",
    ] {
        assert_eq!(check_body(body), unsupported(), "{body:?}");
    }
}

#[test]
fn a_mutation_does_not_change_the_variables_type() {
    let mut checker = TypeChecker::new(SymbolTable::new());
    let source = format!(
        "{PRELUDE}reassign student.age as 1\nreassign student.address.city as \"x\"\nreassign student.gpa as 2"
    );
    assert_eq!(checker.check_program(&parse(&source)), unsupported());
    let type_of = |expression: &str| {
        let expression = Parser::new(tokenize(expression).unwrap())
            .parse_expression()
            .unwrap();
        checker.infer_expression(&expression)
    };
    assert_eq!(type_of("student.age"), Ok(Type::Int));
    assert_eq!(type_of("student.gpa"), Ok(Type::Float));
    assert_eq!(type_of("student.address.city"), Ok(Type::String));
    assert!(matches!(type_of("student"), Ok(Type::Record(_))));
    assert_eq!(type_of("student.address"), type_of("home"));
}

#[test]
fn undeclared_variables_are_rejected_at_the_variable() {
    let source = format!("{PRELUDE}reassign nobody.age as 1");
    assert_eq!(
        check(&source),
        Err(TypeError::UndefinedVariable {
            name: "nobody".into(),
            span: span_of(&source, "nobody", 0),
        })
    );
    // Also deeper in a chain, and when the value is fine or broken.
    let source = format!("{PRELUDE}reassign nobody.a.b as 1");
    assert!(matches!(
        check(&source),
        Err(TypeError::UndefinedVariable { name, span })
            if name == "nobody" && span == span_of(&source, "nobody", 0)
    ));
    // A variable that went out of scope is undeclared again.
    let body = "if true {\n    assign a as Address {\n        city: \"c\"\n        zip: 1\n    }\n}\nreassign a.zip as 2";
    let source = format!("{PRELUDE}{body}");
    assert!(matches!(
        check(&source),
        Err(TypeError::UndefinedVariable { name, .. }) if name == "a"
    ));
    // A record declared only as a type is not a variable.
    assert!(matches!(
        check_body("reassign Student.age as 1"),
        Err(TypeError::UndefinedVariable { name, .. }) if name == "Student"
    ));
    // Functions do not see the program's variables.
    assert!(matches!(
        check_body("function g() be int {\n    reassign student.age as 1\n    return 1\n}"),
        Err(TypeError::UndefinedVariable { name, .. }) if name == "student"
    ));
}

#[test]
fn unknown_fields_are_rejected_at_the_field_name() {
    let source = format!("{PRELUDE}reassign student.height as 180");
    assert_eq!(
        check(&source),
        Err(TypeError::UnknownField {
            record: "Student".into(),
            field: "height".into(),
            span: span_of(&source, "height", 0),
        })
    );
    // Each link is checked against the record before it.
    let source = format!("{PRELUDE}reassign student.address.country as \"x\"");
    assert_eq!(
        check(&source),
        Err(TypeError::UnknownField {
            record: "Address".into(),
            field: "country".into(),
            span: span_of(&source, "country", 0),
        })
    );
    // Names are case-sensitive, and an inner field is not an outer one.
    let source = format!("{PRELUDE}reassign student.Age as 1");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnknownField { field, .. }) if field == "Age"
    ));
    assert!(matches!(
        check_body("reassign student.city as \"x\""),
        Err(TypeError::UnknownField { record, field, .. })
            if record == "Student" && field == "city"
    ));
    assert!(matches!(
        check_body("reassign home.age as 1"),
        Err(TypeError::UnknownField { record, field, .. })
            if record == "Address" && field == "age"
    ));
    // The span is exact with spaces around the dots.
    let source = format!("{PRELUDE}reassign student . address . country as \"x\"");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnknownField { span, .. }) if span == span_of(&source, "country", 0)
    ));
}

#[test]
fn the_value_must_have_the_fields_declared_type() {
    let source = format!("{PRELUDE}reassign student.age as \"old\"");
    assert_eq!(
        check(&source),
        field_mismatch(
            "Student",
            "age",
            "int",
            "string",
            last_span_of(&source, "\"old\"")
        )
    );
    let source = format!("{PRELUDE}reassign student.name as 5");
    assert_eq!(
        check(&source),
        field_mismatch(
            "Student",
            "name",
            "string",
            "int",
            last_span_of(&source, "5")
        )
    );
    let source = format!("{PRELUDE}reassign student.active as 1");
    assert_eq!(
        check(&source),
        field_mismatch(
            "Student",
            "active",
            "bool",
            "int",
            last_span_of(&source, "1")
        )
    );
    // A nested field names the record that declares it.
    let source = format!("{PRELUDE}reassign student.address.zip as \"54000\"");
    assert_eq!(
        check(&source),
        field_mismatch(
            "Address",
            "zip",
            "int",
            "string",
            last_span_of(&source, "\"54000\"")
        )
    );
    // The whole value is what is reported, not just its first token.
    let source = format!("{PRELUDE}reassign student.age as student.name + \"x\"");
    assert_eq!(
        check(&source),
        field_mismatch(
            "Student",
            "age",
            "int",
            "string",
            last_span_of(&source, "student.name + \"x\"")
        )
    );
    let source = format!("{PRELUDE}reassign student.age as student.name");
    assert_eq!(
        check(&source),
        field_mismatch(
            "Student",
            "age",
            "int",
            "string",
            last_span_of(&source, "student.name")
        )
    );
}

#[test]
fn numeric_compatibility_matches_reassignment() {
    // int widens to float...
    assert_eq!(check_body("reassign student.gpa as 4"), unsupported());
    assert_eq!(check_body("reassign student.gpa as n"), unsupported());
    // ...but float never narrows to int, and neither is a bool or string.
    let source = format!("{PRELUDE}reassign student.age as 2.5");
    assert_eq!(
        check(&source),
        field_mismatch(
            "Student",
            "age",
            "int",
            "float",
            last_span_of(&source, "2.5")
        )
    );
    assert!(matches!(
        check_body("reassign student.age as student.gpa"),
        Err(TypeError::FieldMismatch { detail, .. })
            if detail.expected == "int" && detail.actual == "float"
    ));
    assert!(matches!(
        check_body("reassign student.gpa as true"),
        Err(TypeError::FieldMismatch { detail, .. })
            if detail.expected == "float" && detail.actual == "bool"
    ));
    // The same rules as a plain variable.
    assert!(matches!(
        check_body("assign g as 1.5\nreassign g as 2\nreassign n as 2.5"),
        Err(TypeError::Mismatch {
            expected: Type::Int,
            actual: Type::Float,
            ..
        })
    ));
}

#[test]
fn record_valued_fields_need_that_record() {
    // The right record is accepted, and another record or a scalar is not.
    let source = format!("{PRELUDE}reassign student.address as student");
    assert_eq!(
        check(&source),
        field_mismatch(
            "Student",
            "address",
            "Address",
            "Student",
            last_span_of(&source, "student")
        )
    );
    assert!(matches!(
        check_body("reassign student.address as 5"),
        Err(TypeError::FieldMismatch { detail, .. })
            if detail.expected == "Address" && detail.actual == "int"
    ));
    assert!(matches!(
        check_body("reassign student.age as home"),
        Err(TypeError::FieldMismatch { detail, .. })
            if detail.expected == "int" && detail.actual == "Address"
    ));
    // A temporary record is a fine value, and is validated like any initializer.
    assert!(matches!(
        check_body("reassign student.address as Address {\n    city: \"x\"\n}"),
        Err(TypeError::MissingFields { record, fields, .. })
            if record == "Address" && fields == ["zip"]
    ));
    assert!(matches!(
        check_body("reassign student.address as Address {\n    city: 1\n    zip: 1\n}"),
        Err(TypeError::FieldMismatch { detail, .. }) if detail.record == "Address"
    ));
    assert!(matches!(
        check_body("reassign student.address as Nope {\n    x: 1\n}"),
        Err(TypeError::UndefinedRecord { .. })
    ));
}

#[test]
fn assigning_a_field_of_a_non_record_is_rejected_at_the_object() {
    for (body, object, field, label) in [
        ("reassign n.x as 1", "n", "x", "int"),
        ("reassign xs.len as 1", "xs", "len", "int[3]"),
        // A scalar field is not a record either.
        (
            "reassign student.name.first as \"a\"",
            "student.name",
            "first",
            "string",
        ),
        (
            "reassign student.age.digits as 1",
            "student.age",
            "digits",
            "int",
        ),
        (
            "reassign student.address.zip.low as 1",
            "student.address.zip",
            "low",
            "int",
        ),
    ] {
        let source = format!("{PRELUDE}{body}");
        assert_eq!(
            check(&source),
            Err(TypeError::FieldAssignOnNonRecord {
                field: field.into(),
                actual: label.into(),
                span: span_of(&source, object, source.matches(object).count() - 1),
            }),
            "{body:?}"
        );
    }
}

#[test]
fn the_target_is_checked_before_the_value() {
    // Source order: variable, then each field, then the value.
    let cases = [
        ("reassign nobody.x as missing", "UndefinedVariable"),
        ("reassign student.nope as missing", "UnknownField"),
        ("reassign n.x as missing", "FieldAssignOnNonRecord"),
        // Once the target is valid, the value's own errors come through unchanged.
        ("reassign student.age as missing", "UndefinedVariable"),
        ("reassign student.age as student.nope", "UnknownField"),
        ("reassign student.age as xs[\"a\"]", "InvalidArray"),
        ("reassign student.age as f(1)", "InvalidFunction"),
        ("reassign student.age as \"a\" + 1", "InvalidOperands"),
    ];
    for (body, expected) in cases {
        let error = check_body(body).unwrap_err();
        assert!(
            format!("{error:?}").starts_with(expected),
            "{body:?}: {error:?}"
        );
    }
}

#[test]
fn errors_come_in_source_order_with_the_rest_of_the_program() {
    // The first problem wins, and a bad mutation hides the deferred unsupported error.
    let source = format!("{PRELUDE}reassign student.nope as 1\nshow missing");
    assert!(matches!(
        check(&source),
        Err(TypeError::UnknownField { .. })
    ));
    let source = format!("{PRELUDE}show missing\nreassign student.nope as 1");
    assert!(matches!(
        check(&source),
        Err(TypeError::UndefinedVariable { .. })
    ));
    assert!(matches!(
        check_body("reassign student.age as 1\nreassign student.nope as 2"),
        Err(TypeError::UnknownField { .. })
    ));
    assert_eq!(
        check_body("reassign student.age as 1\nreassign student.age as 2"),
        unsupported()
    );
}

#[test]
fn plain_reassignment_behaves_as_before() {
    assert_eq!(check_body("reassign n as 2"), unsupported());
    // An unknown variable is still reported at the whole statement.
    let source = format!("{PRELUDE}reassign nobody as 1");
    assert_eq!(
        check(&source),
        Err(TypeError::UndefinedVariable {
            name: "nobody".into(),
            span: span_of(&source, "reassign nobody as 1", 0),
        })
    );
    assert!(matches!(
        check_body("reassign n as \"s\""),
        Err(TypeError::Mismatch {
            expected: Type::Int,
            actual: Type::String,
            ..
        })
    ));
    // A whole record variable still needs a value of its own record type.
    let source = format!("{PRELUDE}reassign student as 5");
    assert_eq!(
        check(&source),
        Err(TypeError::RecordMismatch {
            expected: "Student".into(),
            actual: "int".into(),
            span: last_span_of(&source, "5"),
        })
    );
    assert_eq!(check_body("reassign student as student"), unsupported());
    assert_eq!(check("assign a as 1\nreassign a as 2\nshow a"), Ok(()));
}

#[test]
fn earlier_record_validation_still_applies_with_mutation_present() {
    for (source, expected) in [
        (
            "record A {\n    x: int\n}\nrecord A {\n    y: int\n}\nassign a as A {\n    x: 1\n}\nreassign a.x as 2",
            "DuplicateRecord",
        ),
        (
            "record A {\n    x: int\n    x: int\n}\nassign a as A {\n    x: 1\n}\nreassign a.x as 2",
            "DuplicateField",
        ),
        (
            "record A {\n    b: Missing\n}\nassign a as A {\n    b: 1\n}\nreassign a.b as 2",
            "UndefinedRecordType",
        ),
        (
            "record A {\n    a: A\n}\nassign a as A {\n    a: 1\n}\nreassign a.a as 2",
            "RecursiveRecord",
        ),
        ("record int {\n    x: int\n}\nshow 1", "ReservedRecordName"),
        (
            "function f() be int {\n    record A {\n        x: int\n    }\n    return 1\n}\nshow 1",
            "RecordNotTopLevel",
        ),
        // Initializer and access validation are unchanged.
        (
            "record A {\n    x: int\n}\nassign a as A {\n    y: 1\n}\nreassign a.x as 2",
            "UnknownField",
        ),
        (
            "record A {\n    x: int\n}\nassign a as A {\n}\nreassign a.x as 2",
            "MissingFields",
        ),
        (
            "record A {\n    x: int\n}\nassign a as A {\n    x: 1\n}\nshow a.nope\nreassign a.x as 2",
            "UnknownField",
        ),
        (
            "record A {\n    x: int\n}\nassign a as A {\n    x: 1\n}\nshow a.x.y",
            "FieldAccessOnNonRecord",
        ),
    ] {
        let error = check(source).unwrap_err();
        assert!(
            format!("{error:?}").starts_with(expected),
            "{source:?}: {error:?}"
        );
    }
    // Declarations alone, and reading fields, still end as unsupported.
    assert!(matches!(
        check("record A {\n    x: int\n}"),
        Err(TypeError::UnsupportedRecord { .. })
    ));
    assert_eq!(check_body("show student.name"), unsupported());
}

#[test]
fn check_statement_works_without_whole_program_collection() {
    // No variable and no record are known, so the variable is the first problem.
    let statements = parse("reassign s.x as 1");
    assert!(matches!(
        TypeChecker::new(SymbolTable::new()).check_statement(&statements[0]),
        Err(TypeError::UndefinedVariable { .. })
    ));
    // A record type this checker never declared is reported, not a panic.
    let mut symbols = SymbolTable::new();
    symbols
        .declare("s".into(), Type::Record(RecordId(7)))
        .unwrap();
    let mut checker = TypeChecker::new(symbols);
    assert_eq!(
        checker.check_statement(&statements[0]),
        Err(TypeError::FieldAssignOnNonRecord {
            field: "x".into(),
            actual: "record".into(),
            span: Span { start: 9, end: 10 },
        })
    );
}

#[test]
fn long_chains_of_nested_mutation_are_checked() {
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
    assert!(matches!(
        check(&format!("{source}\nreassign {path}.n as 2")),
        Err(TypeError::UnsupportedRecord { name, .. }) if name == "R0"
    ));
    assert!(matches!(
        check(&format!("{source}\nreassign {path}.n as \"x\"")),
        Err(TypeError::FieldMismatch { detail, .. })
            if detail.record == format!("R{}", depth - 1) && detail.field == "n"
    ));
    assert!(matches!(
        check(&format!("{source}\nreassign {path}.n.n as 2")),
        Err(TypeError::FieldAssignOnNonRecord { actual, .. }) if actual == "int"
    ));
    assert!(matches!(
        check(&format!("{source}\nreassign {path}.nope as 2")),
        Err(TypeError::UnknownField { .. })
    ));
}

#[test]
fn invalid_targets_are_parse_errors_with_exact_spans() {
    let parse_error = |body: &str| {
        let source = format!("{PRELUDE}{body}");
        let error = compile_source(&source, Path::new("unused")).unwrap_err();
        match error {
            CompileError::Parse(error) => (source, error),
            other => panic!("{body:?}: expected a parse error, found {other:?}"),
        }
    };
    for (body, bad) in [
        ("reassign f().age as 1", "f()"),
        ("reassign f().address.zip as 1", "f()"),
        ("reassign xs[0] as 1", "xs[0]"),
        ("reassign student.name[0] as \"a\"", "student.name[0]"),
        (
            "reassign Student {\n    name: \"a\"\n}.name as \"b\"",
            "Student {\n    name: \"a\"\n}",
        ),
    ] {
        let (source, error) = parse_error(body);
        assert_eq!(
            error,
            ParseError::InvalidAssignmentTarget {
                span: last_span_of(&source, bad)
            },
            "{body:?}"
        );
    }
}

#[test]
fn diagnostics_render_clearly_and_produce_no_executable() {
    let directory =
        std::env::temp_dir().join(format!("cheeku record mutation {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join("program");
    // The body's first line is the line after the prelude.
    let line = PRELUDE.lines().count() + 1;
    for (body, fragments) in [
        (
            "reassign nobody.age as 1",
            vec![
                "Undefined variable 'nobody'".to_owned(),
                format!("p.chuchu:{line}:10"),
                "not declared in this scope".to_owned(),
            ],
        ),
        (
            "reassign student.height as 1",
            vec![
                "Record 'Student' has no field 'height'".to_owned(),
                format!("p.chuchu:{line}:18"),
                "this field is not declared in the record".to_owned(),
                "help: Use one of the fields declared in 'record Student'".to_owned(),
            ],
        ),
        (
            "reassign student.address.country as \"x\"",
            vec![
                "Record 'Address' has no field 'country'".to_owned(),
                format!("p.chuchu:{line}:26"),
            ],
        ),
        (
            "reassign student.age as \"old\"",
            vec![
                "Field 'age' of record 'Student' expects int, found string".to_owned(),
                format!("p.chuchu:{line}:25"),
                "this expression has type string".to_owned(),
            ],
        ),
        (
            "reassign student.address.zip as 2.5",
            vec![
                "Field 'zip' of record 'Address' expects int, found float".to_owned(),
                format!("p.chuchu:{line}:33"),
            ],
        ),
        (
            "reassign student.name.first as \"a\"",
            vec![
                "Cannot assign field 'first' of a value of type string".to_owned(),
                format!("p.chuchu:{line}:10"),
                "this has type string, not a record".to_owned(),
                "help: Only record values have fields".to_owned(),
            ],
        ),
        (
            "reassign f().age as 1",
            vec![
                "Invalid assignment target".to_owned(),
                format!("p.chuchu:{line}:10"),
                "only a variable, or a field of a variable, can be reassigned".to_owned(),
                "help: Assign to a variable or its field, such as 'reassign student.age as 25'"
                    .to_owned(),
            ],
        ),
        (
            "reassign xs[0] as 1",
            vec!["Invalid assignment target".to_owned()],
        ),
        (
            "reassign student.age as 25",
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
        plain("reassign f().age as 1").starts_with("Invalid assignment target at "),
        "target"
    );
    assert!(
        plain("reassign student.age as \"x\"")
            .starts_with("Field \"age\" of record \"Student\" expects int, found string at "),
        "mismatch"
    );
    assert!(
        plain("reassign n.x as 1")
            .starts_with("Cannot assign field \"x\" of a value of type int at "),
        "non-record"
    );
    assert!(matches!(
        compile_source(&format!("{PRELUDE}reassign n.x as 1"), &output),
        Err(CompileError::Type(TypeError::FieldAssignOnNonRecord { .. }))
    ));
    std::fs::remove_dir_all(&directory).unwrap();
}
