use crate::ast::{BinaryOperator, Expr, ExprKind, Stmt, StmtKind};
use crate::symbol_table::{SymbolError, SymbolTable};
use crate::token::Span;
use crate::types::{MAX_ARRAY_LEN, NumericType, ScalarType, Type};
use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
pub enum TypeError {
    UndefinedVariable {
        name: String,
        span: Span,
    },
    DuplicateDeclaration {
        name: String,
        span: Span,
    },
    InvalidOperands {
        left: Type,
        right: Type,
        span: Span,
    },
    Mismatch {
        expected: Type,
        actual: Type,
        span: Span,
    },
    InvalidCondition {
        span: Span,
    },
    InvalidFunction {
        name: String,
        message: &'static str,
        span: Span,
    },
    InvalidArray {
        message: &'static str,
        span: Span,
    },
    InvalidLiteral {
        literal: String,
        span: Span,
    },
    InvalidReturn {
        span: Span,
    },
    MissingReturn {
        name: String,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub parameters: Vec<Type>,
    pub result: Type,
}

pub struct TypeChecker {
    symbols: SymbolTable,
    functions: HashMap<String, FunctionSignature>,
    result: Option<Type>,
    depth: usize,
}

pub fn reserved_function(name: &str) -> bool {
    matches!(
        name,
        "input"
            | "try_input"
            | "parse_int"
            | "parse_float"
            | "to_string"
            | "is_ok"
            | "is_eof"
            | "error_code"
            | "error_message"
            | "unwrap"
            | "ok"
            | "err"
            | "abs"
            | "sqrt"
            | "min"
            | "max"
            | "sum"
            | "length"
            | "main"
            | "printf"
            | "write"
            | "exit"
            | "fmin"
            | "fmax"
            | "fabs"
            | "malloc"
            | "free"
            | "strlen"
            | "memcpy"
            | "strcmp"
    ) || crate::runtime::FUNCTIONS.contains(&name)
}

impl TypeChecker {
    pub fn new(symbols: SymbolTable) -> Self {
        Self {
            symbols,
            functions: HashMap::new(),
            result: None,
            depth: 0,
        }
    }
    /// Checks statements in source order; successful earlier declarations are retained on error.
    pub fn check_program(&mut self, statements: &[Stmt]) -> Result<(), TypeError> {
        for stmt in statements {
            if let StmtKind::Function {
                name,
                parameters,
                return_type,
                ..
            } = &stmt.kind
            {
                if reserved_function(name) || self.functions.contains_key(name) {
                    return Err(TypeError::InvalidFunction {
                        name: name.clone(),
                        message: "duplicate or reserved function name",
                        span: stmt.span,
                    });
                }
                self.functions.insert(
                    name.clone(),
                    FunctionSignature {
                        parameters: parameters.iter().map(|p| p.ty).collect(),
                        result: *return_type,
                    },
                );
            }
        }
        for stmt in statements {
            self.check_statement(stmt)?;
        }
        Ok(())
    }
    fn block(&mut self, statements: &[Stmt]) -> Result<(), TypeError> {
        self.symbols.enter_scope();
        self.depth += 1;
        let result = statements
            .iter()
            .try_for_each(|statement| self.check_statement(statement));
        self.depth -= 1;
        self.symbols.exit_scope();
        result
    }
    fn mismatch(&self, expected: Type, value: &Expr) -> Result<(), TypeError> {
        let actual = self.infer_expression(value)?;
        if !expected.accepts(actual) {
            return Err(TypeError::Mismatch {
                expected,
                actual,
                span: value.span,
            });
        }
        Ok(())
    }
    pub fn check_statement(&mut self, stmt: &Stmt) -> Result<(), TypeError> {
        match &stmt.kind {
            StmtKind::Set { name, value } => {
                let ty = self.infer_expression(value)?;
                self.symbols
                    .declare(name.clone(), ty)
                    .map_err(|error| match error {
                        SymbolError::DuplicateDeclaration { name } => {
                            TypeError::DuplicateDeclaration {
                                name,
                                span: stmt.span,
                            }
                        }
                    })
            }
            StmtKind::Assign { name, value } => {
                let ty = self
                    .symbols
                    .lookup(name)
                    .ok_or_else(|| TypeError::UndefinedVariable {
                        name: name.clone(),
                        span: stmt.span,
                    })?;
                self.mismatch(ty, value)
            }
            StmtKind::Show(expr) => {
                let ty = self.infer_expression(expr)?;
                if matches!(ty, Type::Result(_)) {
                    return Err(TypeError::InvalidFunction {
                        name: "show".into(),
                        message: "unwrap a successful result or inspect error_message before printing",
                        span: expr.span,
                    });
                }
                if matches!(ty, Type::Array { .. }) {
                    return Err(TypeError::InvalidArray {
                        message: "show requires a scalar, not an array",
                        span: expr.span,
                    });
                }
                Ok(())
            }
            StmtKind::If {
                condition,
                then_body,
                else_body,
            } => {
                if self.infer_expression(condition)? != Type::Bool {
                    return Err(TypeError::InvalidCondition {
                        span: condition.span,
                    });
                }
                self.block(then_body)?;
                self.block(else_body)
            }
            StmtKind::While { condition, body } => {
                if self.infer_expression(condition)? != Type::Bool {
                    return Err(TypeError::InvalidCondition {
                        span: condition.span,
                    });
                }
                self.block(body)
            }
            StmtKind::Return(expr) => {
                let ty = self
                    .result
                    .ok_or(TypeError::InvalidReturn { span: stmt.span })?;
                self.mismatch(ty, expr)
            }
            StmtKind::Function {
                name,
                parameters,
                return_type,
                body,
            } => {
                if self.depth != 0 || self.result.is_some() {
                    return Err(TypeError::InvalidFunction {
                        name: name.clone(),
                        message: "functions must be top-level",
                        span: stmt.span,
                    });
                }
                let previous = std::mem::take(&mut self.symbols);
                self.result = Some(*return_type);
                let result = (|| {
                    for parameter in parameters {
                        self.symbols
                            .declare(parameter.name.clone(), parameter.ty)
                            .map_err(|_| TypeError::DuplicateDeclaration {
                                name: parameter.name.clone(),
                                span: parameter.span,
                            })?;
                    }
                    for statement in body {
                        self.check_statement(statement)?;
                    }
                    if !returns(body) {
                        return Err(TypeError::MissingReturn {
                            name: name.clone(),
                            span: stmt.span,
                        });
                    }
                    Ok(())
                })();
                self.symbols = previous;
                self.result = None;
                result
            }
        }
    }
    pub fn infer_expression(&self, expr: &Expr) -> Result<Type, TypeError> {
        match &expr.kind {
            ExprKind::Integer(_) => Ok(Type::Int),
            ExprKind::Float(literal) => {
                if literal.parse::<f64>().is_ok_and(f64::is_finite) {
                    Ok(Type::Float)
                } else {
                    Err(TypeError::InvalidLiteral {
                        literal: literal.clone(),
                        span: expr.span,
                    })
                }
            }
            ExprKind::Bool(_) => Ok(Type::Bool),
            ExprKind::String(_) => Ok(Type::String),
            ExprKind::Error { payload, code } => {
                self.mismatch(Type::Int, code)?;
                Ok(Type::Result(*payload))
            }
            ExprKind::Identifier(name) => {
                self.symbols
                    .lookup(name)
                    .ok_or_else(|| TypeError::UndefinedVariable {
                        name: name.clone(),
                        span: expr.span,
                    })
            }
            ExprKind::Negate(inner) => {
                let ty = self.infer_expression(inner)?;
                if ty.is_numeric() {
                    Ok(ty)
                } else {
                    Err(TypeError::InvalidOperands {
                        left: ty,
                        right: ty,
                        span: expr.span,
                    })
                }
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.infer_expression(left)?;
                let right = self.infer_expression(right)?;
                if left == Type::String && right == Type::String {
                    match operator {
                        BinaryOperator::Add => return Ok(Type::String),
                        BinaryOperator::Equal | BinaryOperator::NotEqual => return Ok(Type::Bool),
                        _ => {}
                    }
                }
                if left.is_numeric() && right.is_numeric() {
                    if operator.is_comparison() {
                        return Ok(Type::Bool);
                    }
                    return Ok(if left == Type::Float || right == Type::Float {
                        Type::Float
                    } else {
                        Type::Int
                    });
                }
                if left == Type::Bool
                    && right == Type::Bool
                    && matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual)
                {
                    return Ok(Type::Bool);
                }
                if let (Type::Array { .. }, Type::Array { .. }) = (left, right)
                    && left == right
                    && matches!(operator, BinaryOperator::Add | BinaryOperator::Multiply)
                {
                    return Ok(left);
                }
                Err(TypeError::InvalidOperands {
                    left,
                    right,
                    span: expr.span,
                })
            }
            ExprKind::Array(elements) => {
                if elements.is_empty() || elements.len() > MAX_ARRAY_LEN {
                    return Err(TypeError::InvalidArray {
                        message: "array literals require 1..=256 elements",
                        span: expr.span,
                    });
                }
                let element = match self.infer_expression(&elements[0])? {
                    Type::Int => NumericType::Int,
                    Type::Float => NumericType::Float,
                    _ => {
                        return Err(TypeError::InvalidArray {
                            message: "arrays require Int or Float elements",
                            span: expr.span,
                        });
                    }
                };
                for value in &elements[1..] {
                    if self.infer_expression(value)? != element.scalar() {
                        return Err(TypeError::InvalidArray {
                            message: "array elements must have identical numeric types",
                            span: value.span,
                        });
                    }
                }
                Ok(Type::Array {
                    element,
                    len: elements.len(),
                })
            }
            ExprKind::Index { array, index } => {
                let ty = self.infer_expression(array)?;
                if self.infer_expression(index)? != Type::Int {
                    return Err(TypeError::InvalidArray {
                        message: "array index must be Int",
                        span: index.span,
                    });
                }
                if let Type::Array { element, .. } = ty {
                    Ok(element.scalar())
                } else {
                    Err(TypeError::InvalidArray {
                        message: "indexing requires an array",
                        span: array.span,
                    })
                }
            }
            ExprKind::Call { name, arguments } => self.call(name, arguments, expr.span),
        }
    }
    fn call(&self, name: &str, arguments: &[Expr], span: Span) -> Result<Type, TypeError> {
        let types = arguments
            .iter()
            .map(|expr| self.infer_expression(expr))
            .collect::<Result<Vec<_>, _>>()?;
        let invalid = || TypeError::InvalidFunction {
            name: name.to_owned(),
            message: "invalid argument count or types",
            span,
        };
        match (name, types.as_slice()) {
            ("input", [] | [Type::String]) => Ok(Type::String),
            ("try_input", [] | [Type::String]) => Ok(Type::Result(ScalarType::String)),
            ("parse_int", [Type::String]) => Ok(Type::Result(ScalarType::Int)),
            ("parse_float", [Type::String]) => Ok(Type::Result(ScalarType::Float)),
            ("to_string", [ty]) if ScalarType::from_type(*ty).is_some() => Ok(Type::String),
            ("ok", [ty]) if ScalarType::from_type(*ty).is_some() => Ok(Type::Result(
                ScalarType::from_type(*ty).expect("checked scalar"),
            )),
            ("is_ok" | "is_eof", [Type::Result(_)]) => Ok(Type::Bool),
            ("error_code", [Type::Result(_)]) => Ok(Type::Int),
            ("error_message", [Type::Result(_)]) => Ok(Type::String),
            ("unwrap", [Type::Result(payload)]) => Ok(payload.ty()),
            (
                "input" | "try_input" | "parse_int" | "parse_float" | "to_string" | "ok" | "is_ok"
                | "is_eof" | "error_code" | "error_message" | "unwrap",
                _,
            ) => Err(invalid()),
            ("length", [Type::Array { .. }]) => Ok(Type::Int),
            ("length", [Type::String]) => Ok(Type::Int),
            ("sum", [Type::Array { element, .. }]) => Ok(element.scalar()),
            ("sqrt", [Type::Float]) => Ok(Type::Float),
            ("abs", [ty]) if ty.is_numeric() => Ok(*ty),
            ("min" | "max", [left, right]) if left.is_numeric() && right.is_numeric() => {
                Ok(if *left == Type::Float || *right == Type::Float {
                    Type::Float
                } else {
                    Type::Int
                })
            }
            ("length" | "sum" | "sqrt" | "abs" | "min" | "max", _) => Err(invalid()),
            _ => {
                let signature =
                    self.functions
                        .get(name)
                        .ok_or_else(|| TypeError::InvalidFunction {
                            name: name.to_owned(),
                            message: "undefined function",
                            span,
                        })?;
                if signature.parameters.len() != types.len()
                    || !signature
                        .parameters
                        .iter()
                        .zip(&types)
                        .all(|(expected, actual)| expected.accepts(*actual))
                {
                    return Err(invalid());
                }
                Ok(signature.result)
            }
        }
    }
}

fn returns(statements: &[Stmt]) -> bool {
    statements.iter().any(|statement| match &statement.kind {
        StmtKind::Return(_) => true,
        StmtKind::If {
            then_body,
            else_body,
            ..
        } => returns(then_body) && returns(else_body),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::Parser;

    #[test]
    fn assignment_diagnostic_preserves_rhs_span() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        let statements = parse_program("assign x as 1\nreassign x as true");
        assert_eq!(
            checker.check_program(&statements),
            Err(TypeError::Mismatch {
                expected: Type::Int,
                actual: Type::Bool,
                span: Span { start: 28, end: 32 },
            })
        );
    }

    #[test]
    fn failing_block_restores_symbol_scope() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        let statements =
            parse_program("assign x as 1\nif true {\nassign x as 2.0\nshow missing\n}");
        assert!(checker.check_program(&statements).is_err());
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
    }

    #[test]
    fn function_signature_rejects_wrong_return_type() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        assert!(matches!(
            checker.check_program(&parse_program("function f() be bool { return 1 }")),
            Err(TypeError::Mismatch {
                expected: Type::Bool,
                actual: Type::Int,
                ..
            })
        ));
    }

    fn parse_expression(source: &str) -> Expr {
        let tokens = tokenize(source).expect("Expression should tokenize");
        Parser::new(tokens)
            .parse_expression()
            .expect("Expression should parse")
    }

    fn parse_statement(source: &str) -> Stmt {
        let tokens = tokenize(source).expect("Statement should tokenize");
        Parser::new(tokens)
            .parse_statement()
            .expect("Statement should parse")
    }

    fn parse_program(source: &str) -> Vec<Stmt> {
        let tokens = tokenize(source).expect("Program should tokenize");
        Parser::new(tokens)
            .parse_program()
            .expect("Program should parse")
    }

    #[test]
    fn checks_valid_program() {
        let statements = parse_program("assign x as 10\nassign y as x + 20\nshow y");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(checker.check_program(&statements), Ok(()));
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        assert_eq!(checker.symbols.lookup("y"), Some(Type::Int));
    }

    #[test]
    fn checks_empty_program() {
        let statements = parse_program("");
        let mut symbols = SymbolTable::new();
        symbols
            .declare("existing".to_owned(), Type::Int)
            .expect("Declaration should succeed");
        let mut checker = TypeChecker::new(symbols);

        assert_eq!(checker.check_program(&statements), Ok(()));
        assert_eq!(checker.symbols.lookup("existing"), Some(Type::Int));
        assert_eq!(
            TypeChecker::new(SymbolTable::new()).check_program(&[]),
            Ok(())
        );
    }

    #[test]
    fn checks_program_declaration_using_previous_variable() {
        let statements = parse_program("assign x as 10\nassign y as x\nassign z as y + x");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(checker.check_program(&statements), Ok(()));
        for name in ["x", "y", "z"] {
            assert_eq!(checker.symbols.lookup(name), Some(Type::Int));
        }
    }

    #[test]
    fn rejects_program_with_undefined_variable() {
        let statements = parse_program("assign x as 10\nshow unknown");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_program(&statements),
            Err(TypeError::UndefinedVariable {
                name: "unknown".to_owned(),
                span: Span { start: 20, end: 27 },
            })
        );
    }

    #[test]
    fn rejects_program_with_duplicate_declaration() {
        let statements = parse_program("assign x as 10\nassign x as 20\nassign later as 30");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_program(&statements),
            Err(TypeError::DuplicateDeclaration {
                name: "x".to_owned(),
                span: Span { start: 15, end: 29 },
            })
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        assert_eq!(checker.symbols.lookup("later"), None);
    }

    #[test]
    fn rejects_program_reference_before_declaration() {
        let statements = parse_program("show x\nassign x as 10");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_program(&statements),
            Err(TypeError::UndefinedVariable {
                name: "x".to_owned(),
                span: Span { start: 5, end: 6 },
            })
        );
        assert_eq!(checker.symbols.lookup("x"), None);
    }

    #[test]
    fn program_stops_at_first_error_and_keeps_previous_declarations() {
        let statements =
            parse_program("assign x as 10\nassign y as unknown\nassign later as 20\nshow other");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_program(&statements),
            Err(TypeError::UndefinedVariable {
                name: "unknown".to_owned(),
                span: Span { start: 27, end: 34 },
            })
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        assert_eq!(checker.symbols.lookup("y"), None);
        assert_eq!(checker.symbols.lookup("later"), None);
    }

    #[test]
    fn checks_declaration_and_subsequent_reference() {
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_statement(&parse_statement("assign x as 10")),
            Ok(())
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        assert_eq!(
            checker.infer_expression(&parse_expression("x")),
            Ok(Type::Int)
        );
    }

    #[test]
    fn checks_declaration_using_previously_declared_variable() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        checker
            .check_statement(&parse_statement("assign x as 10"))
            .expect("First declaration should succeed");

        assert_eq!(
            checker.check_statement(&parse_statement("assign y as x + 20")),
            Ok(())
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        assert_eq!(checker.symbols.lookup("y"), Some(Type::Int));
    }

    #[test]
    fn checks_show_without_changing_symbols() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        checker
            .check_statement(&parse_statement("assign x as 10"))
            .expect("Declaration should succeed");

        for source in ["show 42", "show x", "show x + 20"] {
            assert_eq!(checker.check_statement(&parse_statement(source)), Ok(()));
            assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        }
        // Output does not create a declaration or replace the existing one.
        assert_eq!(checker.symbols.lookup("unknown"), None);
        assert_eq!(
            checker.check_statement(&parse_statement("assign x as 20")),
            Err(TypeError::DuplicateDeclaration {
                name: "x".to_owned(),
                span: Span { start: 0, end: 14 },
            })
        );
    }

    #[test]
    fn rejects_duplicate_declaration_with_statement_span() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        checker
            .check_statement(&parse_statement("assign x as 10"))
            .expect("First declaration should succeed");

        assert_eq!(
            checker.check_statement(&parse_statement("  assign x as 20")),
            Err(TypeError::DuplicateDeclaration {
                name: "x".to_owned(),
                span: Span { start: 2, end: 16 },
            })
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
    }

    #[test]
    fn rejects_show_with_undefined_variable() {
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_statement(&parse_statement("show unknown + 10")),
            Err(TypeError::UndefinedVariable {
                name: "unknown".to_owned(),
                span: Span { start: 5, end: 12 },
            })
        );
        assert_eq!(checker.symbols.lookup("unknown"), None);
    }

    #[test]
    fn failed_initializer_does_not_register_variable() {
        let mut checker = TypeChecker::new(SymbolTable::new());

        for (source, name, undefined, span) in [
            (
                "assign x as unknown + 10",
                "x",
                "unknown",
                Span { start: 12, end: 19 },
            ),
            ("assign y as y + 10", "y", "y", Span { start: 12, end: 13 }),
        ] {
            assert_eq!(
                checker.check_statement(&parse_statement(source)),
                Err(TypeError::UndefinedVariable {
                    name: undefined.to_owned(),
                    span,
                })
            );
            assert_eq!(checker.symbols.lookup(name), None);
        }

        assert_eq!(
            checker.check_statement(&parse_statement("assign x as 10")),
            Ok(())
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
    }

    #[test]
    fn infers_integer_literal_type() {
        let checker = TypeChecker::new(SymbolTable::new());
        let expression = parse_expression("42");

        assert_eq!(checker.infer_expression(&expression), Ok(Type::Int));
    }

    #[test]
    fn infers_declared_identifier_type() {
        let mut symbols = SymbolTable::new();
        symbols
            .declare("total".to_owned(), Type::Int)
            .expect("Declaration should succeed");
        let checker = TypeChecker::new(symbols);

        assert_eq!(
            checker.infer_expression(&parse_expression("total")),
            Ok(Type::Int)
        );
    }

    #[test]
    fn reports_undefined_identifier_with_span() {
        let checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.infer_expression(&parse_expression("  unknown")),
            Err(TypeError::UndefinedVariable {
                name: "unknown".to_owned(),
                span: Span { start: 2, end: 9 },
            })
        );
    }

    #[test]
    fn infers_integer_addition_type() {
        let checker = TypeChecker::new(SymbolTable::new());

        for source in ["10 + 20", "10 + 20 + 30"] {
            assert_eq!(
                checker.infer_expression(&parse_expression(source)),
                Ok(Type::Int)
            );
        }
    }

    #[test]
    fn infers_addition_with_declared_identifier() {
        let mut symbols = SymbolTable::new();
        symbols
            .declare("x".to_owned(), Type::Int)
            .expect("Declaration should succeed");
        let checker = TypeChecker::new(symbols);

        for source in ["x + 20", "20 + x", "x + x + 20"] {
            assert_eq!(
                checker.infer_expression(&parse_expression(source)),
                Ok(Type::Int)
            );
        }
    }

    #[test]
    fn propagates_undefined_addition_operand_errors() {
        let checker = TypeChecker::new(SymbolTable::new());

        for (source, span) in [
            ("unknown + 10", Span { start: 0, end: 7 }),
            ("10 + unknown", Span { start: 5, end: 12 }),
            ("10 + 20 + unknown", Span { start: 10, end: 17 }),
            ("10 + unknown + 20", Span { start: 5, end: 12 }),
        ] {
            assert_eq!(
                checker.infer_expression(&parse_expression(source)),
                Err(TypeError::UndefinedVariable {
                    name: "unknown".to_owned(),
                    span,
                })
            );
        }
    }
}
