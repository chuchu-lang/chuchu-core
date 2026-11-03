//! Built-in result values and the small native input/conversion boundary.
use super::*;
use crate::runtime::ErrorCode;

impl<'ctx> Generator<'ctx> {
    pub(super) fn error_result(
        &mut self,
        payload: ScalarType,
        code: IntValue<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        // Codes 1..7 are predefined; larger positive i32 codes are available
        // for application-defined errors. Zero is reserved for success.
        let positive = build(
            self.builder.build_int_compare(
                IntPredicate::SGT,
                code,
                self.context.i64_type().const_zero(),
                "error.positive",
            ),
            span,
        )?;
        let fits = build(
            self.builder.build_int_compare(
                IntPredicate::SLE,
                code,
                self.context.i64_type().const_int(i32::MAX as u64, false),
                "error.fits",
            ),
            span,
        )?;
        let valid = build(self.builder.build_and(positive, fits, "error.valid"), span)?;
        self.guard(valid, "error code must be in 1..=2147483647", span)?;
        let code = build(
            self.builder
                .build_int_truncate(code, self.context.i32_type(), "error.code"),
            span,
        )?;
        let ty = payload.ty();
        self.make_result(
            code,
            Value {
                raw: self.llvm_type(&ty, span)?.const_zero(),
                ty,
            },
            span,
        )
    }
    fn result_code(&self, value: Value<'ctx>, span: Span) -> Result<IntValue<'ctx>, CodegenError> {
        if !matches!(value.ty, Type::Result(_)) {
            return Err(invalid("Expected result value", span));
        }
        Ok(build(
            self.builder
                .build_extract_value(value.raw.into_struct_value(), 0, "result.code"),
            span,
        )?
        .into_int_value())
    }

    pub(super) fn result_payload(
        &self,
        value: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let Type::Result(payload) = value.ty else {
            return Err(invalid("Expected result value", span));
        };
        Ok(Value {
            raw: build(
                self.builder
                    .build_extract_value(value.raw.into_struct_value(), 1, "result.value"),
                span,
            )?,
            ty: payload.ty(),
        })
    }

    fn make_result(
        &self,
        code: IntValue<'ctx>,
        value: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let payload = ScalarType::from_type(&value.ty)
            .ok_or(invalid("Result payload must be a scalar", span))?;
        let ty = Type::Result(payload);
        let llvm = self.llvm_type(&ty, span)?.into_struct_type();
        let aggregate = build(
            self.builder
                .build_insert_value(llvm.get_undef(), code, 0, "result.status"),
            span,
        )?;
        let aggregate = build(
            self.builder
                .build_insert_value(aggregate, value.raw, 1, "result"),
            span,
        )?;
        Ok(Value {
            raw: aggregate.into_struct_value().into(),
            ty,
        })
    }

    pub(super) fn copy_result(
        &mut self,
        value: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let code = self.result_code(value.clone(), span)?;
        let success = build(
            self.builder.build_int_compare(
                IntPredicate::EQ,
                code,
                self.context.i32_type().const_zero(),
                "result.ok",
            ),
            span,
        )?;
        let function = self.function(span)?;
        let yes = self.context.append_basic_block(function, "result.copy.ok");
        let no = self
            .context
            .append_basic_block(function, "result.copy.error");
        let done = self
            .context
            .append_basic_block(function, "result.copy.done");
        build(
            self.builder.build_conditional_branch(success, yes, no),
            span,
        )?;
        self.builder.position_at_end(yes);
        let payload = self.result_payload(value, span)?;
        let copy = self.copy_string(payload, span)?;
        let copied_block = self
            .builder
            .get_insert_block()
            .ok_or(CodegenError::InvalidInsertionPoint { span })?;
        build(self.builder.build_unconditional_branch(done), span)?;
        self.builder.position_at_end(no);
        build(self.builder.build_unconditional_branch(done), span)?;
        self.builder.position_at_end(done);
        let ptr = self.context.ptr_type(AddressSpace::default());
        let result = build(self.builder.build_phi(ptr, "result.string.copy"), span)?;
        let null = ptr.const_null();
        result.add_incoming(&[(&copy.raw, copied_block), (&null, no)]);
        self.make_result(
            code,
            Value {
                raw: result.as_basic_value(),
                ty: Type::String,
            },
            span,
        )
    }

    fn native_result(
        &mut self,
        name: &str,
        payload: ScalarType,
        arguments: &[Value<'ctx>],
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let ty = payload.ty();
        let llvm = self.llvm_type(&ty, span)?;
        let output = self.allocate(&ty, span)?;
        build(self.builder.build_store(output, llvm.const_zero()), span)?;
        let ptr = self.context.ptr_type(AddressSpace::default());
        let parameters = vec![ptr.into(); arguments.len() + 1];
        let function = self.external(name, self.context.i32_type().fn_type(&parameters, false))?;
        let mut args = arguments
            .iter()
            .map(|value| value.raw.into())
            .collect::<Vec<BasicMetadataValueEnum<'ctx>>>();
        args.push(output.into());
        let status = self.call_value(function, &args, span)?.into_int_value();
        for value in arguments {
            self.release_value(value.clone(), span)?;
        }
        let raw = build(self.builder.build_load(llvm, output, "native.result"), span)?;
        self.make_result(status, Value { raw, ty }, span)
    }

    /// Ends the program with `message` unless a native helper reported success.
    fn guard_status_is_ok(
        &mut self,
        code: IntValue<'ctx>,
        message: &str,
        span: Span,
    ) -> Result<(), CodegenError> {
        let success = build(
            self.builder.build_int_compare(
                IntPredicate::EQ,
                code,
                self.context
                    .i32_type()
                    .const_int(ErrorCode::Ok as u64, false),
                "native.ok",
            ),
            span,
        )?;
        self.guard(success, message, span)
    }

    /// Ends the program with `message` when a native helper reported `status`.
    fn guard_status_is_not(
        &mut self,
        code: IntValue<'ctx>,
        status: ErrorCode,
        message: &str,
        span: Span,
    ) -> Result<(), CodegenError> {
        let differs = build(
            self.builder.build_int_compare(
                IntPredicate::NE,
                code,
                self.context.i32_type().const_int(status as u64, false),
                "native.status",
            ),
            span,
        )?;
        self.guard(differs, message, span)
    }

    pub(super) fn result_builtin(
        &mut self,
        name: &str,
        values: &[Value<'ctx>],
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        match (name, values) {
            (
                "input" | "try_input",
                []
                | [
                    Value {
                        ty: Type::String, ..
                    },
                ],
            ) => {
                let helper = if values.is_empty() {
                    "cheeku_read_line"
                } else {
                    "cheeku_read_line_prompt"
                };
                let result = self.native_result(helper, ScalarType::String, values, span)?;
                if name == "try_input" {
                    return Ok(result);
                }
                let code = self.result_code(result.clone(), span)?;
                let not_eof = build(
                    self.builder.build_int_compare(
                        IntPredicate::NE,
                        code,
                        self.context
                            .i32_type()
                            .const_int(ErrorCode::Eof as u64, false),
                        "input.not_eof",
                    ),
                    span,
                )?;
                self.guard(
                    not_eof,
                    "input reached end of input; use try_input() to handle EOF",
                    span,
                )?;
                let success = build(
                    self.builder.build_int_compare(
                        IntPredicate::EQ,
                        code,
                        self.context.i32_type().const_zero(),
                        "input.ok",
                    ),
                    span,
                )?;
                self.guard(
                    success,
                    "input failed; use try_input() to handle read errors",
                    span,
                )?;
                // Transfer the successful owned string to the caller.
                self.result_payload(result, span)
            }
            (
                "read_file" | "try_read_file",
                [
                    Value {
                        ty: Type::String, ..
                    },
                ],
            ) => {
                let result =
                    self.native_result("cheeku_read_file", ScalarType::String, values, span)?;
                if name == "try_read_file" {
                    return Ok(result);
                }
                // Report the specific reason before the catch-all, so a failure
                // explains itself. Codes are the shared runtime codes.
                let code = self.result_code(result, span)?;
                self.guard_status_is_not(
                    code,
                    ErrorCode::InvalidUtf8,
                    "read_file failed: the file is not valid UTF-8; use try_read_file() to handle errors",
                    span,
                )?;
                self.guard_status_is_not(
                    code,
                    ErrorCode::Nul,
                    "read_file failed: the file contains a NUL byte; use try_read_file() to handle errors",
                    span,
                )?;
                self.guard_status_is_not(
                    code,
                    ErrorCode::Allocation,
                    "read_file failed: out of memory while reading the file; use try_read_file() to handle errors",
                    span,
                )?;
                self.guard_status_is_ok(
                    code,
                    "read_file failed: the file could not be opened or read; use try_read_file() to handle errors",
                    span,
                )?;
                // Transfer the successful owned string to the caller.
                self.result_payload(result, span)
            }
            (
                "write_file" | "try_write_file",
                [
                    Value {
                        ty: Type::String, ..
                    },
                    Value {
                        ty: Type::String, ..
                    },
                ],
            ) => {
                let result =
                    self.native_result("cheeku_write_file", ScalarType::Int, values, span)?;
                if name == "try_write_file" {
                    return Ok(result);
                }
                let code = self.result_code(result, span)?;
                self.guard_status_is_ok(
                    code,
                    "write_file failed: the file could not be created, written or closed; use try_write_file() to handle errors",
                    span,
                )?;
                // The payload is the byte count; there is nothing to release.
                self.result_payload(result, span)
            }
            ("parse_int", [value]) if value.ty == Type::String => {
                self.native_result("cheeku_parse_int", ScalarType::Int, values, span)
            }
            ("parse_float", [value]) if value.ty == Type::String => {
                self.native_result("cheeku_parse_float", ScalarType::Float, values, span)
            }
            ("ok", [value]) => self.make_result(
                self.context
                    .i32_type()
                    .const_int(ErrorCode::Ok as u64, false),
                value.clone(),
                span,
            ),
            ("to_string", [value]) => self.format_string_value(value.clone(), span),
            ("is_ok" | "is_eof" | "error_code" | "error_message" | "unwrap", [value])
                if matches!(value.ty, Type::Result(_)) =>
            {
                let code = self.result_code(value.clone(), span)?;
                let result = match name {
                    "unwrap" => {
                        let valid = build(
                            self.builder.build_int_compare(
                                IntPredicate::EQ,
                                code,
                                self.context.i32_type().const_zero(),
                                "unwrap.ok",
                            ),
                            span,
                        )?;
                        self.guard(
                            valid,
                            "cannot unwrap an error result; check is_ok and error_message",
                            span,
                        )?;
                        // The temporary result's successful payload is transferred
                        // to the caller. Do not release it after extraction.
                        return self.result_payload(value.clone(), span);
                    }
                    "is_ok" | "is_eof" => {
                        let expected = if name == "is_ok" {
                            ErrorCode::Ok
                        } else {
                            ErrorCode::Eof
                        };
                        Value {
                            raw: build(
                                self.builder.build_int_compare(
                                    IntPredicate::EQ,
                                    code,
                                    self.context.i32_type().const_int(expected as u64, false),
                                    "result.test",
                                ),
                                span,
                            )?
                            .into(),
                            ty: Type::Bool,
                        }
                    }
                    "error_code" => Value {
                        raw: build(
                            self.builder.build_int_z_extend(
                                code,
                                self.context.i64_type(),
                                "result.error.code",
                            ),
                            span,
                        )?
                        .into(),
                        ty: Type::Int,
                    },
                    _ => {
                        let function = self.external(
                            "cheeku_error_message",
                            self.context
                                .ptr_type(AddressSpace::default())
                                .fn_type(&[self.context.i32_type().into()], false),
                        )?;
                        let raw = self.call_value(function, &[code.into()], span)?;
                        self.copy_string(
                            Value {
                                raw,
                                ty: Type::String,
                            },
                            span,
                        )?
                    }
                };
                self.release_value(value.clone(), span)?;
                Ok(result)
            }
            _ => Err(invalid("Invalid input/conversion builtin arguments", span)),
        }
    }

    fn format_string_value(
        &mut self,
        value: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        if value.ty == Type::String {
            return Ok(value);
        }
        if value.ty == Type::Bool {
            let yes = build(
                self.builder.build_global_string_ptr("true", ".string.true"),
                span,
            )?;
            let no = build(
                self.builder
                    .build_global_string_ptr("false", ".string.false"),
                span,
            )?;
            let raw = build(
                self.builder.build_select(
                    value.int(span)?,
                    yes.as_pointer_value(),
                    no.as_pointer_value(),
                    "bool.string",
                ),
                span,
            )?;
            return self.copy_string(
                Value {
                    raw,
                    ty: Type::String,
                },
                span,
            );
        }
        let name = match value.ty {
            Type::Int => "cheeku_to_string_int",
            Type::Float => "cheeku_to_string_float",
            _ => return Err(invalid("to_string expects a scalar", span)),
        };
        let parameter = self.llvm_type(&value.ty, span)?;
        let function = self.external(
            name,
            self.context
                .ptr_type(AddressSpace::default())
                .fn_type(&[parameter.into()], false),
        )?;
        let raw = self.call_value(function, &[value.raw.into()], span)?;
        let valid = build(
            self.builder
                .build_is_not_null(raw.into_pointer_value(), "format.allocated"),
            span,
        )?;
        self.guard(valid, "string conversion allocation failed", span)?;
        Ok(Value {
            raw,
            ty: Type::String,
        })
    }
}
