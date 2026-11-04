use chuchu::lexer::tokenize;
use chuchu::token::TokenKind;

#[test]
fn assign_is_reserved_and_set_is_an_identifier() {
    let tokens = tokenize("assign set as 5\nshow assignable").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Set);
    assert_eq!(tokens[0].span, chuchu::token::Span { start: 0, end: 6 });
    assert_eq!(tokens[1].kind, TokenKind::Identifier("set".to_owned()));
    assert_eq!(tokens[2].kind, TokenKind::As);
    assert_eq!(tokens[3].kind, TokenKind::Integer("5".to_owned()));
    assert_eq!(tokens[3].span, chuchu::token::Span { start: 14, end: 15 });
    assert_eq!(
        tokens[6].kind,
        TokenKind::Identifier("assignable".to_owned())
    );
}

#[test]
fn tokenizes_complete_chuchu_program() {
    let source = include_str!("../examples/main.chuchu");

    let tokens = tokenize(source).expect("Valid Chuchu program should tokenize");

    let kinds: Vec<TokenKind> = tokens
        .into_iter()
        .map(|token| token.kind)
        .filter(|kind| !matches!(kind, TokenKind::Newline))
        .collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::Set,
            TokenKind::Identifier("x".to_owned()),
            TokenKind::As,
            TokenKind::Integer("10".to_owned()),
            TokenKind::Set,
            TokenKind::Identifier("y".to_owned()),
            TokenKind::As,
            TokenKind::Integer("20".to_owned()),
            TokenKind::Reassign,
            TokenKind::Identifier("x".to_owned()),
            TokenKind::As,
            TokenKind::Integer("15".to_owned()),
            TokenKind::Show,
            TokenKind::Identifier("x".to_owned()),
            TokenKind::Plus,
            TokenKind::Identifier("y".to_owned()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn recognizes_function_and_reassign_keywords_and_identifier_boundaries() {
    let tokens = tokenize("function function_name reassign reassignment").unwrap();
    let kinds: Vec<TokenKind> = tokens.into_iter().map(|token| token.kind).collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::Function,
            TokenKind::Identifier("function_name".to_owned()),
            TokenKind::Reassign,
            TokenKind::Identifier("reassignment".to_owned()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn as_is_reserved_with_exact_spans_and_identifier_boundaries() {
    let source = "as aspect as_value assignable reassignment";
    let tokens = tokenize(source).unwrap();
    assert_eq!(tokens[0].kind, TokenKind::As);
    assert_eq!(tokens[0].span, chuchu::token::Span { start: 0, end: 2 });
    for (token, name) in
        tokens[1..5]
            .iter()
            .zip(["aspect", "as_value", "assignable", "reassignment"])
    {
        assert_eq!(token.kind, TokenKind::Identifier(name.to_owned()));
        assert_eq!(&source[token.span.start..token.span.end], name);
    }
}

#[test]
fn be_is_reserved_with_identifier_boundaries_and_exact_spans() {
    let source = "be before be_value";
    let tokens = tokenize(source).unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Be);
    assert_eq!(tokens[0].span, chuchu::token::Span { start: 0, end: 2 });
    assert_eq!(tokens[1].kind, TokenKind::Identifier("before".into()));
    assert_eq!(tokens[2].kind, TokenKind::Identifier("be_value".into()));
    assert_eq!(tokenize("->").unwrap()[0].kind, TokenKind::Minus);
    assert_eq!(tokenize("->").unwrap()[1].kind, TokenKind::Greater);
}

#[test]
fn record_is_reserved_with_identifier_boundaries_and_exact_spans() {
    let tokens = tokenize("record").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Record);
    assert_eq!(tokens[0].span, chuchu::token::Span { start: 0, end: 6 });

    let tokens = tokenize("record Student").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Record);
    assert_eq!(tokens[1].kind, TokenKind::Identifier("Student".into()));
    assert_eq!(tokens[1].span, chuchu::token::Span { start: 7, end: 14 });

    let tokens = tokenize("recording student_record record_ Record records").unwrap();
    for (token, name) in tokens.iter().zip([
        "recording",
        "student_record",
        "record_",
        "Record",
        "records",
    ]) {
        assert_eq!(token.kind, TokenKind::Identifier(name.into()));
    }
}

#[test]
fn module_keywords_and_namespace_separator_preserve_boundaries_and_spans() {
    let source = "module use export modular useful exported math::add import";
    let tokens = chuchu::lexer::tokenize(source).unwrap();
    use chuchu::token::TokenKind;
    assert_eq!(tokens[0].kind, TokenKind::Module);
    assert_eq!(tokens[1].kind, TokenKind::Use);
    assert_eq!(tokens[2].kind, TokenKind::Export);
    for (index, name) in [(3, "modular"), (4, "useful"), (5, "exported")] {
        assert_eq!(tokens[index].kind, TokenKind::Identifier(name.into()));
    }
    assert_eq!(tokens[9].kind, TokenKind::Identifier("import".into()));
    assert_eq!(tokens[7].kind, TokenKind::ColonColon);
    assert_eq!(&source[tokens[7].span.start..tokens[7].span.end], "::");
}
