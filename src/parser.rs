use crate::ast::{
    BinaryOperator, Expr, ExprKind, FieldInit, FieldName, FieldTarget, FieldType, Parameter,
    RecordField, Stmt, StmtKind,
};
use crate::token::{Span, Token, TokenKind};
use crate::types::{ScalarType, Type};

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    ExpectedExpression {
        span: Span,
    },
    ExpectedStatement {
        span: Span,
    },
    ExpectedIdentifier {
        span: Span,
    },
    ExpectedStatementSeparator {
        span: Span,
    },
    ExpectedToken {
        expected: &'static str,
        span: Span,
    },
    /// `reassign` was given a target that is not a variable or one of its fields.
    InvalidAssignmentTarget {
        span: Span,
    },
    UnexpectedEnd,
}

pub struct Parser {
    tokens: Vec<Token>,
    position: usize,
    /// Set while parsing an `if` or `while` condition, where `name {` opens the
    /// body block rather than a record initializer. Parentheses, brackets, and
    /// call arguments clear it again.
    in_condition: bool,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
            in_condition: false,
        }
    }
    pub fn current(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }
    pub fn advance(&mut self) {
        if self.position + 1 < self.tokens.len() {
            self.position += 1;
        }
    }
    fn token(&self) -> Result<Token, ParseError> {
        self.current().cloned().ok_or(ParseError::UnexpectedEnd)
    }
    fn at(&self, kind: &TokenKind) -> bool {
        self.current().is_some_and(|token| &token.kind == kind)
    }
    fn require(&mut self, kind: TokenKind, expected: &'static str) -> Result<Token, ParseError> {
        let token = self.token()?;
        if token.kind != kind {
            return Err(ParseError::ExpectedToken {
                expected,
                span: token.span,
            });
        }
        self.advance();
        Ok(token)
    }
    fn newlines(&mut self) -> Result<(), ParseError> {
        while self.at(&TokenKind::Newline) {
            if self.position + 1 >= self.tokens.len() {
                return Err(ParseError::UnexpectedEnd);
            }
            self.advance();
        }
        Ok(())
    }
    fn name(&mut self) -> Result<(String, Span), ParseError> {
        let token = self.token()?;
        let TokenKind::Identifier(name) = token.kind else {
            return Err(ParseError::ExpectedIdentifier { span: token.span });
        };
        self.advance();
        Ok((name, token.span))
    }
    pub fn parse_program(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.sequence(false)
    }
    fn sequence(&mut self, block: bool) -> Result<Vec<Stmt>, ParseError> {
        let mut statements = Vec::new();
        loop {
            self.newlines()?;
            let token = self.token()?;
            if token.kind == TokenKind::Eof {
                if block {
                    return Err(ParseError::ExpectedToken {
                        expected: "'}'",
                        span: token.span,
                    });
                }
                return Ok(statements);
            }
            if block && token.kind == TokenKind::RightBrace {
                return Ok(statements);
            }
            statements.push(self.parse_statement()?);
            let token = self.token()?;
            if !matches!(token.kind, TokenKind::Newline | TokenKind::Eof)
                && !(block && token.kind == TokenKind::RightBrace)
            {
                return Err(ParseError::ExpectedStatementSeparator { span: token.span });
            }
        }
    }
    fn block(&mut self) -> Result<(Vec<Stmt>, usize), ParseError> {
        self.require(TokenKind::LeftBrace, "'{'")?;
        let statements = self.sequence(true)?;
        let end = self.require(TokenKind::RightBrace, "'}'")?.span.end;
        Ok((statements, end))
    }
    fn annotation(&mut self) -> Result<Type, ParseError> {
        let token = self.token()?;
        let ty = match &token.kind {
            TokenKind::Identifier(name) if name == "result" => {
                self.advance();
                self.require(TokenKind::Less, "'<'")?;
                let inner = self.annotation()?;
                let payload = ScalarType::from_type(inner).ok_or(ParseError::ExpectedToken {
                    expected: "int, float, bool, or string result payload",
                    span: token.span,
                })?;
                self.require(TokenKind::Greater, "'>'")?;
                return Ok(Type::Result(payload));
            }
            TokenKind::Identifier(name) if name == "int" => Type::Int,
            TokenKind::Identifier(name) if name == "float" => Type::Float,
            TokenKind::Identifier(name) if name == "bool" => Type::Bool,
            TokenKind::Identifier(name) if name == "string" => Type::String,
            _ => {
                return Err(ParseError::ExpectedToken {
                    expected: "int, float, bool, string, or result<T> type",
                    span: token.span,
                });
            }
        };
        self.advance();
        Ok(ty)
    }
    /// Parses `{ name: type }` with one field per line; returns the fields and
    /// the end of the closing brace.
    fn record_fields(&mut self) -> Result<(Vec<RecordField>, usize), ParseError> {
        self.require(TokenKind::LeftBrace, "'{'")?;
        let mut fields = Vec::new();
        loop {
            self.newlines()?;
            let token = self.token()?;
            if token.kind == TokenKind::RightBrace {
                if fields.is_empty() {
                    return Err(ParseError::ExpectedToken {
                        expected: "at least one field",
                        span: token.span,
                    });
                }
                self.advance();
                return Ok((fields, token.span.end));
            }
            if token.kind == TokenKind::Eof {
                return Err(ParseError::ExpectedToken {
                    expected: "'}'",
                    span: token.span,
                });
            }
            let (name, name_span) = self.name()?;
            self.require(TokenKind::Colon, "':'")?;
            let (ty, ty_span) = self.field_type()?;
            fields.push(RecordField {
                name,
                ty,
                span: Span {
                    start: name_span.start,
                    end: ty_span.end,
                },
            });
            let token = self.token()?;
            if !matches!(token.kind, TokenKind::Newline | TokenKind::RightBrace) {
                return Err(ParseError::ExpectedToken {
                    expected: "newline between fields",
                    span: token.span,
                });
            }
        }
    }
    fn field_type(&mut self) -> Result<(FieldType, Span), ParseError> {
        let token = self.token()?;
        let ty = match &token.kind {
            TokenKind::Identifier(name) if name == "int" => FieldType::Builtin(Type::Int),
            TokenKind::Identifier(name) if name == "float" => FieldType::Builtin(Type::Float),
            TokenKind::Identifier(name) if name == "bool" => FieldType::Builtin(Type::Bool),
            TokenKind::Identifier(name) if name == "string" => FieldType::Builtin(Type::String),
            TokenKind::Identifier(name) if name != "result" => FieldType::Named(name.clone()),
            _ => {
                return Err(ParseError::ExpectedToken {
                    expected: "int, float, bool, string, or a record type name",
                    span: token.span,
                });
            }
        };
        self.advance();
        Ok((ty, token.span))
    }
    pub fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        let token = self.token()?;
        let start = token.span.start;
        let (kind, end) = match token.kind {
            TokenKind::Set => {
                self.advance();
                let (name, _) = self.name()?;
                self.require(TokenKind::As, "'as'")?;
                let value = self.parse_expression()?;
                let end = value.span.end;
                (StmtKind::Set { name, value }, end)
            }
            TokenKind::Reassign => {
                self.advance();
                let target_start = self.position;
                let (name, _) = self.name()?;
                // A name followed by `.`, `(`, `[`, or `{` begins a longer target.
                let longer = matches!(
                    self.current().map(|token| &token.kind),
                    Some(
                        TokenKind::Dot
                            | TokenKind::LeftParen
                            | TokenKind::LeftBracket
                            | TokenKind::LeftBrace
                    )
                );
                if longer {
                    self.position = target_start;
                    let target = self.field_target()?;
                    self.require(TokenKind::As, "'as'")?;
                    let value = self.parse_expression()?;
                    let end = value.span.end;
                    (StmtKind::AssignField { target, value }, end)
                } else {
                    self.require(TokenKind::As, "'as'")?;
                    let value = self.parse_expression()?;
                    let end = value.span.end;
                    (StmtKind::Assign { name, value }, end)
                }
            }
            TokenKind::Show | TokenKind::Return => {
                self.advance();
                let expression = self.parse_expression()?;
                let end = expression.span.end;
                (
                    if token.kind == TokenKind::Show {
                        StmtKind::Show(expression)
                    } else {
                        StmtKind::Return(expression)
                    },
                    end,
                )
            }
            TokenKind::If => {
                self.advance();
                let condition = self.condition()?;
                let (then_body, mut end) = self.block()?;
                let saved = self.position;
                self.newlines()?;
                let else_body = if self.at(&TokenKind::Else) {
                    self.advance();
                    let (body, last) = self.block()?;
                    end = last;
                    body
                } else {
                    self.position = saved;
                    Vec::new()
                };
                (
                    StmtKind::If {
                        condition,
                        then_body,
                        else_body,
                    },
                    end,
                )
            }
            TokenKind::While => {
                self.advance();
                let condition = self.condition()?;
                let (body, end) = self.block()?;
                (StmtKind::While { condition, body }, end)
            }
            TokenKind::Function => {
                self.advance();
                let (name, _) = self.name()?;
                self.require(TokenKind::LeftParen, "'('")?;
                let mut parameters = Vec::new();
                self.newlines()?;
                if !self.at(&TokenKind::RightParen) {
                    loop {
                        let (name, span) = self.name()?;
                        self.require(TokenKind::Colon, "':'")?;
                        let ty = self.annotation()?;
                        parameters.push(Parameter { name, ty, span });
                        self.newlines()?;
                        if !self.at(&TokenKind::Comma) {
                            break;
                        }
                        self.advance();
                        self.newlines()?;
                    }
                }
                self.require(TokenKind::RightParen, "')'")?;
                self.require(TokenKind::Be, "'be'")?;
                let return_type = self.annotation()?;
                let (body, end) = self.block()?;
                (
                    StmtKind::Function {
                        name,
                        parameters,
                        return_type,
                        body,
                    },
                    end,
                )
            }
            TokenKind::Record => {
                self.advance();
                let (name, _) = self.name()?;
                let (fields, end) = self.record_fields()?;
                (StmtKind::Record { name, fields }, end)
            }
            _ => return Err(ParseError::ExpectedStatement { span: token.span }),
        };
        Ok(Stmt {
            kind,
            span: Span { start, end },
        })
    }
    pub fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        self.binary(0)
    }
    /// Parses a target longer than a name, as an expression, then keeps it only if
    /// it is `variable.field...`. Otherwise the error points at the part that is
    /// not a variable: the call, index, or record initializer the chain starts from.
    fn field_target(&mut self) -> Result<FieldTarget, ParseError> {
        let expression = self.atom()?;
        let mut fields = Vec::new();
        let mut root = &expression;
        while let ExprKind::FieldAccess { object, field } = &root.kind {
            fields.push(FieldName {
                name: field.clone(),
                // An access ends with its field name.
                span: Span {
                    start: root.span.end - field.len(),
                    end: root.span.end,
                },
            });
            root = object;
        }
        fields.reverse();
        match &root.kind {
            ExprKind::Identifier(variable) if !fields.is_empty() => Ok(FieldTarget {
                variable: variable.clone(),
                variable_span: root.span,
                fields,
                span: expression.span,
            }),
            _ => Err(ParseError::InvalidAssignmentTarget { span: root.span }),
        }
    }
    /// Parses an `if` or `while` condition, where `name {` must open the body.
    fn condition(&mut self) -> Result<Expr, ParseError> {
        let outer = std::mem::replace(&mut self.in_condition, true);
        let condition = self.parse_expression();
        self.in_condition = outer;
        condition
    }
    /// Parses an expression nested in delimiters, where a record initializer is
    /// unambiguous again even inside a condition.
    fn nested_expression(&mut self) -> Result<Expr, ParseError> {
        let outer = std::mem::replace(&mut self.in_condition, false);
        let expression = self.parse_expression();
        self.in_condition = outer;
        expression
    }
    /// Parses `{ field: expression }` after a record name, one field per line like
    /// a declaration. Which fields are required is checked by the type checker.
    fn record_init(&mut self, name: String, name_span: Span) -> Result<Expr, ParseError> {
        self.require(TokenKind::LeftBrace, "'{'")?;
        let mut fields = Vec::new();
        let end = loop {
            self.newlines()?;
            let token = self.token()?;
            match token.kind {
                TokenKind::RightBrace => {
                    self.advance();
                    break token.span.end;
                }
                TokenKind::Eof => {
                    return Err(ParseError::ExpectedToken {
                        expected: "'}'",
                        span: token.span,
                    });
                }
                _ => {}
            }
            let (field, field_span) = self.name()?;
            self.require(TokenKind::Colon, "':'")?;
            let value = self.nested_expression()?;
            fields.push(FieldInit {
                name: field,
                span: Span {
                    start: field_span.start,
                    end: value.span.end,
                },
                value,
            });
            let token = self.token()?;
            if !matches!(token.kind, TokenKind::Newline | TokenKind::RightBrace) {
                return Err(ParseError::ExpectedToken {
                    expected: "newline between fields",
                    span: token.span,
                });
            }
        };
        Ok(Expr {
            kind: ExprKind::RecordInit { name, fields },
            span: Span {
                start: name_span.start,
                end,
            },
        })
    }
    fn operator(&self) -> Option<(BinaryOperator, u8)> {
        use BinaryOperator as B;
        Some(match self.current()?.kind {
            TokenKind::Equal => (B::Equal, 1),
            TokenKind::NotEqual => (B::NotEqual, 1),
            TokenKind::Less => (B::Less, 1),
            TokenKind::LessEqual => (B::LessEqual, 1),
            TokenKind::Greater => (B::Greater, 1),
            TokenKind::GreaterEqual => (B::GreaterEqual, 1),
            TokenKind::Plus => (B::Add, 2),
            TokenKind::Minus => (B::Subtract, 2),
            TokenKind::Star => (B::Multiply, 3),
            TokenKind::Slash => (B::Divide, 3),
            _ => return None,
        })
    }
    fn binary(&mut self, minimum: u8) -> Result<Expr, ParseError> {
        let mut left = self.atom()?;
        while let Some((operator, precedence)) = self.operator() {
            if precedence < minimum {
                break;
            }
            self.advance();
            let right = self.binary(precedence + 1)?;
            let span = Span {
                start: left.span.start,
                end: right.span.end,
            };
            left = Expr {
                kind: ExprKind::Binary {
                    left: Box::new(left),
                    operator,
                    right: Box::new(right),
                },
                span,
            };
        }
        Ok(left)
    }
    fn arguments(
        &mut self,
        closing: TokenKind,
        expected: &'static str,
    ) -> Result<(Vec<Expr>, usize), ParseError> {
        let mut arguments = Vec::new();
        self.newlines()?;
        if !self.at(&closing) {
            loop {
                arguments.push(self.nested_expression()?);
                self.newlines()?;
                if !self.at(&TokenKind::Comma) {
                    break;
                }
                self.advance();
                self.newlines()?;
            }
        }
        let end = self.require(closing, expected)?.span.end;
        Ok((arguments, end))
    }
    fn atom(&mut self) -> Result<Expr, ParseError> {
        let saved = self.position;
        let token = self.token()?;
        self.advance();
        let mut expression = match token.kind {
            TokenKind::Err => {
                self.require(TokenKind::Less, "'<'")?;
                let ty = self.annotation()?;
                let payload = ScalarType::from_type(ty).ok_or(ParseError::ExpectedToken {
                    expected: "int, float, bool, or string result payload",
                    span: token.span,
                })?;
                self.require(TokenKind::Greater, "'>'")?;
                self.require(TokenKind::LeftParen, "'('")?;
                let code = self.nested_expression()?;
                let end = self.require(TokenKind::RightParen, "')'")?.span.end;
                Expr {
                    kind: ExprKind::Error {
                        payload,
                        code: Box::new(code),
                    },
                    span: Span {
                        start: token.span.start,
                        end,
                    },
                }
            }
            TokenKind::Integer(value) => Expr {
                kind: ExprKind::Integer(value),
                span: token.span,
            },
            TokenKind::Float(value) => Expr {
                kind: ExprKind::Float(value),
                span: token.span,
            },
            TokenKind::String(value) => Expr {
                kind: ExprKind::String(value),
                span: token.span,
            },
            TokenKind::True | TokenKind::False => Expr {
                kind: ExprKind::Bool(token.kind == TokenKind::True),
                span: token.span,
            },
            TokenKind::Identifier(name) => {
                if self.at(&TokenKind::LeftBrace) && !self.in_condition {
                    self.record_init(name, token.span)?
                } else {
                    Expr {
                        kind: ExprKind::Identifier(name),
                        span: token.span,
                    }
                }
            }
            TokenKind::Minus => {
                let inner = self.binary(4)?;
                let span = Span {
                    start: token.span.start,
                    end: inner.span.end,
                };
                return Ok(Expr {
                    kind: ExprKind::Negate(Box::new(inner)),
                    span,
                });
            }
            TokenKind::LeftParen => {
                self.newlines()?;
                let mut inner = self.nested_expression()?;
                self.newlines()?;
                let end = self.require(TokenKind::RightParen, "')'")?.span.end;
                inner.span = Span {
                    start: token.span.start,
                    end,
                };
                inner
            }
            TokenKind::LeftBracket => {
                let (elements, end) = self.arguments(TokenKind::RightBracket, "']'")?;
                Expr {
                    kind: ExprKind::Array(elements),
                    span: Span {
                        start: token.span.start,
                        end,
                    },
                }
            }
            _ => {
                // Leave the unexpected token current, preserving the existing error contract.
                self.position = saved;
                return Err(ParseError::ExpectedExpression { span: token.span });
            }
        };
        loop {
            if self.at(&TokenKind::LeftParen) {
                let ExprKind::Identifier(name) = expression.kind else {
                    return Err(ParseError::ExpectedExpression {
                        span: expression.span,
                    });
                };
                self.advance();
                let (arguments, end) = self.arguments(TokenKind::RightParen, "')'")?;
                expression = Expr {
                    kind: ExprKind::Call { name, arguments },
                    span: Span {
                        start: expression.span.start,
                        end,
                    },
                };
            } else if self.at(&TokenKind::LeftBracket) {
                self.advance();
                let index = self.nested_expression()?;
                let end = self.require(TokenKind::RightBracket, "']'")?.span.end;
                let span = Span {
                    start: expression.span.start,
                    end,
                };
                expression = Expr {
                    kind: ExprKind::Index {
                        array: Box::new(expression),
                        index: Box::new(index),
                    },
                    span,
                };
            } else if self.at(&TokenKind::Dot) {
                self.advance();
                let (field, field_span) = self.name()?;
                let span = Span {
                    start: expression.span.start,
                    end: field_span.end,
                };
                expression = Expr {
                    kind: ExprKind::FieldAccess {
                        object: Box::new(expression),
                        field,
                    },
                    span,
                };
            } else {
                break;
            }
        }
        Ok(expression)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::token::TokenKind;

    #[test]
    fn parses_multiple_statements() {
        let mut parser = Parser::new(tokenize("assign x as 10\nshow x").unwrap());
        assert_eq!(
            parser.parse_program(),
            Ok(vec![
                Stmt {
                    kind: StmtKind::Set {
                        name: "x".to_owned(),
                        value: Expr {
                            kind: ExprKind::Integer("10".to_owned()),
                            span: Span { start: 12, end: 14 },
                        },
                    },
                    span: Span { start: 0, end: 14 },
                },
                Stmt {
                    kind: StmtKind::Show(Expr {
                        kind: ExprKind::Identifier("x".to_owned()),
                        span: Span { start: 20, end: 21 },
                    }),
                    span: Span { start: 15, end: 21 },
                },
            ])
        );
        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::Eof)
        );
    }

    #[test]
    fn parses_program_with_blank_lines() {
        let source = "\n\nshow 1\n\n\nshow 2\n\n";
        let mut parser = Parser::new(tokenize(source).unwrap());
        let statements = parser
            .parse_program()
            .expect("Blank lines should be allowed");
        assert_eq!(statements.len(), 2);
        assert_eq!(statements[0].span, Span { start: 2, end: 8 });
        assert_eq!(statements[1].span, Span { start: 11, end: 17 });
        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::Eof)
        );
    }

    #[test]
    fn parses_empty_program() {
        for source in ["", " \t", "\n\n"] {
            let mut parser = Parser::new(tokenize(source).unwrap());
            assert_eq!(parser.parse_program(), Ok(Vec::new()));
            assert_eq!(
                parser.current().map(|token| &token.kind),
                Some(&TokenKind::Eof)
            );
            parser.advance();
            assert_eq!(
                parser.current().map(|token| &token.kind),
                Some(&TokenKind::Eof)
            );
        }
    }

    #[test]
    fn parses_program_with_trailing_newline() {
        let mut parser = Parser::new(tokenize("show 42\n").unwrap());
        assert_eq!(
            parser.parse_program(),
            Ok(vec![Stmt {
                kind: StmtKind::Show(Expr {
                    kind: ExprKind::Integer("42".to_owned()),
                    span: Span { start: 5, end: 7 },
                }),
                span: Span { start: 0, end: 7 },
            }])
        );
        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::Eof)
        );
    }

    #[test]
    fn rejects_program_without_statement_separator() {
        for source in ["show 1 show 2", "show 1 assign x as 2", "show 1 2"] {
            let tokens = tokenize(source).unwrap();
            let unexpected = tokens[2].clone();
            let mut parser = Parser::new(tokens);
            assert_eq!(
                parser.parse_program(),
                Err(ParseError::ExpectedStatementSeparator {
                    span: unexpected.span,
                })
            );
            assert_eq!(parser.current(), Some(&unexpected));
        }
    }

    #[test]
    fn propagates_invalid_program_statement_errors() {
        for (source, error) in [
            (
                "show 1\n+",
                ParseError::ExpectedStatement {
                    span: Span { start: 7, end: 8 },
                },
            ),
            (
                "show 1\nassign 2",
                ParseError::ExpectedIdentifier {
                    span: Span { start: 14, end: 15 },
                },
            ),
            (
                "show 1\nshow",
                ParseError::ExpectedExpression {
                    span: Span { start: 11, end: 11 },
                },
            ),
        ] {
            let mut parser = Parser::new(tokenize(source).unwrap());
            assert_eq!(parser.parse_program(), Err(error));
        }
    }

    #[test]
    fn rejects_program_token_stream_without_eof() {
        assert_eq!(
            Parser::new(Vec::new()).parse_program(),
            Err(ParseError::UnexpectedEnd)
        );
        // A final newline must not loop forever when advance cannot move forward.
        let mut tokens = tokenize("\n").unwrap();
        tokens.pop();
        assert_eq!(
            Parser::new(tokens).parse_program(),
            Err(ParseError::UnexpectedEnd)
        );
    }

    #[test]
    fn parses_variable_declaration() {
        for (source, start) in [("assign x as 10", 0), ("  assign x as 10  ", 2)] {
            let tokens = tokenize(source).expect("Lexer should succeed");
            let mut parser = Parser::new(tokens);

            assert_eq!(
                parser.parse_statement(),
                Ok(Stmt {
                    kind: StmtKind::Set {
                        name: "x".to_owned(),
                        value: Expr {
                            kind: ExprKind::Integer("10".to_owned()),
                            span: Span {
                                start: start + 12,
                                end: start + 14,
                            },
                        },
                    },
                    span: Span {
                        start,
                        end: start + 14
                    },
                })
            );
            assert_eq!(
                parser.current().map(|token| &token.kind),
                Some(&TokenKind::Eof)
            );
        }
    }

    #[test]
    fn parses_variable_declaration_with_addition() {
        let tokens = tokenize("assign result as x + 20").expect("Lexer should succeed");
        let mut parser = Parser::new(tokens);

        assert_eq!(
            parser.parse_statement(),
            Ok(Stmt {
                kind: StmtKind::Set {
                    name: "result".to_owned(),
                    value: Expr {
                        kind: ExprKind::Binary {
                            left: Box::new(Expr {
                                kind: ExprKind::Identifier("x".to_owned()),
                                span: Span { start: 17, end: 18 },
                            }),
                            operator: BinaryOperator::Add,
                            right: Box::new(Expr {
                                kind: ExprKind::Integer("20".to_owned()),
                                span: Span { start: 21, end: 23 },
                            }),
                        },
                        span: Span { start: 17, end: 23 },
                    },
                },
                span: Span { start: 0, end: 23 },
            })
        );
        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::Eof)
        );
    }

    #[test]
    fn rejects_variable_declaration_without_identifier() {
        for (source, span, kind) in [
            (
                "assign 10",
                Span { start: 7, end: 9 },
                TokenKind::Integer("10".to_owned()),
            ),
            ("assign", Span { start: 6, end: 6 }, TokenKind::Eof),
        ] {
            let tokens = tokenize(source).expect("Lexer should succeed");
            let mut parser = Parser::new(tokens);

            assert_eq!(
                parser.parse_statement(),
                Err(ParseError::ExpectedIdentifier { span })
            );
            assert_eq!(parser.current().map(|token| &token.kind), Some(&kind));
        }
    }

    #[test]
    fn parses_declaration_and_reassignment() {
        for (source, name, value) in [
            ("assign x as 10", "x", ExprKind::Integer("10".to_owned())),
            ("assign x as 1.5", "x", ExprKind::Float("1.5".to_owned())),
        ] {
            let mut parser = Parser::new(tokenize(source).unwrap());
            let statement = parser.parse_statement().expect("declaration should parse");
            let StmtKind::Set {
                name: parsed,
                value: expr,
            } = statement.kind
            else {
                panic!("expected declaration for {source:?}");
            };
            assert_eq!((parsed.as_str(), expr.kind), (name, value));
        }
        let mut parser = Parser::new(tokenize("reassign x as 30").unwrap());
        let statement = parser.parse_statement().expect("reassignment should parse");
        let StmtKind::Assign { name, value } = statement.kind else {
            panic!("expected reassignment");
        };
        assert_eq!(
            (name.as_str(), value.kind),
            ("x", ExprKind::Integer("30".to_owned()))
        );
    }

    #[test]
    fn rejects_variable_declaration_without_initializer() {
        for (source, span, kind) in [
            (
                "assign x as show 10",
                Span { start: 12, end: 16 },
                TokenKind::Show,
            ),
            ("assign x as", Span { start: 11, end: 11 }, TokenKind::Eof),
        ] {
            let tokens = tokenize(source).expect("Lexer should succeed");
            let mut parser = Parser::new(tokens);

            assert_eq!(
                parser.parse_statement(),
                Err(ParseError::ExpectedExpression { span })
            );
            assert_eq!(parser.current().map(|token| &token.kind), Some(&kind));
        }
    }

    #[test]
    fn rejects_variable_declaration_without_valid_initializer() {
        for (source, span, kind) in [
            (
                "assign x as +",
                Span { start: 12, end: 13 },
                TokenKind::Plus,
            ),
            (
                "assign x as show",
                Span { start: 12, end: 16 },
                TokenKind::Show,
            ),
            (
                "assign x as 10 +",
                Span { start: 16, end: 16 },
                TokenKind::Eof,
            ),
        ] {
            let tokens = tokenize(source).expect("Lexer should succeed");
            let mut parser = Parser::new(tokens);

            assert_eq!(
                parser.parse_statement(),
                Err(ParseError::ExpectedExpression { span })
            );
            assert_eq!(parser.current().map(|token| &token.kind), Some(&kind));
        }
    }

    #[test]
    fn parses_show_statement() {
        for (source, expression, span) in [
            (
                "show 42",
                Expr {
                    kind: ExprKind::Integer("42".to_owned()),
                    span: Span { start: 5, end: 7 },
                },
                Span { start: 0, end: 7 },
            ),
            (
                "  show x + 20",
                Expr {
                    kind: ExprKind::Binary {
                        left: Box::new(Expr {
                            kind: ExprKind::Identifier("x".to_owned()),
                            span: Span { start: 7, end: 8 },
                        }),
                        operator: BinaryOperator::Add,
                        right: Box::new(Expr {
                            kind: ExprKind::Integer("20".to_owned()),
                            span: Span { start: 11, end: 13 },
                        }),
                    },
                    span: Span { start: 7, end: 13 },
                },
                Span { start: 2, end: 13 },
            ),
        ] {
            let tokens = tokenize(source).expect("Lexer should succeed");
            let mut parser = Parser::new(tokens);

            let statement = parser
                .parse_statement()
                .expect("Show statement should parse successfully");

            assert_eq!(
                statement,
                Stmt {
                    kind: StmtKind::Show(expression),
                    span,
                }
            );
            assert_eq!(
                parser.current().map(|token| &token.kind),
                Some(&TokenKind::Eof)
            );
        }

        for (source, expected) in [
            (
                "42",
                ParseError::ExpectedStatement {
                    span: Span { start: 0, end: 2 },
                },
            ),
            (
                "show",
                ParseError::ExpectedExpression {
                    span: Span { start: 4, end: 4 },
                },
            ),
            (
                "show +",
                ParseError::ExpectedExpression {
                    span: Span { start: 5, end: 6 },
                },
            ),
        ] {
            let tokens = tokenize(source).expect("Lexer should succeed");
            let mut parser = Parser::new(tokens);
            assert_eq!(parser.parse_statement(), Err(expected));
        }

        assert_eq!(
            Parser::new(Vec::new()).parse_statement(),
            Err(ParseError::UnexpectedEnd)
        );
    }

    #[test]
    fn parses_addition_expression() {
        let tokens = tokenize("10 + 20").expect("Lexer should succeed");
        let mut parser = Parser::new(tokens);

        let expression = parser
            .parse_expression()
            .expect("Addition should parse successfully");

        let expected = Expr {
            kind: ExprKind::Binary {
                left: Box::new(Expr {
                    kind: ExprKind::Integer("10".to_owned()),
                    span: Span { start: 0, end: 2 },
                }),
                operator: BinaryOperator::Add,
                right: Box::new(Expr {
                    kind: ExprKind::Integer("20".to_owned()),
                    span: Span { start: 5, end: 7 },
                }),
            },
            span: Span { start: 0, end: 7 },
        };

        assert_eq!(expression, expected);
        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::Eof)
        );

        // Chained additions group to the left and can include identifiers.
        for (source, kind, end) in [
            ("10 + 20 + 30", ExprKind::Integer("30".to_owned()), 12),
            (
                "10 + 20 + total",
                ExprKind::Identifier("total".to_owned()),
                15,
            ),
        ] {
            let tokens = tokenize(source).expect("Lexer should succeed");
            let mut parser = Parser::new(tokens);

            let expression = parser
                .parse_expression()
                .expect("Chained addition should parse successfully");

            assert_eq!(
                expression,
                Expr {
                    kind: ExprKind::Binary {
                        left: Box::new(expected.clone()),
                        operator: BinaryOperator::Add,
                        right: Box::new(Expr {
                            kind,
                            span: Span { start: 10, end },
                        }),
                    },
                    span: Span { start: 0, end },
                }
            );
            assert_eq!(
                parser.current().map(|token| &token.kind),
                Some(&TokenKind::Eof)
            );
        }
    }

    #[test]
    fn parses_identifier_expression() {
        let tokens = tokenize("total").expect("Lexer should succeed");
        let mut parser = Parser::new(tokens);

        let expression = parser
            .parse_expression()
            .expect("Identifier should parse successfully");

        assert_eq!(
            expression,
            Expr {
                kind: ExprKind::Identifier("total".to_owned()),
                span: Span { start: 0, end: 5 },
            }
        );

        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::Eof)
        );
    }

    #[test]
    fn parses_integer_expression() {
        let tokens = tokenize("42").expect("Lexer should succeed");
        let mut parser = Parser::new(tokens);

        let expression = parser
            .parse_expression()
            .expect("Integer should parse successfully");

        assert_eq!(
            expression,
            Expr {
                kind: ExprKind::Integer("42".to_owned()),
                span: Span { start: 0, end: 2 },
            }
        );

        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::Eof)
        );
    }

    #[test]
    fn advances_through_tokens() {
        let tokens = tokenize("{+}").expect("Lexer should succeed");
        let mut parser = Parser::new(tokens);

        parser.advance();

        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::Plus)
        );

        parser.advance();
        parser.advance();

        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::Eof)
        );

        // Advancing again should remain at EOF.
        parser.advance();

        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::Eof)
        );
    }

    #[test]
    fn starts_at_first_token() {
        let tokens = tokenize("{+}").expect("Lexer should succeed");

        let parser = Parser::new(tokens);

        assert_eq!(
            parser.current().map(|token| &token.kind),
            Some(&TokenKind::LeftBrace)
        );
    }
}
