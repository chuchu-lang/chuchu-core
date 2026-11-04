use super::*;

impl<'ctx> Generator<'ctx> {
    pub(super) fn external(
        &self,
        name: &str,
        ty: FunctionType<'ctx>,
    ) -> Result<FunctionValue<'ctx>, CodegenError> {
        match self.module.get_function(name) {
            Some(function) if function.get_type() == ty => Ok(function),
            Some(_) => Err(CodegenError::InvalidRuntimeDeclaration),
            None => Ok(self.module.add_function(name, ty, None)),
        }
    }
    pub(super) fn call_value(
        &self,
        function: FunctionValue<'ctx>,
        arguments: &[BasicMetadataValueEnum<'ctx>],
        span: Span,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        build(self.builder.build_call(function, arguments, "call"), span)?
            .try_as_basic_value()
            .basic()
            .ok_or(invalid("Expected function result", span))
    }
    pub(super) fn guard(
        &mut self,
        valid: IntValue<'ctx>,
        message: &str,
        span: Span,
    ) -> Result<(), CodegenError> {
        if valid.get_zero_extended_constant() == Some(1) {
            return Ok(());
        }
        let function = self.function(span)?;
        let good = self.context.append_basic_block(function, "runtime.ok");
        let bad = self.context.append_basic_block(function, "runtime.fail");
        build(
            self.builder.build_conditional_branch(valid, good, bad),
            span,
        )?;
        self.builder.position_at_end(bad);
        let message = if let Some(source) = &self.source
            && source.files().count() > 1
            && let Some((file, local)) = source.origin(span.start)
            && let Some(position) = file.position(local)
        {
            format!(
                "cheeku runtime: {message} at {}:{}:{} ({}..{})\n",
                file.path().display(),
                position.line,
                position.column,
                local,
                local + span.end.saturating_sub(span.start)
            )
        } else {
            format!(
                "cheeku runtime: {message} at {}..{}\n",
                span.start, span.end
            )
        };
        let text = build(
            self.builder
                .build_global_string_ptr(&message, "runtime.message"),
            span,
        )?;
        let ptr = self.context.ptr_type(AddressSpace::default());
        // Unix write: ssize_t write(int, const void *, size_t).
        // MSVC _write: int _write(int, const void *, unsigned int).
        // All supported hosts are 64-bit, but Windows uses a 32-bit count/result.
        let (write_name, count_type) = if cfg!(target_os = "windows") {
            ("_write", self.context.i32_type())
        } else {
            ("write", self.context.i64_type())
        };
        let write = self.external(
            write_name,
            count_type.fn_type(
                &[
                    self.context.i32_type().into(),
                    ptr.into(),
                    count_type.into(),
                ],
                false,
            ),
        )?;
        let length =
            u64::try_from(message.len()).map_err(|_| invalid("Runtime message too long", span))?;
        if cfg!(target_os = "windows") && length > u64::from(u32::MAX) {
            return Err(invalid("Runtime message too long for _write", span));
        }
        build(
            self.builder.build_call(
                write,
                &[
                    self.context.i32_type().const_int(2, false).into(),
                    text.as_pointer_value().into(),
                    count_type.const_int(length, false).into(),
                ],
                "diagnostic",
            ),
            span,
        )?;
        let exit = self.external(
            "exit",
            self.context
                .void_type()
                .fn_type(&[self.context.i32_type().into()], false),
        )?;
        build(
            self.builder.build_call(
                exit,
                &[self.context.i32_type().const_int(1, false).into()],
                "",
            ),
            span,
        )?;
        build(self.builder.build_unreachable(), span)?;
        self.builder.position_at_end(good);
        Ok(())
    }
    pub(super) fn checked_int(
        &mut self,
        op: Op,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
        span: Span,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        if op == Op::Divide {
            let nonzero = build(
                self.builder.build_int_compare(
                    IntPredicate::NE,
                    right,
                    self.context.i64_type().const_zero(),
                    "nonzero",
                ),
                span,
            )?;
            self.guard(nonzero, "division by zero", span)?;
            let min = build(
                self.builder.build_int_compare(
                    IntPredicate::EQ,
                    left,
                    self.context
                        .i64_type()
                        .const_int(i64::MIN.cast_unsigned(), true),
                    "min",
                ),
                span,
            )?;
            let negative_one = build(
                self.builder.build_int_compare(
                    IntPredicate::EQ,
                    right,
                    self.context.i64_type().const_all_ones(),
                    "negative.one",
                ),
                span,
            )?;
            let overflow = build(
                self.builder
                    .build_and(min, negative_one, "division.overflow"),
                span,
            )?;
            let safe = build(self.builder.build_not(overflow, "safe"), span)?;
            self.guard(safe, "integer division overflow", span)?;
            // Never construct a poison constant even in the unreachable success block.
            if right.get_sign_extended_constant() == Some(0)
                || (left.get_sign_extended_constant() == Some(i64::MIN)
                    && right.get_sign_extended_constant() == Some(-1))
            {
                return Ok(self.context.i64_type().const_zero());
            }
            return build(
                self.builder.build_int_signed_div(left, right, "divide"),
                span,
            );
        }
        let name = match op {
            Op::Add => "llvm.sadd.with.overflow.i64",
            Op::Subtract => "llvm.ssub.with.overflow.i64",
            Op::Multiply => "llvm.smul.with.overflow.i64",
            _ => return Err(invalid("Invalid integer operator", span)),
        };
        let ty = self.context.struct_type(
            &[
                self.context.i64_type().into(),
                self.context.bool_type().into(),
            ],
            false,
        );
        let function = self.external(
            name,
            ty.fn_type(
                &[
                    self.context.i64_type().into(),
                    self.context.i64_type().into(),
                ],
                false,
            ),
        )?;
        let raw = self.call_value(function, &[left.into(), right.into()], span)?;
        let BasicValueEnum::StructValue(result) = raw else {
            return Err(invalid("Invalid overflow intrinsic result", span));
        };
        let value = build(
            self.builder.build_extract_value(result, 0, "arithmetic"),
            span,
        )?;
        let overflow = build(
            self.builder.build_extract_value(result, 1, "overflow"),
            span,
        )?;
        let flag = Value {
            raw: overflow,
            ty: Type::Bool,
        }
        .int(span)?;
        let safe = build(self.builder.build_not(flag, "safe"), span)?;
        self.guard(safe, "integer overflow", span)?;
        Value {
            raw: value,
            ty: Type::Int,
        }
        .int(span)
    }
    pub(super) fn output(&mut self, value: Value<'ctx>, span: Span) -> Result<(), CodegenError> {
        let (format, name, raw) = match value.ty {
            Type::Int => ("%lld\n", ".cheeku.format.i64", value.raw),
            Type::Float => ("%.15g\n", ".cheeku.format.float", value.raw),
            Type::String => ("%s\n", ".cheeku.format.string", value.raw),
            Type::Bool => {
                let yes = build(
                    self.builder.build_global_string_ptr("true", "bool.true"),
                    span,
                )?;
                let no = build(
                    self.builder.build_global_string_ptr("false", "bool.false"),
                    span,
                )?;
                let text = build(
                    self.builder.build_select(
                        value.int(span)?,
                        yes.as_pointer_value(),
                        no.as_pointer_value(),
                        "bool.text",
                    ),
                    span,
                )?;
                ("%s\n", ".cheeku.format.bool", text)
            }
            _ => return Err(invalid("show requires a scalar", span)),
        };
        let format = match self.module.get_global(name) {
            Some(global) => global,
            None => build(self.builder.build_global_string_ptr(format, name), span)?,
        };
        let printf = self.external(
            "printf",
            self.context.i32_type().fn_type(
                &[self.context.ptr_type(AddressSpace::default()).into()],
                true,
            ),
        )?;
        build(
            self.builder.build_call(
                printf,
                &[format.as_pointer_value().into(), raw.into()],
                "printf_result",
            ),
            span,
        )?;
        Ok(())
    }
}
