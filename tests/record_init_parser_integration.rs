//! Parsing of record initializers, `Name { field: value }` (Issue #11, step 4).
//! Initializers are parsed into the AST only; the type checker validates them and
//! code generation still rejects records.
use chuchu::ast::{Expr, ExprKind, FieldInit, Stmt, StmtKind};
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

fn expr(kind: ExprKind, span: Span) -> Expr {
    Expr { kind, span }
}

/// The initializer on the right of a leading `assign x as ...` statement.
fn initializer(source: &str) -> Expr {
    let mut statements = parse(source).unwrap_or_else(|error| panic!("{source:?}: {error:?}"));
    match statements.remove(0).kind {
        StmtKind::Set { value, .. } => value,
        other => panic!("expected a declaration, found {other:?}"),
    }
}

fn is_initializer(expression: &Expr, expected_name: &str, field_names: &[&str]) -> bool {
    matches!(
        &expression.kind,
        ExprKind::RecordInit { name, fields }
            if name == expected_name
                && fields.iter().map(|f| f.name.as_str()).eq(field_names.iter().copied())
    )
}

#[test]
fn parses_an_initializer_with_exact_spans() {
    let source = "assign student as Student {\n    name: \"Ali\"\n    age: 22\n}";
    let statements = parse(source).unwrap();
    let initializer_span = Span {
        start: source.find("Student {").unwrap(),
        end: source.len(),
    };
    assert_eq!(
        statements,
        vec![Stmt {
            kind: StmtKind::Set {
                name: "student".into(),
                value: expr(
                    ExprKind::RecordInit {
                        name: "Student".into(),
                        fields: vec![
                            FieldInit {
                                name: "name".into(),
                                value: expr(
                                    ExprKind::String("Ali".into()),
                                    span_of(source, "\"Ali\"", 0)
                                ),
                                span: span_of(source, "name: \"Ali\"", 0),
                            },
                            FieldInit {
                                name: "age".into(),
                                value: expr(
                                    ExprKind::Integer("22".into()),
                                    span_of(source, "22", 0)
                                ),
                                span: span_of(source, "age: 22", 0),
                            },
                        ],
                    },
                    initializer_span
                ),
            },
            span: Span {
                start: 0,
                end: source.len()
            },
        }]
    );
}

#[test]
fn parses_nested_initializers() {
    let source = "assign c as Course {\n    title: \"Math\"\n    teacher: Person {\n        name: \"Sam\"\n    }\n}";
    let course = initializer(source);
    assert!(is_initializer(&course, "Course", &["title", "teacher"]));
    let ExprKind::RecordInit { fields, .. } = &course.kind else {
        unreachable!()
    };
    let person = &fields[1].value;
    assert!(is_initializer(person, "Person", &["name"]));
    // The nested initializer spans `Person` through its own closing brace, and the
    // field spans from `teacher` to that same brace.
    let start = source.find("Person {").unwrap();
    let end = source.rfind("    }").unwrap() + "    }".len();
    assert_eq!(person.span, Span { start, end });
    assert_eq!(
        fields[1].span,
        Span {
            start: source.find("teacher").unwrap(),
            end
        }
    );
    assert_eq!(
        course.span,
        Span {
            start: source.find("Course {").unwrap(),
            end: source.len()
        }
    );
}

#[test]
fn field_values_are_full_expressions() {
    let source = "assign p as P {\n    a: 1 + 2 * b\n    b: f(3)\n    c: [1, 2][0]\n    d: -x\n    e: (1 + 2)\n}";
    let value = initializer(source);
    let ExprKind::RecordInit { fields, .. } = &value.kind else {
        panic!("expected an initializer")
    };
    assert!(matches!(fields[0].value.kind, ExprKind::Binary { .. }));
    assert!(matches!(fields[1].value.kind, ExprKind::Call { .. }));
    assert!(matches!(fields[2].value.kind, ExprKind::Index { .. }));
    assert!(matches!(fields[3].value.kind, ExprKind::Negate(_)));
    assert!(matches!(fields[4].value.kind, ExprKind::Binary { .. }));
    assert_eq!(fields[0].span, span_of(source, "a: 1 + 2 * b", 0));
}

#[test]
fn single_line_initializers_follow_the_declaration_conventions() {
    let one = initializer("assign p as P { x: 1 }");
    assert!(is_initializer(&one, "P", &["x"]));
    // Blank lines around and between fields are allowed, like in a declaration.
    let spaced = initializer("assign p as P {\n\n    x: 1\n\n    y: 2\n\n}");
    assert!(is_initializer(&spaced, "P", &["x", "y"]));
    // An empty initializer parses; the type checker reports the missing fields.
    let empty = initializer("assign p as P {}");
    assert!(is_initializer(&empty, "P", &[]));
    let empty = initializer("assign p as P {\n}");
    assert!(is_initializer(&empty, "P", &[]));
    // CRLF line endings are newlines too.
    let crlf = initializer("assign p as P {\r\n    x: 1\r\n    y: 2\r\n}");
    assert!(is_initializer(&crlf, "P", &["x", "y"]));
}

#[test]
fn initializers_are_expressions_wherever_expressions_are_valid() {
    // `reassign`, `show`, call arguments, array elements, and field values.
    for (source, position) in [
        ("assign p as P { x: 1 }\nreassign p as P { x: 2 }", 1),
        ("show P { x: 1 }", 0),
        ("assign a as f(P { x: 1 }, 2)", 0),
        ("assign a as [P { x: 1 }, P { x: 2 }]", 0),
        ("assign a as g(\n    P {\n        x: 1\n    }\n)", 0),
        ("function f() be int {\n    return P { x: 1 }\n}", 0),
        ("if true {\n    assign p as P { x: 1 }\n}", 0),
    ] {
        let statements = parse(source).unwrap_or_else(|error| panic!("{source:?}: {error:?}"));
        assert!(!statements.is_empty(), "{source:?}");
        let found = format!("{:?}", statements[position]);
        assert!(found.contains("RecordInit"), "{source:?}: {found}");
    }
}

#[test]
fn braces_after_a_condition_still_open_the_body() {
    // `name {` in an `if` or `while` condition is the body, not an initializer.
    for source in [
        "assign ready as true\nif ready {\n    show 1\n}",
        "assign ready as true\nwhile ready {\n    reassign ready as false\n}",
        "if a == b {\n    show 1\n} else {\n    show 2\n}",
        "if a < b {\n    show 1\n}",
        "while done {\n    show 1\n}",
        "if ready {\n    if inner {\n        show 1\n    }\n}",
    ] {
        let statements = parse(source).unwrap_or_else(|error| panic!("{source:?}: {error:?}"));
        let text = format!("{statements:?}");
        assert!(!text.contains("RecordInit"), "{source:?}: {text}");
    }
    let statements = parse("if ready {\n    show 1\n}").unwrap();
    let StmtKind::If {
        condition,
        then_body,
        ..
    } = &statements[0].kind
    else {
        panic!("expected an if statement")
    };
    assert_eq!(condition.kind, ExprKind::Identifier("ready".into()));
    assert_eq!(then_body.len(), 1);
}

#[test]
fn delimiters_allow_initializers_inside_conditions() {
    // Inside parentheses, call arguments, or an index the `{` is unambiguous.
    let source = "if f(P { x: 1 }) {\n    show 1\n}";
    let statements = parse(source).unwrap();
    let StmtKind::If {
        condition,
        then_body,
        ..
    } = &statements[0].kind
    else {
        panic!("expected an if statement")
    };
    assert!(format!("{condition:?}").contains("RecordInit"));
    assert_eq!(then_body.len(), 1);

    for source in [
        "while (P { x: 1 }) {\n    show 1\n}",
        "if a[P { x: 1 }] {\n    show 1\n}",
    ] {
        let text = format!("{:?}", parse(source).unwrap());
        assert!(text.contains("RecordInit"), "{source:?}");
    }
    // The restriction returns after the closing delimiter.
    let source = "if f(P { x: 1 }) == ready {\n    show 1\n}";
    let text = format!("{:?}", parse(source).unwrap());
    assert_eq!(text.matches("RecordInit").count(), 1, "{text}");
    // A body inside a condition's initializer is not affected either.
    let statements = parse("if x {\n    assign p as P { y: 1 }\n}").unwrap();
    assert!(format!("{statements:?}").contains("RecordInit"));
}

#[test]
fn plain_identifiers_and_calls_are_unchanged() {
    let statements = parse("assign a as b\nassign c as f(b)\nshow a + c").unwrap();
    assert_eq!(statements.len(), 3);
    assert!(!format!("{statements:?}").contains("RecordInit"));
    // A brace on the next line is not an initializer.
    assert!(parse("assign a as b\n{").is_err());
}

#[test]
fn rejects_malformed_initializers() {
    let source = "assign p as P { x: 1 y: 2 }";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedToken {
            expected: "newline between fields",
            span: span_of(source, "y", 0)
        })
    );
    let source = "assign p as P {\n    x 1\n}";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedToken {
            expected: "':'",
            span: span_of(source, "1", 0)
        })
    );
    let source = "assign p as P {\n    x:\n}";
    assert!(matches!(
        parse(source),
        Err(ParseError::ExpectedExpression { .. })
    ));
    let source = "assign p as P {\n    : 1\n}";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedIdentifier {
            span: span_of(source, ":", 0)
        })
    );
    let source = "assign p as P {\n    1: 1\n}";
    assert!(matches!(
        parse(source),
        Err(ParseError::ExpectedIdentifier { .. })
    ));
    // Unterminated initializers.
    assert!(matches!(
        parse("assign p as P {\n    x: 1\n"),
        Err(ParseError::ExpectedToken {
            expected: "'}'",
            ..
        } | ParseError::UnexpectedEnd)
    ));
    assert!(matches!(
        parse("assign p as P {"),
        Err(ParseError::ExpectedToken {
            expected: "'}'",
            ..
        } | ParseError::UnexpectedEnd)
    ));
    // Another statement cannot follow the closing brace on the same line.
    let source = "assign p as P { x: 1 } show p";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedStatementSeparator {
            span: span_of(source, "show", 0)
        })
    );
    // A trailing comma is not part of the syntax.
    assert!(parse("assign p as P { x: 1, }").is_err());
    assert!(parse("assign p as P {\n    x: 1,\n    y: 2\n}").is_err());
}

#[test]
fn record_declarations_still_parse_and_require_fields() {
    let statements = parse("record P {\n    x: int\n}").unwrap();
    assert!(matches!(statements[0].kind, StmtKind::Record { .. }));
    assert!(matches!(
        parse("record P {\n}"),
        Err(ParseError::ExpectedToken {
            expected: "at least one field",
            ..
        })
    ));
}
