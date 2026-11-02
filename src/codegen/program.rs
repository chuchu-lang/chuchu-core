//! Typed native lowering. Mutable locals and bounded array temporaries live in entry-block
//! allocations; lexical scopes track storage independently of semantic symbol tables.
use std::collections::HashMap;

use inkwell::builder::{Builder, BuilderError};
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::{BasicType, BasicTypeEnum, FunctionType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FloatValue, FunctionValue, IntValue, PointerValue,
};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate};

use super::CodegenError;

mod arrays;
mod debug;
mod results;
mod runtime;
mod strings;
use crate::ast::{BinaryOperator as Op, Expr, ExprKind, Stmt, StmtKind};
use crate::token::Span;
use crate::types::{MAX_ARRAY_LEN, NumericType, ScalarType, Type};

fn build<T>(result: Result<T, BuilderError>, span: Span) -> Result<T, CodegenError> {
    result.map_err(|source| CodegenError::Build { source, span })
}
fn invalid(message: &'static str, span: Span) -> CodegenError {
    CodegenError::InvalidType { message, span }
}

#[derive(Clone, Copy)]
struct Value<'ctx> {
    raw: BasicValueEnum<'ctx>,
    ty: Type,
}
impl<'ctx> Value<'ctx> {
    fn int(self, span: Span) -> Result<IntValue<'ctx>, CodegenError> {
        if let BasicValueEnum::IntValue(value) = self.raw {
            Ok(value)
        } else {
            Err(invalid("Expected integer LLVM value", span))
        }
    }
    fn float(self, span: Span) -> Result<FloatValue<'ctx>, CodegenError> {
        if let BasicValueEnum::FloatValue(value) = self.raw {
            Ok(value)
        } else {
            Err(invalid("Expected Float LLVM value", span))
        }
    }
}
#[derive(Clone, Copy)]
struct Binding<'ctx> {
    pointer: PointerValue<'ctx>,
    ty: Type,
}
#[derive(Clone)]
struct Function<'ctx> {
    value: FunctionValue<'ctx>,
    parameters: Vec<Type>,
    result: Type,
}

struct Generator<'ctx> {
    context: &'ctx Context,
    // The DI builder finalizes on drop and must be disposed before its module,
    // including when lowering returns an error. Rust drops fields in this order.
    debug: Option<debug::DebugState<'ctx>>,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    scopes: Vec<HashMap<String, Binding<'ctx>>>,
    functions: HashMap<String, Function<'ctx>>,
    result: Type,
}

pub(super) fn generate<'ctx>(
    context: &'ctx Context,
    statements: &[Stmt],
) -> Result<Module<'ctx>, CodegenError> {
    generate_with_options(
        context,
        statements,
        None,
        &crate::optimization::BuildOptions::default(),
    )
}

pub(super) fn generate_with_options<'ctx>(
    context: &'ctx Context,
    statements: &[Stmt],
    source: Option<&crate::source::SourceFile>,
    options: &crate::optimization::BuildOptions,
) -> Result<Module<'ctx>, CodegenError> {
    let module = context.create_module("cheeku");
    let debug = if options.debug_info == crate::optimization::DebugInfo::Full {
        Some(debug::DebugState::new(
            context,
            &module,
            source.ok_or(invalid(
                "Debug generation requires named source",
                Span { start: 0, end: 0 },
            ))?,
            options,
        )?)
    } else {
        None
    };
    let mut generator = Generator {
        context,
        module,
        debug,
        builder: context.create_builder(),
        scopes: vec![HashMap::new()],
        functions: HashMap::new(),
        result: Type::Int,
    };
    for statement in statements {
        if let StmtKind::Function {
            name,
            parameters,
            return_type,
            ..
        } = &statement.kind
        {
            if generator.functions.contains_key(name)
                || crate::type_checker::reserved_function(name)
            {
                return Err(CodegenError::DuplicateDeclaration {
                    name: name.clone(),
                    span: statement.span,
                });
            }
            let params = parameters
                .iter()
                .map(|p| generator.llvm_type(p.ty, p.span).map(Into::into))
                .collect::<Result<Vec<_>, _>>()?;
            let result_type = generator.llvm_type(*return_type, statement.span)?;
            let value = generator.module.add_function(
                &format!("chuchu.{name}"),
                result_type.fn_type(&params, false),
                None,
            );
            generator.functions.insert(
                name.clone(),
                Function {
                    value,
                    parameters: parameters.iter().map(|p| p.ty).collect(),
                    result: *return_type,
                },
            );
        }
    }
    for statement in statements {
        if let StmtKind::Function {
            name,
            parameters,
            return_type,
            body,
        } = &statement.kind
        {
            let function = generator
                .functions
                .get(name)
                .ok_or(invalid("Missing function declaration", statement.span))?
                .value;
            generator.scopes = vec![HashMap::new()];
            generator.result = *return_type;
            generator
                .builder
                .position_at_end(context.append_basic_block(function, "entry"));
            generator.builder.unset_current_debug_location();
            if generator.debug.is_some() {
                let result = generator.llvm_type(*return_type, statement.span)?;
                let params = parameters
                    .iter()
                    .map(|p| generator.llvm_type(p.ty, p.span).map(|llvm| (p.ty, llvm)))
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(debug) = &mut generator.debug {
                    debug.function(
                        function,
                        name,
                        statement.span,
                        Some((*return_type, result)),
                        &params,
                    )?;
                }
            }
            for (index, parameter) in parameters.iter().enumerate() {
                let value = function
                    .get_nth_param(
                        u32::try_from(index)
                            .map_err(|_| invalid("Too many parameters", parameter.span))?,
                    )
                    .ok_or(invalid("Missing LLVM parameter", parameter.span))?;
                generator.declare(
                    &parameter.name,
                    Value {
                        raw: value,
                        ty: parameter.ty,
                    },
                    parameter.span,
                    Some(
                        u32::try_from(index + 1)
                            .map_err(|_| invalid("Too many parameters", parameter.span))?,
                    ),
                )?;
            }
            generator.statements(body)?;
            if !generator.terminated() {
                return Err(invalid(
                    "Function may finish without returning",
                    statement.span,
                ));
            }
        }
    }
    generator.scopes = vec![HashMap::new()];
    generator.result = Type::Int;
    let main = generator
        .module
        .add_function("main", context.i32_type().fn_type(&[], false), None);
    generator
        .builder
        .position_at_end(context.append_basic_block(main, "entry"));
    generator.builder.unset_current_debug_location();
    if let Some(debug) = &mut generator.debug {
        let span = statements
            .iter()
            .find(|s| !matches!(s.kind, StmtKind::Function { .. }))
            .map(|s| s.span)
            .unwrap_or(Span { start: 0, end: 0 });
        debug.function(main, "main", span, None, &[])?;
    }
    for statement in statements {
        if !matches!(statement.kind, StmtKind::Function { .. }) {
            generator.statement(statement)?;
        }
    }
    if !generator.terminated() {
        generator.cleanup_strings(
            true,
            statements
                .last()
                .map(|s| s.span)
                .unwrap_or(Span { start: 0, end: 0 }),
        )?;
        generator.builder.unset_current_debug_location();
        build(
            generator
                .builder
                .build_return(Some(&context.i32_type().const_zero())),
            Span { start: 0, end: 0 },
        )?;
    }
    if let Some(debug) = &generator.debug {
        debug.builder.finalize();
    }
    generator
        .module
        .verify()
        .map_err(|error| CodegenError::Verification(error.to_string()))?;
    Ok(generator.module)
}

impl<'ctx> Generator<'ctx> {
    fn llvm_type(&self, ty: Type, span: Span) -> Result<BasicTypeEnum<'ctx>, CodegenError> {
        Ok(match ty {
            Type::Int => self.context.i64_type().into(),
            Type::Float => self.context.f64_type().into(),
            Type::Bool => self.context.bool_type().into(),
            Type::String => self.context.ptr_type(AddressSpace::default()).into(),
            Type::Result(payload) => self
                .context
                .struct_type(
                    &[
                        self.context.i32_type().into(),
                        self.llvm_type(payload.ty(), span)?,
                    ],
                    false,
                )
                .into(),
            Type::Array { element, len } => {
                if len == 0 || len > MAX_ARRAY_LEN {
                    return Err(invalid("Array length outside 1..=256", span));
                }
                self.llvm_type(element.scalar(), span)?
                    .array_type(u32::try_from(len).map_err(|_| invalid("Array too large", span))?)
                    .into()
            }
        })
    }
    fn function(&self, span: Span) -> Result<FunctionValue<'ctx>, CodegenError> {
        self.builder
            .get_insert_block()
            .and_then(|block| block.get_parent())
            .ok_or(CodegenError::InvalidInsertionPoint { span })
    }
    fn terminated(&self) -> bool {
        self.builder
            .get_insert_block()
            .is_some_and(|block| block.get_terminator().is_some())
    }
    fn allocate(&self, ty: Type, span: Span) -> Result<PointerValue<'ctx>, CodegenError> {
        let entry = self
            .function(span)?
            .get_first_basic_block()
            .ok_or(CodegenError::InvalidInsertionPoint { span })?;
        let builder = self.context.create_builder();
        if let Some(first) = entry.get_first_instruction() {
            builder.position_before(&first);
        } else {
            builder.position_at_end(entry);
        }
        build(
            builder.build_alloca(self.llvm_type(ty, span)?, "local"),
            span,
        )
    }
    fn lookup(&self, name: &str, span: Span) -> Result<Binding<'ctx>, CodegenError> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .ok_or_else(|| CodegenError::UndefinedVariable {
                name: name.to_owned(),
                span,
            })
    }
    fn declare(
        &mut self,
        name: &str,
        value: Value<'ctx>,
        span: Span,
        argument: Option<u32>,
    ) -> Result<(), CodegenError> {
        if self
            .scopes
            .last()
            .is_some_and(|scope| scope.contains_key(name))
        {
            return Err(CodegenError::DuplicateDeclaration {
                name: name.to_owned(),
                span,
            });
        }
        let pointer = self.allocate(value.ty, span)?;
        if argument.is_none() {
            self.set_location(span)?;
        } else {
            self.builder.unset_current_debug_location();
        }
        build(self.builder.build_store(pointer, value.raw), span)?;
        let llvm = self.llvm_type(value.ty, span)?;
        if let Some(debug) = &mut self.debug {
            let block = self
                .builder
                .get_insert_block()
                .ok_or(CodegenError::InvalidInsertionPoint { span })?;
            debug.variable(
                name,
                Binding {
                    pointer,
                    ty: value.ty,
                },
                llvm,
                span,
                argument,
                block,
            )?;
        }
        self.scopes
            .last_mut()
            .ok_or(invalid("Missing lexical scope", span))?
            .insert(
                name.to_owned(),
                Binding {
                    pointer,
                    ty: value.ty,
                },
            );
        Ok(())
    }
    fn cast(
        &self,
        value: Value<'ctx>,
        target: Type,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        if value.ty == target {
            return Ok(value);
        }
        if value.ty == Type::Int && target == Type::Float {
            return Ok(Value {
                raw: build(
                    self.builder.build_signed_int_to_float(
                        value.int(span)?,
                        self.context.f64_type(),
                        "widen",
                    ),
                    span,
                )?
                .into(),
                ty: target,
            });
        }
        Err(invalid("Incompatible value type", span))
    }

    fn set_location(&self, span: Span) -> Result<(), CodegenError> {
        if let Some(debug) = &self.debug {
            self.builder
                .set_current_debug_location(debug.location(self.context, span)?);
        }
        Ok(())
    }
    fn expression(&mut self, expr: &Expr) -> Result<Value<'ctx>, CodegenError> {
        let previous = self.builder.get_current_debug_location();
        self.set_location(expr.span)?;
        let result = self.expression_inner(expr);
        if let Some(location) = previous {
            self.builder.set_current_debug_location(location);
        } else {
            self.builder.unset_current_debug_location();
        }
        result
    }
    fn expression_inner(&mut self, expr: &Expr) -> Result<Value<'ctx>, CodegenError> {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Error { payload, code } => {
                let code = self.expression(code)?.int(span)?;
                self.error_result(*payload, code, span)
            }
            ExprKind::Integer(literal) => {
                let value =
                    literal
                        .parse::<i64>()
                        .map_err(|_| CodegenError::InvalidIntegerLiteral {
                            literal: literal.clone(),
                            span,
                        })?;
                Ok(Value {
                    raw: self
                        .context
                        .i64_type()
                        .const_int(value.cast_unsigned(), true)
                        .into(),
                    ty: Type::Int,
                })
            }
            ExprKind::Float(literal) => {
                let value = literal
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| CodegenError::InvalidFloatLiteral {
                        literal: literal.clone(),
                        span,
                    })?;
                Ok(Value {
                    raw: self.context.f64_type().const_float(value).into(),
                    ty: Type::Float,
                })
            }
            ExprKind::String(literal) => self.string_literal(literal, span),
            ExprKind::Bool(value) => Ok(Value {
                raw: self
                    .context
                    .bool_type()
                    .const_int(u64::from(*value), false)
                    .into(),
                ty: Type::Bool,
            }),
            ExprKind::Identifier(name) => {
                let binding = self.lookup(name, span)?;
                let value = Value {
                    raw: build(
                        self.builder.build_load(
                            self.llvm_type(binding.ty, span)?,
                            binding.pointer,
                            name,
                        ),
                        span,
                    )?,
                    ty: binding.ty,
                };
                if value.ty == Type::String {
                    self.copy_string(value, span)
                } else if value.ty == Type::Result(ScalarType::String) {
                    self.copy_result(value, span)
                } else {
                    Ok(value)
                }
            }
            ExprKind::Negate(inner) => {
                if let ExprKind::Integer(literal) = &inner.kind
                    && let Ok(value) = format!("-{literal}").parse::<i64>()
                {
                    return Ok(Value {
                        raw: self
                            .context
                            .i64_type()
                            .const_int(value.cast_unsigned(), true)
                            .into(),
                        ty: Type::Int,
                    });
                }
                let value = self.expression(inner)?;
                match value.ty {
                    Type::Int => Ok(Value {
                        raw: self
                            .checked_int(
                                Op::Subtract,
                                self.context.i64_type().const_zero(),
                                value.int(span)?,
                                span,
                            )?
                            .into(),
                        ty: Type::Int,
                    }),
                    Type::Float => Ok(Value {
                        raw: build(
                            self.builder.build_float_neg(value.float(span)?, "negate"),
                            span,
                        )?
                        .into(),
                        ty: Type::Float,
                    }),
                    _ => Err(invalid("Negation requires a number", span)),
                }
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.expression(left)?;
                let right = self.expression(right)?;
                self.binary(*operator, left, right, span)
            }
            ExprKind::Call { name, arguments } => self.call(name, arguments, span),
            ExprKind::Array(elements) => self.array_literal(elements, span),
            ExprKind::Index { array, index } => {
                let array = self.expression(array)?;
                let Type::Array { element, len } = array.ty else {
                    return Err(invalid("Indexing requires an array", span));
                };
                let index = self.expression(index)?;
                if index.ty != Type::Int {
                    return Err(invalid("Array index must be Int", span));
                }
                let index = index.int(span)?;
                let bound = self.context.i64_type().const_int(
                    u64::try_from(len).map_err(|_| invalid("Invalid array length", span))?,
                    false,
                );
                let valid = build(
                    self.builder
                        .build_int_compare(IntPredicate::ULT, index, bound, "in.bounds"),
                    span,
                )?;
                self.guard(valid, "array index out of bounds", span)?;
                let pointer = self.spill(array, span)?;
                let address = self.element_pointer(pointer, array.ty, index, span)?;
                Ok(Value {
                    raw: build(
                        self.builder.build_load(
                            self.llvm_type(element.scalar(), span)?,
                            address,
                            "element",
                        ),
                        span,
                    )?,
                    ty: element.scalar(),
                })
            }
        }
    }
    fn binary(
        &mut self,
        op: Op,
        mut left: Value<'ctx>,
        mut right: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        if left.ty == Type::String && right.ty == Type::String {
            return self.string_binary(op, left, right, span);
        }
        if matches!(left.ty, Type::Array { .. }) {
            return self.array_binary(op, left, right, span);
        }
        if left.ty.is_numeric()
            && right.ty.is_numeric()
            && (left.ty == Type::Float || right.ty == Type::Float)
        {
            left = self.cast(left, Type::Float, span)?;
            right = self.cast(right, Type::Float, span)?;
        }
        if left.ty != right.ty {
            return Err(invalid("Incompatible operands", span));
        }
        if op.is_comparison() {
            let raw = if left.ty == Type::Float {
                let predicate = match op {
                    Op::Equal => FloatPredicate::OEQ,
                    Op::NotEqual => FloatPredicate::UNE,
                    Op::Less => FloatPredicate::OLT,
                    Op::LessEqual => FloatPredicate::OLE,
                    Op::Greater => FloatPredicate::OGT,
                    Op::GreaterEqual => FloatPredicate::OGE,
                    _ => return Err(invalid("Invalid comparison", span)),
                };
                build(
                    self.builder.build_float_compare(
                        predicate,
                        left.float(span)?,
                        right.float(span)?,
                        "compare",
                    ),
                    span,
                )?
            } else {
                if left.ty == Type::Bool && !matches!(op, Op::Equal | Op::NotEqual) {
                    return Err(invalid("Bool only supports equality", span));
                }
                let predicate = match op {
                    Op::Equal => IntPredicate::EQ,
                    Op::NotEqual => IntPredicate::NE,
                    Op::Less => IntPredicate::SLT,
                    Op::LessEqual => IntPredicate::SLE,
                    Op::Greater => IntPredicate::SGT,
                    Op::GreaterEqual => IntPredicate::SGE,
                    _ => return Err(invalid("Invalid comparison", span)),
                };
                build(
                    self.builder.build_int_compare(
                        predicate,
                        left.int(span)?,
                        right.int(span)?,
                        "compare",
                    ),
                    span,
                )?
            };
            return Ok(Value {
                raw: raw.into(),
                ty: Type::Bool,
            });
        }
        match left.ty {
            Type::Int => Ok(Value {
                raw: self
                    .checked_int(op, left.int(span)?, right.int(span)?, span)?
                    .into(),
                ty: Type::Int,
            }),
            Type::Float => {
                let a = left.float(span)?;
                let b = right.float(span)?;
                let raw = match op {
                    Op::Add => build(self.builder.build_float_add(a, b, "add"), span)?,
                    Op::Subtract => build(self.builder.build_float_sub(a, b, "subtract"), span)?,
                    Op::Multiply => build(self.builder.build_float_mul(a, b, "multiply"), span)?,
                    Op::Divide => {
                        let valid = build(
                            self.builder.build_float_compare(
                                FloatPredicate::UNE,
                                b,
                                self.context.f64_type().const_zero(),
                                "nonzero",
                            ),
                            span,
                        )?;
                        self.guard(valid, "division by zero", span)?;
                        build(self.builder.build_float_div(a, b, "divide"), span)?
                    }
                    _ => return Err(invalid("Invalid Float operator", span)),
                };
                Ok(Value {
                    raw: raw.into(),
                    ty: Type::Float,
                })
            }
            _ => Err(invalid("Arithmetic requires numeric operands", span)),
        }
    }

    fn call(
        &mut self,
        name: &str,
        arguments: &[Expr],
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let mut values = Vec::new();
        for argument in arguments {
            values.push(self.expression(argument)?);
        }
        if matches!(
            name,
            "input"
                | "try_input"
                | "parse_int"
                | "parse_float"
                | "to_string"
                | "ok"
                | "is_ok"
                | "is_eof"
                | "error_code"
                | "error_message"
                | "unwrap"
        ) {
            return self.result_builtin(name, &values, span);
        }
        match (name, values.as_slice()) {
            ("length", [value]) if value.ty == Type::String => self.string_len(*value, span),
            (
                "length",
                [
                    Value {
                        ty: Type::Array { len, .. },
                        ..
                    },
                ],
            ) => Ok(Value {
                raw: self
                    .context
                    .i64_type()
                    .const_int(
                        u64::try_from(*len).map_err(|_| invalid("Invalid array length", span))?,
                        false,
                    )
                    .into(),
                ty: Type::Int,
            }),
            ("sum", [array]) => self.sum(*array, span),
            ("abs", [value]) if value.ty == Type::Int => {
                let integer = value.int(span)?;
                let is_min = build(
                    self.builder.build_int_compare(
                        IntPredicate::NE,
                        integer,
                        self.context
                            .i64_type()
                            .const_int(i64::MIN.cast_unsigned(), true),
                        "abs.safe",
                    ),
                    span,
                )?;
                self.guard(is_min, "integer abs overflow", span)?;
                let negative = build(
                    self.builder.build_int_compare(
                        IntPredicate::SLT,
                        integer,
                        self.context.i64_type().const_zero(),
                        "negative",
                    ),
                    span,
                )?;
                let negated = build(self.builder.build_int_neg(integer, "absolute"), span)?;
                Ok(Value {
                    raw: build(
                        self.builder.build_select(negative, negated, integer, "abs"),
                        span,
                    )?,
                    ty: Type::Int,
                })
            }
            ("abs" | "sqrt", [value]) if value.ty == Type::Float => {
                let function = self.external(
                    if name == "abs" { "fabs" } else { "sqrt" },
                    self.context
                        .f64_type()
                        .fn_type(&[self.context.f64_type().into()], false),
                )?;
                Ok(Value {
                    raw: self.call_value(function, &[value.raw.into()], span)?,
                    ty: Type::Float,
                })
            }
            ("min" | "max", [a, b]) if a.ty.is_numeric() && b.ty.is_numeric() => {
                if a.ty == Type::Float || b.ty == Type::Float {
                    let a = self.cast(*a, Type::Float, span)?;
                    let b = self.cast(*b, Type::Float, span)?;
                    let function = self.external(
                        if name == "min" { "fmin" } else { "fmax" },
                        self.context.f64_type().fn_type(
                            &[
                                self.context.f64_type().into(),
                                self.context.f64_type().into(),
                            ],
                            false,
                        ),
                    )?;
                    Ok(Value {
                        raw: self.call_value(function, &[a.raw.into(), b.raw.into()], span)?,
                        ty: Type::Float,
                    })
                } else {
                    let predicate = if name == "min" {
                        IntPredicate::SLT
                    } else {
                        IntPredicate::SGT
                    };
                    let condition = build(
                        self.builder.build_int_compare(
                            predicate,
                            a.int(span)?,
                            b.int(span)?,
                            "compare",
                        ),
                        span,
                    )?;
                    Ok(Value {
                        raw: build(
                            self.builder
                                .build_select(condition, a.raw, b.raw, "extreme"),
                            span,
                        )?,
                        ty: Type::Int,
                    })
                }
            }
            _ => {
                let function = self.functions.get(name).cloned().ok_or(invalid(
                    "Undefined function or invalid built-in arguments",
                    span,
                ))?;
                if function.parameters.len() != values.len() {
                    return Err(invalid("Wrong function argument count", span));
                }
                let args = values
                    .into_iter()
                    .zip(function.parameters)
                    .map(|(value, ty)| self.cast(value, ty, span).map(|value| value.raw.into()))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value {
                    raw: self.call_value(function.value, &args, span)?,
                    ty: function.result,
                })
            }
        }
    }

    fn statements(&mut self, statements: &[Stmt]) -> Result<(), CodegenError> {
        for statement in statements {
            if self.terminated() {
                break;
            }
            self.statement(statement)?;
        }
        Ok(())
    }
    fn scoped(&mut self, body: &[Stmt]) -> Result<(), CodegenError> {
        if let Some(debug) = &mut self.debug {
            if let Some(first) = body.first() {
                debug.enter_scope(first.span)?;
            } else {
                return Ok(());
            }
        }
        self.scopes.push(HashMap::new());
        let result = self.statements(body).and_then(|()| {
            if !self.terminated() {
                self.cleanup_strings(
                    false,
                    body.last()
                        .map(|s| s.span)
                        .unwrap_or(Span { start: 0, end: 0 }),
                )?;
            }
            Ok(())
        });
        self.scopes.pop();
        if let Some(debug) = &mut self.debug {
            debug.exit_scope();
        }
        self.builder.unset_current_debug_location();
        result
    }
    fn statement(&mut self, stmt: &Stmt) -> Result<(), CodegenError> {
        let span = stmt.span;
        self.set_location(span)?;
        match &stmt.kind {
            StmtKind::Set { name, value } => {
                if self
                    .scopes
                    .last()
                    .is_some_and(|scope| scope.contains_key(name))
                {
                    return Err(CodegenError::DuplicateDeclaration {
                        name: name.clone(),
                        span,
                    });
                }
                let value = self.expression(value)?;
                self.declare(name, value, span, None)
            }
            StmtKind::Assign { name, value } => {
                let binding = self.lookup(name, span)?;
                let value = self.expression(value)?;
                let value = self.cast(value, binding.ty, span)?;
                self.release_binding(binding, span)?;
                build(self.builder.build_store(binding.pointer, value.raw), span)?;
                Ok(())
            }
            StmtKind::Show(expr) => {
                let value = self.expression(expr)?;
                self.output(value, span)?;
                self.release_value(value, span)
            }
            StmtKind::Return(expr) => {
                let value = self.expression(expr)?;
                let value = self.cast(value, self.result, span)?;
                self.cleanup_strings(true, span)?;
                build(self.builder.build_return(Some(&value.raw)), span)?;
                Ok(())
            }
            StmtKind::If {
                condition,
                then_body,
                else_body,
            } => {
                let value = self.expression(condition)?;
                if value.ty != Type::Bool {
                    return Err(invalid("Condition must be Bool", condition.span));
                }
                let function = self.function(span)?;
                let yes = self.context.append_basic_block(function, "if.then");
                let no = self.context.append_basic_block(function, "if.else");
                let merge = self.context.append_basic_block(function, "if.end");
                build(
                    self.builder
                        .build_conditional_branch(value.int(span)?, yes, no),
                    span,
                )?;
                self.builder.position_at_end(yes);
                self.scoped(then_body)?;
                let yes_returns = self.terminated();
                if !yes_returns {
                    build(self.builder.build_unconditional_branch(merge), span)?;
                }
                self.builder.position_at_end(no);
                self.scoped(else_body)?;
                let no_returns = self.terminated();
                if !no_returns {
                    build(self.builder.build_unconditional_branch(merge), span)?;
                }
                self.builder.position_at_end(merge);
                if yes_returns && no_returns {
                    build(self.builder.build_unreachable(), span)?;
                }
                Ok(())
            }
            StmtKind::While { condition, body } => {
                let function = self.function(span)?;
                let test = self.context.append_basic_block(function, "while.condition");
                let loop_body = self.context.append_basic_block(function, "while.body");
                let done = self.context.append_basic_block(function, "while.end");
                // A line breakpoint must resolve to the repeated condition,
                // not the one-time jump from the preceding statement.
                self.builder.unset_current_debug_location();
                build(self.builder.build_unconditional_branch(test), span)?;
                self.builder.position_at_end(test);
                self.set_location(condition.span)?;
                let value = self.expression(condition)?;
                if value.ty != Type::Bool {
                    return Err(invalid("Condition must be Bool", condition.span));
                }
                build(
                    self.builder
                        .build_conditional_branch(value.int(span)?, loop_body, done),
                    span,
                )?;
                self.builder.position_at_end(loop_body);
                self.scoped(body)?;
                if !self.terminated() {
                    build(self.builder.build_unconditional_branch(test), span)?;
                }
                self.builder.position_at_end(done);
                Ok(())
            }
            StmtKind::Function { .. } => Err(invalid("Nested functions are unsupported", span)),
        }
    }
}
