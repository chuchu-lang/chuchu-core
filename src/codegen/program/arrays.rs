use super::*;

impl<'ctx> Generator<'ctx> {
    pub(super) fn spill(
        &self,
        value: &Value<'ctx>,
        span: Span,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let pointer = self.allocate(&value.ty, span)?;
        build(self.builder.build_store(pointer, value.raw), span)?;
        Ok(pointer)
    }
    pub(super) fn element_pointer(
        &self,
        pointer: PointerValue<'ctx>,
        ty: &Type,
        index: IntValue<'ctx>,
        span: Span,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        // SAFETY: pointer is allocated with this exact fixed array type by allocate/spill.
        // Source indices are checked before use; generated loop indices are in 0..len.
        // GEP is deliberately not inbounds, so even an unreachable failing path adds no
        // LLVM inbounds assumptions. Loads are only reachable after the bounds guard.
        unsafe {
            build(
                self.builder.build_gep(
                    self.llvm_type(&ty, span)?,
                    pointer,
                    &[self.context.i64_type().const_zero(), index],
                    "element.ptr",
                ),
                span,
            )
        }
    }
    pub(super) fn array_literal(
        &mut self,
        elements: &[Expr],
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        if elements.is_empty() || elements.len() > MAX_ARRAY_LEN {
            return Err(invalid("Array length outside 1..=256", span));
        }
        let mut values = Vec::new();
        for element in elements {
            values.push(self.expression(element)?);
        }
        let element = match values[0].ty {
            Type::Int => NumericType::Int,
            Type::Float => NumericType::Float,
            _ => return Err(invalid("Array elements must be numeric", span)),
        };
        let ty = Type::Array {
            element,
            len: values.len(),
        };
        let pointer = self.allocate(&ty, span)?;
        for (index, value) in values.iter().enumerate() {
            if value.ty != element.scalar() {
                return Err(invalid("Array elements must be homogeneous", span));
            }
            let index = self.context.i64_type().const_int(
                u64::try_from(index).map_err(|_| invalid("Invalid index", span))?,
                false,
            );
            let address = self.element_pointer(pointer, &ty, index, span)?;
            build(self.builder.build_store(address, value.raw), span)?;
        }
        Ok(Value {
            raw: build(
                self.builder
                    .build_load(self.llvm_type(&ty, span)?, pointer, "array"),
                span,
            )?,
            ty,
        })
    }
    pub(super) fn array_loop(
        &mut self,
        len: usize,
        span: Span,
        mut body: impl FnMut(&mut Self, IntValue<'ctx>) -> Result<(), CodegenError>,
    ) -> Result<(), CodegenError> {
        let counter = self.allocate(&Type::Int, span)?;
        build(
            self.builder
                .build_store(counter, self.context.i64_type().const_zero()),
            span,
        )?;
        let function = self.function(span)?;
        let condition = self.context.append_basic_block(function, "array.condition");
        let loop_body = self.context.append_basic_block(function, "array.body");
        let done = self.context.append_basic_block(function, "array.done");
        build(self.builder.build_unconditional_branch(condition), span)?;
        self.builder.position_at_end(condition);
        let raw = build(
            self.builder
                .build_load(self.context.i64_type(), counter, "index"),
            span,
        )?;
        let index = Value { raw, ty: Type::Int }.int(span)?;
        let length = self.context.i64_type().const_int(
            u64::try_from(len).map_err(|_| invalid("Invalid array length", span))?,
            false,
        );
        let valid = build(
            self.builder
                .build_int_compare(IntPredicate::ULT, index, length, "more"),
            span,
        )?;
        build(
            self.builder
                .build_conditional_branch(valid, loop_body, done),
            span,
        )?;
        self.builder.position_at_end(loop_body);
        body(self, index)?;
        let next = build(
            self.builder
                .build_int_add(index, self.context.i64_type().const_int(1, false), "next"),
            span,
        )?;
        build(self.builder.build_store(counter, next), span)?;
        build(self.builder.build_unconditional_branch(condition), span)?;
        self.builder.position_at_end(done);
        Ok(())
    }
    pub(super) fn array_binary(
        &mut self,
        op: Op,
        left: Value<'ctx>,
        right: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let Type::Array { element, len } = left.ty else {
            return Err(invalid("Expected arrays", span));
        };
        if left.ty != right.ty || !matches!(op, Op::Add | Op::Multiply) {
            return Err(invalid(
                "Array types and lengths must match for + or *",
                span,
            ));
        }
        let a = self.spill(&left, span)?;
        let b = self.spill(&right, span)?;
        let result = self.allocate(&left.ty, span)?;
        self.array_loop(len, span, |this, index| {
            let ap = this.element_pointer(a, &left.ty, index, span)?;
            let bp = this.element_pointer(b, &right.ty, index, span)?;
            let ty = this.llvm_type(&element.scalar(), span)?;
            let a = Value {
                raw: build(this.builder.build_load(ty, ap, "a"), span)?,
                ty: element.scalar(),
            };
            let b = Value {
                raw: build(this.builder.build_load(ty, bp, "b"), span)?,
                ty: element.scalar(),
            };
            let value = this.binary(op, a, b, span)?;
            let out = this.element_pointer(result, &left.ty, index, span)?;
            build(this.builder.build_store(out, value.raw), span)?;
            Ok(())
        })?;
        Ok(Value {
            raw: build(
                self.builder
                    .build_load(self.llvm_type(&left.ty, span)?, result, "array.result"),
                span,
            )?,
            ty: left.ty,
        })
    }
    pub(super) fn sum(
        &mut self,
        array: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let Type::Array { element, len } = array.ty else {
            return Err(invalid("sum requires an array", span));
        };
        let pointer = self.spill(&array, span)?;
        let ty = self.llvm_type(&element.scalar(), span)?;
        let accumulator = self.allocate(&element.scalar(), span)?;
        build(self.builder.build_store(accumulator, ty.const_zero()), span)?;
        self.array_loop(len, span, |this, index| {
            let address = this.element_pointer(pointer, &array.ty, index, span)?;
            let value = Value {
                raw: build(this.builder.build_load(ty, address, "element"), span)?,
                ty: element.scalar(),
            };
            let current = Value {
                raw: build(this.builder.build_load(ty, accumulator, "sum"), span)?,
                ty: element.scalar(),
            };
            let result = this.binary(Op::Add, current, value, span)?;
            build(this.builder.build_store(accumulator, result.raw), span)?;
            Ok(())
        })?;
        Ok(Value {
            raw: build(self.builder.build_load(ty, accumulator, "sum.result"), span)?,
            ty: element.scalar(),
        })
    }
}
