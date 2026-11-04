//! Lexing and parsing of record field access, `object.field` (Issue #11, step 5).
//! Access is parsed into the AST only; the type checker validates it and code
//! generation still rejects records.
use chuchu::ast::{BinaryOperator, Expr, ExprKind, Stmt, StmtKind};
use chuchu::lexer::tokenize;
use chuchu::parser::{ParseError, Parser};
use chuchu::token::{Span, TokenKind};

fn parse(source: &str) -> Result<Vec<Stmt>, ParseError> {
    Parser::new(tokenize(source).unwrap()).parse_program()
}

fn parse_expression(source: &str) -> Result<Expr, ParseError> {
    Parser::new(tokenize(source).unwrap()).parse_expression()
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

fn kinds(source: &str) -> Vec<TokenKind> {
    let mut tokens: Vec<TokenKind> = tokenize(source)
        .unwrap()
        .into_iter()
        .map(|token| token.kind)
        .collect();
    assert_eq!(tokens.pop(), Some(TokenKind::Eof));
    tokens
}

fn ident(name: &str) -> TokenKind {
    TokenKind::Identifier(name.into())
}

/// `object.field` pieces of a field access.
fn access(expression: &Expr) -> (&Expr, &str) {
    match &expression.kind {
        ExprKind::FieldAccess { object, field } => (object, field),
        other => panic!("expected a field access, found {other:?}"),
    }
}

#[test]
fn the_dot_is_its_own_token_with_an_exact_span() {
    let tokens = tokenize("student.name").unwrap();
    assert_eq!(tokens[0].kind, ident("student"));
    assert_eq!(tokens[0].span, Span { start: 0, end: 7 });
    assert_eq!(tokens[1].kind, TokenKind::Dot);
    assert_eq!(tokens[1].span, Span { start: 7, end: 8 });
    assert_eq!(tokens[2].kind, ident("name"));
    assert_eq!(tokens[2].span, Span { start: 8, end: 12 });
    // Whitespace around a dot is skipped like anywhere else.
    assert_eq!(kinds("a . b"), vec![ident("a"), TokenKind::Dot, ident("b")]);
    assert_eq!(
        kinds("a.b.c"),
        vec![
            ident("a"),
            TokenKind::Dot,
            ident("b"),
            TokenKind::Dot,
            ident("c")
        ]
    );
}

#[test]
fn dots_after_names_calls_and_indexes_are_access_not_numbers() {
    assert_eq!(
        kinds("p1.x2"),
        vec![ident("p1"), TokenKind::Dot, ident("x2")]
    );
    assert_eq!(
        kinds("a[0].b"),
        vec![
            ident("a"),
            TokenKind::LeftBracket,
            TokenKind::Integer("0".into()),
            TokenKind::RightBracket,
            TokenKind::Dot,
            ident("b")
        ]
    );
    assert_eq!(
        kinds("f().b"),
        vec![
            ident("f"),
            TokenKind::LeftParen,
            TokenKind::RightParen,
            TokenKind::Dot,
            ident("b")
        ]
    );
    assert_eq!(
        kinds("(a).b"),
        vec![
            TokenKind::LeftParen,
            ident("a"),
            TokenKind::RightParen,
            TokenKind::Dot,
            ident("b")
        ]
    );
}

#[test]
fn float_literals_are_unchanged() {
    for literal in [
        "1.5", "0.25", "10.0", "2.", "1.5e3", "1.5e-3", "2e5", "3.E2",
    ] {
        let tokens = tokenize(literal).unwrap();
        assert_eq!(
            tokens[0].kind,
            if literal.contains(['.', 'e', 'E']) {
                TokenKind::Float(literal.into())
            } else {
                TokenKind::Integer(literal.into())
            },
            "{literal}"
        );
        assert_eq!(tokens.len(), 2, "{literal} is one token and Eof");
        assert_eq!(
            tokens[0].span,
            Span {
                start: 0,
                end: literal.len()
            }
        );
    }
    // A float followed by a dot and a name: the number keeps only its own dot.
    assert_eq!(
        kinds("1.5.x"),
        vec![TokenKind::Float("1.5".into()), TokenKind::Dot, ident("x")]
    );
    // A float next to field access in arithmetic.
    assert_eq!(
        kinds("a.x*1.5"),
        vec![
            ident("a"),
            TokenKind::Dot,
            ident("x"),
            TokenKind::Star,
            TokenKind::Float("1.5".into())
        ]
    );
    // A dot is never the start of a number: `.5` is a dot and an integer.
    assert_eq!(
        kinds(".5"),
        vec![TokenKind::Dot, TokenKind::Integer("5".into())]
    );
}

#[test]
fn parses_simple_access_with_exact_spans() {
    let source = "student.name";
    let expression = parse_expression(source).unwrap();
    assert_eq!(expression.span, Span { start: 0, end: 12 });
    let (object, field) = access(&expression);
    assert_eq!(field, "name");
    assert_eq!(object.kind, ExprKind::Identifier("student".into()));
    assert_eq!(object.span, Span { start: 0, end: 7 });
}

#[test]
fn chains_nest_to_the_left() {
    let source = "student.address.city";
    let outer = parse_expression(source).unwrap();
    assert_eq!(outer.span, Span { start: 0, end: 20 });
    let (inner, field) = access(&outer);
    assert_eq!(field, "city");
    assert_eq!(inner.span, span_of(source, "student.address", 0));
    let (root, middle) = access(inner);
    assert_eq!(middle, "address");
    assert_eq!(root.kind, ExprKind::Identifier("student".into()));
    // Longer chains keep nesting, and each prefix has its own span.
    let source = "a.b.c.d.e";
    let mut current = parse_expression(source).unwrap();
    for (name, prefix) in [
        ("e", "a.b.c.d.e"),
        ("d", "a.b.c.d"),
        ("c", "a.b.c"),
        ("b", "a.b"),
    ] {
        assert_eq!(current.span, span_of(source, prefix, 0), "{prefix}");
        let (object, field) = access(&current);
        assert_eq!(field, name);
        current = object.clone();
    }
    assert_eq!(current.kind, ExprKind::Identifier("a".into()));
}

#[test]
fn whitespace_around_the_dot_does_not_change_the_structure() {
    let spaced = parse_expression("a . b").unwrap();
    let (object, field) = access(&spaced);
    assert_eq!(field, "b");
    assert_eq!(object.kind, ExprKind::Identifier("a".into()));
    // The access still ends with the field name.
    assert_eq!(spaced.span, Span { start: 0, end: 5 });
}

#[test]
fn access_composes_with_calls_and_indexing() {
    // A call result.
    let source = "get_student().name";
    let expression = parse_expression(source).unwrap();
    let (object, field) = access(&expression);
    assert_eq!(field, "name");
    assert!(matches!(&object.kind, ExprKind::Call { name, arguments }
        if name == "get_student" && arguments.is_empty()));
    assert_eq!(object.span, span_of(source, "get_student()", 0));
    assert_eq!(
        expression.span,
        Span {
            start: 0,
            end: source.len()
        }
    );

    // Access on an indexed element, and an index on an accessed field.
    let expression = parse_expression("students[0].name").unwrap();
    let (object, field) = access(&expression);
    assert_eq!(field, "name");
    assert!(matches!(object.kind, ExprKind::Index { .. }));

    let expression = parse_expression("student.scores[1]").unwrap();
    let ExprKind::Index { array, index } = &expression.kind else {
        panic!("expected an index, found {expression:?}");
    };
    assert_eq!(access(array).1, "scores");
    assert_eq!(index.kind, ExprKind::Integer("1".into()));

    // Everything at once, in source order.
    let expression = parse_expression("f(x).items[2].name.first").unwrap();
    let (name_access, first) = access(&expression);
    assert_eq!(first, "first");
    let (index, name) = access(name_access);
    assert_eq!(name, "name");
    let ExprKind::Index { array, .. } = &index.kind else {
        panic!("expected an index, found {index:?}");
    };
    let (call, items) = access(array);
    assert_eq!(items, "items");
    assert!(matches!(call.kind, ExprKind::Call { .. }));

    // Arguments and indexes may themselves use access.
    let expression = parse_expression("f(a.x, b.y.z)").unwrap();
    let ExprKind::Call { arguments, .. } = &expression.kind else {
        panic!("expected a call");
    };
    assert_eq!(access(&arguments[0]).1, "x");
    assert_eq!(access(&arguments[1]).1, "z");
    let expression = parse_expression("xs[a.i]").unwrap();
    let ExprKind::Index { index, .. } = &expression.kind else {
        panic!("expected an index");
    };
    assert_eq!(access(index).1, "i");
}

#[test]
fn access_binds_tighter_than_operators() {
    let expression = parse_expression("a.x + b.y * 2").unwrap();
    let ExprKind::Binary {
        left,
        operator,
        right,
    } = &expression.kind
    else {
        panic!("expected a binary expression");
    };
    assert_eq!(*operator, BinaryOperator::Add);
    assert_eq!(access(left).1, "x");
    let ExprKind::Binary {
        left: product_left,
        operator: product,
        right: product_right,
    } = &right.kind
    else {
        panic!("expected a product on the right");
    };
    assert_eq!(*product, BinaryOperator::Multiply);
    assert_eq!(access(product_left).1, "y");
    assert_eq!(product_right.kind, ExprKind::Integer("2".into()));

    // Negation applies to the whole access, not to the object.
    let expression = parse_expression("-a.x").unwrap();
    let ExprKind::Negate(inner) = &expression.kind else {
        panic!("expected a negation, found {expression:?}");
    };
    assert_eq!(access(inner).1, "x");
    assert_eq!(inner.span, Span { start: 1, end: 4 });

    // Comparisons take accesses on both sides.
    let expression = parse_expression("a.x < b.y").unwrap();
    let ExprKind::Binary { left, right, .. } = &expression.kind else {
        panic!("expected a comparison");
    };
    assert_eq!((access(left).1, access(right).1), ("x", "y"));
}

#[test]
fn parentheses_can_group_before_access() {
    let source = "(a).x";
    let expression = parse_expression(source).unwrap();
    let (object, field) = access(&expression);
    assert_eq!(field, "x");
    // The parenthesized object keeps the span of its parentheses.
    assert_eq!(object.span, Span { start: 0, end: 3 });
    assert_eq!(expression.span, Span { start: 0, end: 5 });

    let expression = parse_expression("(a + b).x").unwrap();
    let (object, field) = access(&expression);
    assert_eq!(field, "x");
    assert!(matches!(object.kind, ExprKind::Binary { .. }));
    let expression = parse_expression("(a.b).c").unwrap();
    assert_eq!(access(&expression).1, "c");
}

#[test]
fn access_works_on_initializers() {
    let source = "P { x: 1 }.x";
    let expression = parse_expression(source).unwrap();
    let (object, field) = access(&expression);
    assert_eq!(field, "x");
    assert!(matches!(object.kind, ExprKind::RecordInit { .. }));
    assert_eq!(object.span, span_of(source, "P { x: 1 }", 0));

    // A multi-line initializer, then access on the closing line.
    let source = "assign c as Outer {\n    inner: Inner {\n        n: 1\n    }.n\n}";
    let statements = parse(source).unwrap();
    let StmtKind::Set { value, .. } = &statements[0].kind else {
        panic!("expected a declaration");
    };
    let ExprKind::RecordInit { fields, .. } = &value.kind else {
        panic!("expected an initializer");
    };
    assert_eq!(access(&fields[0].value).1, "n");
    // And an initializer field whose value reads another record's field.
    let statements = parse("assign p as P {\n    n: student.age\n}").unwrap();
    assert!(format!("{statements:?}").contains("FieldAccess"));
}

#[test]
fn access_is_an_expression_wherever_expressions_are_valid() {
    for source in [
        "show student.name",
        "assign n as student.age",
        "reassign n as student.age",
        "assign xs as [a.x, b.y]",
        "assign n as f(student.age)",
        "function f() be int {\n    return s.n\n}",
        "if s.ready {\n    show 1\n}",
        "while s.n < 3 {\n    reassign i as i + 1\n}",
        "if a.x == b.y {\n    show 1\n} else {\n    show 2\n}",
    ] {
        let statements = parse(source).unwrap_or_else(|error| panic!("{source:?}: {error:?}"));
        let text = format!("{statements:?}");
        assert!(text.contains("FieldAccess"), "{source:?}: {text}");
    }
    // `name.field {` in a condition still ends at the body brace.
    let statements = parse("if s.ready {\n    show 1\n}").unwrap();
    let StmtKind::If {
        condition,
        then_body,
        ..
    } = &statements[0].kind
    else {
        panic!("expected an if statement");
    };
    assert_eq!(access(condition).1, "ready");
    assert_eq!(then_body.len(), 1);
    assert!(!format!("{statements:?}").contains("RecordInit"));
}

#[test]
fn statements_keep_their_spans_around_access() {
    let source = "show student.name";
    let statements = parse(source).unwrap();
    assert_eq!(
        statements[0].span,
        Span {
            start: 0,
            end: source.len()
        }
    );
    let source = "assign n as student.address.city";
    let statements = parse(source).unwrap();
    assert_eq!(
        statements[0].span,
        Span {
            start: 0,
            end: source.len()
        }
    );
}

#[test]
fn rejects_malformed_access() {
    // Nothing after the dot.
    for source in ["show a.", "show a.\nshow 1"] {
        assert!(
            matches!(parse(source), Err(ParseError::ExpectedIdentifier { .. })),
            "{source:?}: {:?}",
            parse(source)
        );
    }
    // The field must be a name: not a number, a keyword, or another dot.
    let source = "show a.5";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedIdentifier {
            span: span_of(source, "5", 0)
        })
    );
    for source in [
        "show a.if",
        "show a.show",
        "show a.true",
        "show a..b",
        "show a.(b)",
    ] {
        assert!(
            matches!(parse(source), Err(ParseError::ExpectedIdentifier { .. })),
            "{source:?}: {:?}",
            parse(source)
        );
    }
    // A leading dot has nothing to read a field from.
    let source = "show .a";
    assert_eq!(
        parse(source),
        Err(ParseError::ExpectedExpression {
            span: span_of(source, ".", 0)
        })
    );
    // The dot must stay on the same line as the object and the field.
    assert!(parse("show a\n.b").is_err());
    assert!(parse("show a.\nb").is_err());
    // A module call has one named receiver; nested record fields are not callable.
    assert!(matches!(
        parse("show a.b.c(1)"),
        Err(ParseError::ExpectedExpression { .. })
    ));
    // A number literal is not a record: `1.x` is the float `1.` and then a name.
    assert!(matches!(
        parse("show 1.x"),
        Err(ParseError::ExpectedStatementSeparator { .. })
    ));
}

#[test]
fn only_reassign_can_write_a_field() {
    // Step 6 made `reassign s.x as 1` a field assignment; reading syntax is
    // unchanged, and nothing else can write to a field.
    let statements = parse("reassign s.x as 1").unwrap();
    assert!(matches!(statements[0].kind, StmtKind::AssignField { .. }));
    assert!(parse("assign s.x as 1").is_err());
    assert!(parse("s.x = 1").is_err());
    assert!(parse("s.x as 1").is_err());
}

#[test]
fn programs_without_access_parse_as_before() {
    let statements = parse("assign a as 1.5\nassign b as 2.\nshow a + b * 3.25e1").unwrap();
    assert_eq!(statements.len(), 3);
    assert!(!format!("{statements:?}").contains("FieldAccess"));
}
