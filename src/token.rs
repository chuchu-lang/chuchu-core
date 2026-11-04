#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    /// Variable declaration keyword: `assign`.
    Set,
    /// Assignment separator keyword: `as`.
    As,
    Show,

    Identifier(String),
    Integer(String),
    Float(String),
    String(String),
    True,
    False,
    If,
    Else,
    While,
    Function,
    /// Function return type separator keyword: `be`.
    Be,
    Reassign,
    Return,
    Err,
    /// Record definition keyword: `record`.
    Record,

    Plus,
    Minus,
    Star,
    Slash,
    Assign,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    LeftBrace,
    RightBrace,
    Comma,
    Colon,
    /// Member-access operator: `.`.
    Dot,

    Newline,
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
