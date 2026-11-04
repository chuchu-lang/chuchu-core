use std::collections::HashMap;
use std::fmt;

use inkwell::AddressSpace;
use inkwell::builder::{Builder, BuilderError};
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::values::IntValue;

use crate::ast::{BinaryOperator, Expr, ExprKind, Stmt, StmtKind};
use crate::token::Span;

mod program;
mod records;

pub use records::{FieldLayout, RecordLayout, RecordLayouts};

#[derive(Debug)]
pub enum CodegenError {
    BuildReturn(BuilderError),
    Verification(String),
    InvalidIntegerLiteral { literal: String, span: Span },
    UnsupportedExpression { span: Span },
    InvalidInsertionPoint { span: Span },
    BuildAddition { source: BuilderError, span: Span },
    UndefinedVariable { name: String, span: Span },
    DuplicateDeclaration { name: String, span: Span },
    UnsupportedStatement { span: Span },
    BuildOutput { source: BuilderError, span: Span },
    InvalidRuntimeDeclaration,
    Build { source: BuilderError, span: Span },
    InvalidType { message: &'static str, span: Span },
    InvalidFloatLiteral { literal: String, span: Span },
}

impl fmt::Display for CodegenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuildReturn(error) => {
                write!(formatter, "Failed to build return instruction: {error}")
            }
            Self::Verification(message) => {
                write!(formatter, "LLVM module verification failed: {message}")
            }
            Self::InvalidIntegerLiteral { literal, span } => write!(
                formatter,
                "Invalid or out-of-range signed 64-bit integer literal {literal:?} at {}..{}",
                span.start, span.end
            ),
            Self::UnsupportedExpression { span } => write!(
                formatter,
                "Unsupported expression for LLVM code generation at {}..{}",
                span.start, span.end
            ),
            Self::InvalidInsertionPoint { span } => write!(
                formatter,
                "Code generation requires a builder positioned in an unterminated function block at {}..{}",
                span.start, span.end
            ),
            Self::BuildAddition { source, span } => write!(
                formatter,
                "Failed to build addition at {}..{}: {source}",
                span.start, span.end
            ),
            Self::UndefinedVariable { name, span } => write!(
                formatter,
                "Undefined variable {name:?} at {}..{}",
                span.start, span.end
            ),
            Self::DuplicateDeclaration { name, span } => write!(
                formatter,
                "Duplicate declaration of {name:?} at {}..{}",
                span.start, span.end
            ),
            Self::UnsupportedStatement { span } => write!(
                formatter,
                "Unsupported statement for LLVM code generation at {}..{}",
                span.start, span.end
            ),
            Self::BuildOutput { source, span } => write!(
                formatter,
                "Failed to build output at {}..{}: {source}",
                span.start, span.end
            ),
            Self::InvalidRuntimeDeclaration => write!(
                formatter,
                "Existing printf declaration has an incompatible type"
            ),
            Self::Build { source, span } => write!(
                formatter,
                "LLVM builder error at {}..{}: {source}",
                span.start, span.end
            ),
            Self::InvalidType { message, span } => {
                write!(formatter, "{message} at {}..{}", span.start, span.end)
            }
            Self::InvalidFloatLiteral { literal, span } => write!(
                formatter,
                "Invalid or non-finite Float literal {literal:?} at {}..{}",
                span.start, span.end
            ),
        }
    }
}

impl std::error::Error for CodegenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BuildReturn(error) => Some(error),
            Self::BuildAddition { source, .. } => Some(source),
            Self::BuildOutput { source, .. } => Some(source),
            Self::Build { source, .. } => Some(source),
            Self::Verification(_)
            | Self::InvalidIntegerLiteral { .. }
            | Self::UnsupportedExpression { .. }
            | Self::InvalidInsertionPoint { .. }
            | Self::UndefinedVariable { .. }
            | Self::DuplicateDeclaration { .. }
            | Self::UnsupportedStatement { .. }
            | Self::InvalidRuntimeDeclaration
            | Self::InvalidType { .. }
            | Self::InvalidFloatLiteral { .. } => None,
        }
    }
}

/// Low-level utility for integer literals, SSA references, and raw LLVM additions.
/// Source-language checked arithmetic and typed storage are implemented by `generate_program`.
/// Bindings must belong to the same LLVM context and current function as the builder.
/// Additions require the builder to be positioned in an unterminated function block.
/// Signed i64 is a provisional backend representation, not a language-level width guarantee.
/// This low-level utility sets neither `nsw` nor `nuw`; it does not implement runtime checks.
pub fn generate_expression<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    bindings: &HashMap<String, IntValue<'ctx>>,
    expr: &Expr,
) -> Result<IntValue<'ctx>, CodegenError> {
    match &expr.kind {
        ExprKind::Integer(literal) => {
            let value =
                literal
                    .parse::<i64>()
                    .map_err(|_| CodegenError::InvalidIntegerLiteral {
                        literal: literal.clone(),
                        span: expr.span,
                    })?;
            Ok(context.i64_type().const_int(value.cast_unsigned(), true))
        }
        ExprKind::Binary {
            left,
            operator: BinaryOperator::Add,
            right,
        } => {
            validate_insertion_point(builder, expr.span)?;
            let left = generate_expression(context, builder, bindings, left)?;
            let right = generate_expression(context, builder, bindings, right)?;
            builder.build_int_add(left, right, "add").map_err(|source| {
                CodegenError::BuildAddition {
                    source,
                    span: expr.span,
                }
            })
        }
        ExprKind::Identifier(name) => {
            bindings
                .get(name)
                .copied()
                .ok_or_else(|| CodegenError::UndefinedVariable {
                    name: name.clone(),
                    span: expr.span,
                })
        }
        _ => Err(CodegenError::UnsupportedExpression { span: expr.span }),
    }
}

fn validate_insertion_point(builder: &Builder<'_>, span: Span) -> Result<(), CodegenError> {
    let block = builder
        .get_insert_block()
        .ok_or(CodegenError::InvalidInsertionPoint { span })?;
    if block.get_parent().is_none() || block.get_terminator().is_some() {
        return Err(CodegenError::InvalidInsertionPoint { span });
    }
    Ok(())
}

/// Binds an immutable declaration to its initializer's SSA value without stack storage.
/// The caller owns one binding environment per function; failed initializers add no binding.
pub fn generate_declaration<'ctx>(
    context: &'ctx Context,
    builder: &Builder<'ctx>,
    bindings: &mut HashMap<String, IntValue<'ctx>>,
    stmt: &Stmt,
) -> Result<(), CodegenError> {
    let StmtKind::Set { name, value } = &stmt.kind else {
        return Err(CodegenError::UnsupportedStatement { span: stmt.span });
    };

    if bindings.contains_key(name) {
        return Err(CodegenError::DuplicateDeclaration {
            name: name.clone(),
            span: stmt.span,
        });
    }

    let value = generate_expression(context, builder, bindings, value)?;
    bindings.insert(name.clone(), value);
    Ok(())
}

/// Emits one declaration or signed integer output statement.
/// C `long long` is 64 bits on the supported macOS host ABI, matching LLVM i64.
pub fn generate_statement<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder<'ctx>,
    bindings: &mut HashMap<String, IntValue<'ctx>>,
    stmt: &Stmt,
) -> Result<(), CodegenError> {
    match &stmt.kind {
        StmtKind::Set { .. } => generate_declaration(context, builder, bindings, stmt),
        StmtKind::Show(expr) => {
            validate_insertion_point(builder, stmt.span)?;
            let value = generate_expression(context, builder, bindings, expr)?;
            let printf_type = context
                .i32_type()
                .fn_type(&[context.ptr_type(AddressSpace::default()).into()], true);
            let printf = match module.get_function("printf") {
                Some(function) if function.get_type() == printf_type => function,
                Some(_) => return Err(CodegenError::InvalidRuntimeDeclaration),
                None => module.add_function("printf", printf_type, None),
            };
            let format = match module.get_global(".cheeku.format.i64") {
                Some(global) => global,
                None => builder
                    .build_global_string_ptr("%lld\n", ".cheeku.format.i64")
                    .map_err(|source| CodegenError::BuildOutput {
                        source,
                        span: stmt.span,
                    })?,
            };
            builder
                .build_call(
                    printf,
                    &[format.as_pointer_value().into(), value.into()],
                    "printf_result",
                )
                .map_err(|source| CodegenError::BuildOutput {
                    source,
                    span: stmt.span,
                })?;
            Ok(())
        }
        _ => Err(CodegenError::UnsupportedStatement { span: stmt.span }),
    }
}

/// Generates a verified native main function from a semantically validated AST.
pub fn generate_program<'ctx>(
    context: &'ctx Context,
    statements: &[Stmt],
) -> Result<Module<'ctx>, CodegenError> {
    program::generate(context, statements)
}

/// Production lowering with optional source-level native debug metadata.
pub fn generate_program_with_options<'ctx>(
    context: &'ctx Context,
    statements: &[Stmt],
    source: Option<&crate::source::SourceFile>,
    options: &crate::optimization::BuildOptions,
) -> Result<Module<'ctx>, CodegenError> {
    program::generate_with_options(context, statements, source, options)
}

/// Lowering for a program the type checker accepted, with the validated record
/// metadata from [`crate::type_checker::TypeChecker::record_table`]. Record
/// declarations are not executable statements: each is checked against `records`
/// and then generates no code. Anything that would create, read, write, or pass a
/// record is still rejected.
pub fn generate_program_with_records<'ctx>(
    context: &'ctx Context,
    statements: &[Stmt],
    records: &crate::records::RecordTable,
    source: Option<&crate::source::SourceFile>,
    options: &crate::optimization::BuildOptions,
) -> Result<Module<'ctx>, CodegenError> {
    program::generate_with_records(context, statements, records, source, options)
}

/// Generates a verified module containing `main`, which returns an i32 zero.
/// The returned module borrows the caller's LLVM context.
pub fn generate_module<'ctx>(context: &'ctx Context) -> Result<Module<'ctx>, CodegenError> {
    let module = context.create_module("cheeku");
    let builder = context.create_builder();
    let int_type = context.i32_type();
    let main = module.add_function("main", int_type.fn_type(&[], false), None);
    let entry = context.append_basic_block(main, "entry");
    builder.position_at_end(entry);

    builder
        .build_return(Some(&int_type.const_zero()))
        .map_err(CodegenError::BuildReturn)?;
    module
        .verify()
        .map_err(|message| CodegenError::Verification(message.to_string()))?;

    Ok(module)
}
