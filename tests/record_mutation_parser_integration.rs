//! Parsing of record field mutation, `reassign variable.field as value`
//! (Issue #11, step 6). Mutation is parsed into the AST only; the type checker
//! validates it and code generation still rejects records.
use chuchu::ast::{Expr, ExprKind, FieldName, FieldTarget, Stmt, StmtKind};
use chuchu::lexer::tokenize;
use chuchu::parser::{ParseError, Parser};
use chuchu::token::Span;

fn parse(source: &str) -> Result<Vec<Stmt>, ParseError> {
    Parser::new(tokenize(source).unwrap()).parse_program()
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

/// The span of `part` inside the first match of `whole`, for names that also occur
/// elsewhere, such as the `e` of `reassign`.
fn span_in(source: &str, whole: &str, part: &str) -> Span {
    let start = source.find(whole).unwrap() + whole.find(part).unwrap();
    Span {
        start,
        end: start + part.len(),
    }
}

fn field(source: &str, name: &str, occurrence: usize) -> FieldName {
    FieldName {
        name: name.into(),
        span: span_of(source, name, occurrence),
    }
}

/// The target and value of the first statement, which must be a field assignment.
fn assignment(source: &str) -> (FieldTarget, Expr) {
    let mut statements = parse(source).unwrap_or_else(|error| panic!("{source:?}: {error:?}"));
    match statements.remove(0).kind {
        StmtKind::AssignField { target, value } => (target, value),
        other => panic!("expected a field assignment, found {other:?}"),
    }
}

#[test]
fn parses_a_field_target_with_exact_spans() {
    let source = "reassign student.age as 25";
    let statements = parse(source).unwrap();
    assert_eq!(
        statements,
        vec![Stmt {
            kind: StmtKind::AssignField {
                target: FieldTarget {
                    variable: "student".into(),
                    variable_span: span_of(source, "student", 0),
                    fields: vec![field(source, "age", 0)],
                    span: span_of(source, "student.age", 0),
                },
                value: Expr {
                    kind: ExprKind::Integer("25".into()),
                    span: span_of(source, "25", 0),
                },
            },
            span: Span {
                start: 0,
                end: source.len()
            },
        }]
    );
}

#[test]
fn parses_nested_field_targets() {
    let source = "reassign student.address.city as \"Lahore\"";
    let (target, value) = assignment(source);
    assert_eq!(target.variable, "student");
    assert_eq!(target.variable_span, span_of(source, "student", 0));
    assert_eq!(
        target.fields,
        vec![field(source, "address", 0), field(source, "city", 0)]
    );
    assert_eq!(target.span, span_of(source, "student.address.city", 0));
    assert_eq!(value.kind, ExprKind::String("Lahore".into()));
    assert_eq!(value.span, span_of(source, "\"Lahore\"", 0));

    // Any depth: fields come out in source order from the variable outward.
    let source = "reassign a.b.c.d.e as 1";
    let (target, _) = assignment(source);
    let names: Vec<&str> = target.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["b", "c", "d", "e"]);
    assert_eq!(target.span, span_of(source, "a.b.c.d.e", 0));
    assert_eq!(target.fields[3].span, span_in(source, "a.b.c.d.e", "e"));
}

#[test]
fn field_spans_cover_only_the_name_even_with_spaces_around_dots() {
    let source = "reassign a . b . c as 1";
    let (target, _) = assignment(source);
    let whole = "a . b . c";
    assert_eq!(target.variable_span, span_in(source, whole, "a"));
    assert_eq!(
        target.fields,
        vec![
            FieldName {
                name: "b".into(),
                span: span_in(source, whole, "b")
            },
            FieldName {
                name: "c".into(),
                span: span_in(source, whole, "c")
            },
        ]
    );
    assert_eq!(target.span, span_of(source, whole, 0));
}

#[test]
fn the_assigned_value_is_a_full_expression() {
    for (source, expected) in [
        ("reassign s.n as s.n + 1", "Binary"),
        ("reassign s.n as 2 * (s.n - 1)", "Binary"),
        ("reassign s.n as -s.n", "Negate"),
        ("reassign s.n as f(1, s.m)", "Call"),
        ("reassign s.xs as [1, 2, 3]", "Array"),
        ("reassign s.n as xs[0]", "Index"),
        ("reassign s.n as t.n", "FieldAccess"),
        ("reassign s.n as t.u.n", "FieldAccess"),
        ("reassign s.ok as s.n < 3", "Binary"),
        ("reassign s.name as \"x\"", "String"),
        ("reassign s.ok as true", "Bool"),
        ("reassign s.f as 1.5", "Float"),
    ] {
        let (_, value) = assignment(source);
        assert!(
            format!("{:?}", value.kind).starts_with(expected),
            "{source:?}: {value:?}"
        );
    }
    // A record value, written over several lines.
    let source = "reassign s.address as Address {\n    city: \"x\"\n    zip: 1\n}";
    let (target, value) = assignment(source);
    assert_eq!(target.fields.len(), 1);
    assert!(matches!(value.kind, ExprKind::RecordInit { .. }));
    assert_eq!(value.span.end, source.len());
}

#[test]
fn statement_spans_run_from_reassign_to_the_end_of_the_value() {
    let source = "assign x as 1\nreassign s.n as x + 1\nshow x";
    let statements = parse(source).unwrap();
    assert_eq!(
        statements[1].span,
        span_of(source, "reassign s.n as x + 1", 0)
    );
    assert_eq!(statements.len(), 3);
}

#[test]
fn field_assignment_is_a_statement_wherever_statements_are_valid() {
    for source in [
        "if true {\n    reassign s.n as 1\n}",
        "if a.x < 1 {\n    reassign s.n as 1\n} else {\n    reassign s.n as 2\n}",
        "while s.n < 3 {\n    reassign s.n as s.n + 1\n}",
        "function f() be int {\n    reassign s.n as 1\n    return s.n\n}",
        "reassign s.n as 1\nreassign s.n as 2",
        "reassign s.n as 1\r\nreassign s.m as 2\r\n",
    ] {
        let statements = parse(source).unwrap_or_else(|error| panic!("{source:?}: {error:?}"));
        let text = format!("{statements:?}");
        assert!(text.contains("AssignField"), "{source:?}: {text}");
    }
    // Two assignments on one line are still separate statements.
    let source = "reassign s.n as 1 reassign s.m as 2";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedStatementSeparator {
            span: span_of(source, "reassign", 1)
        })
    );
}

#[test]
fn plain_reassignment_is_unchanged() {
    let statements = parse("reassign x as 1").unwrap();
    assert!(matches!(
        &statements[0].kind,
        StmtKind::Assign { name, .. } if name == "x"
    ));
    // Reading a field on the right of a plain reassignment is still just a value.
    let statements = parse("reassign x as s.n").unwrap();
    assert!(matches!(
        &statements[0].kind,
        StmtKind::Assign { name, value }
            if name == "x" && matches!(value.kind, ExprKind::FieldAccess { .. })
    ));
    // The existing parse errors of plain reassignment are unchanged.
    let source = "reassign x";
    assert!(matches!(
        parse(source),
        Err(ParseError::ExpectedToken {
            expected: "'as'",
            ..
        })
    ));
    let source = "reassign 5 as 1";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedIdentifier {
            span: span_of(source, "5", 0)
        })
    );
    assert!(matches!(
        parse("reassign x as"),
        Err(ParseError::ExpectedExpression { .. })
    ));
}

#[test]
fn rejects_targets_that_are_not_rooted_in_a_variable() {
    for (source, bad) in [
        // The result of a call.
        ("reassign f().x as 1", "f()"),
        ("reassign f(1, 2).x.y as 1", "f(1, 2)"),
        ("reassign f(1) as 2", "f(1)"),
        // A temporary record.
        (
            "reassign Student { name: \"a\" }.name as \"b\"",
            "Student { name: \"a\" }",
        ),
        (
            "reassign Student { name: \"a\" } as 1",
            "Student { name: \"a\" }",
        ),
        // Array elements: indexing is not a target, nor a field of an element.
        ("reassign xs[0] as 1", "xs[0]"),
        ("reassign xs[0].x as 1", "xs[0]"),
        ("reassign s.xs[0] as 1", "s.xs[0]"),
        ("reassign s.xs[0].x as 1", "s.xs[0]"),
    ] {
        assert_eq!(
            parse(source),
            Err(ParseError::InvalidAssignmentTarget {
                span: span_of(source, bad, 0)
            }),
            "{source:?}"
        );
    }
    // A multi-line temporary record reports the whole initializer.
    let source = "reassign Student {\n    name: \"a\"\n}.name as \"b\"";
    assert_eq!(
        parse(source),
        Err(ParseError::InvalidAssignmentTarget {
            span: Span {
                start: source.find("Student").unwrap(),
                end: source.find("}.").unwrap() + 1
            }
        })
    );
}

#[test]
fn rejects_targets_that_do_not_start_with_a_name() {
    for (source, bad) in [
        ("reassign 5 as 1", "5"),
        ("reassign \"s\".x as 1", "\"s\""),
        ("reassign (s).x as 1", "("),
        ("reassign -s.x as 1", "-"),
        ("reassign [1].x as 1", "["),
        ("reassign .x as 1", "."),
        ("reassign true.x as 1", "true"),
    ] {
        assert_eq!(
            parse(source),
            Err(ParseError::ExpectedIdentifier {
                span: span_of(source, bad, 0)
            }),
            "{source:?}"
        );
    }
}

#[test]
fn rejects_incomplete_assignments() {
    let source = "reassign s.x 5";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedToken {
            expected: "'as'",
            span: span_of(source, "5", 0)
        })
    );
    assert!(matches!(
        parse("reassign s.x"),
        Err(ParseError::ExpectedToken {
            expected: "'as'",
            ..
        })
    ));
    assert!(matches!(
        parse("reassign s.x as"),
        Err(ParseError::ExpectedExpression { .. })
    ));
    assert!(matches!(
        parse("reassign s.x as\nshow 1"),
        Err(ParseError::ExpectedExpression { .. })
    ));
    // The assignment operator of other languages is not syntax.
    let source = "reassign s.x = 5";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedToken {
            expected: "'as'",
            span: span_of(source, "=", 0)
        })
    );
    // A dot with no field name, or a number or keyword in its place.
    for source in [
        "reassign s. as 1",
        "reassign s.5 as 1",
        "reassign s.if as 1",
        "reassign s..x as 1",
        "reassign s.x. as 1",
    ] {
        assert!(
            matches!(parse(source), Err(ParseError::ExpectedIdentifier { .. })),
            "{source:?}: {:?}",
            parse(source)
        );
    }
    // The dot cannot be split across lines.
    assert!(parse("reassign s\n.x as 1").is_err());
    assert!(parse("reassign s.\nx as 1").is_err());
    // Fields are not callable, so a call after the target is not a target.
    assert!(parse("reassign s.x() as 1").is_err());
}

#[test]
fn declarations_cannot_target_fields() {
    // `assign` introduces a variable; only `reassign` writes to a field.
    let source = "assign s.x as 1";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedToken {
            expected: "'as'",
            span: span_of(source, ".", 0)
        })
    );
    assert!(parse("s.x as 1").is_err());
    assert!(parse("s.x = 1").is_err());
}

#[test]
fn programs_without_field_assignment_parse_as_before() {
    let statements =
        parse("assign a as 1.5\nreassign a as a + 2.\nshow a.x\nassign p as P {\n    x: 1\n}")
            .unwrap();
    assert_eq!(statements.len(), 4);
    assert!(!format!("{statements:?}").contains("AssignField"));
}
