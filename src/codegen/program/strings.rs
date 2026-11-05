//! Immutable UTF-8 values with explicit ownership in native lowering.
//!
//! Every string expression produces an owned allocation. Bindings and function
//! arguments take ownership; identifier reads copy, and returns transfer their
//! expression's allocation after cleaning up locals. No stack pointer escapes.
//!
//! A record owns the strings in its fields, including those of records nested in
//! it, so records follow the same rules: copying a record copies every string it
//! owns, and releasing one releases them. A record of only numbers and booleans
//! owns nothing and is an ordinary value.
use super::*;

impl<'ctx> Generator<'ctx> {
    fn string_pointer(
        &self,
        value: Value<'ctx>,
        span: Span,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        if value.ty == Type::String
            && let BasicValueEnum::PointerValue(pointer) = value.raw
        {
            Ok(pointer)
        } else {
            Err(invalid("Expected string value", span))
        }
    }

    fn string_bytes(
        &self,
        pointer: PointerValue<'ctx>,
        span: Span,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let function = self.external(
            "strlen",
            self.context
                .i64_type()
                .fn_type(&[pointer.get_type().into()], false),
        )?;
        Ok(self
            .call_value(function, &[pointer.into()], span)?
            .into_int_value())
    }

    fn allocate_string(
        &mut self,
        length: IntValue<'ctx>,
        span: Span,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let bytes = self.checked_int(
            Op::Add,
            length,
            self.context.i64_type().const_int(1, false),
            span,
        )?;
        let malloc = self.external(
            "malloc",
            self.context
                .ptr_type(AddressSpace::default())
                .fn_type(&[self.context.i64_type().into()], false),
        )?;
        let pointer = self
            .call_value(malloc, &[bytes.into()], span)?
            .into_pointer_value();
        let valid = build(
            self.builder.build_is_not_null(pointer, "string.allocated"),
            span,
        )?;
        self.guard(valid, "string allocation failed", span)?;
        Ok(pointer)
    }

    pub(super) fn copy_string(
        &mut self,
        value: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let source = self.string_pointer(value, span)?;
        let length = self.string_bytes(source, span)?;
        let destination = self.allocate_string(length, span)?;
        let bytes = build(
            self.builder.build_int_add(
                length,
                self.context.i64_type().const_int(1, false),
                "string.copy.bytes",
            ),
            span,
        )?;
        build(
            self.builder.build_memcpy(destination, 1, source, 1, bytes),
            span,
        )?;
        Ok(Value {
            raw: destination.into(),
            ty: Type::String,
        })
    }

    pub(super) fn string_literal(
        &mut self,
        literal: &str,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        if literal.contains('\0') {
            return Err(invalid("Strings cannot contain NUL characters", span));
        }
        let global = build(
            self.builder
                .build_global_string_ptr(literal, ".chuchu.string"),
            span,
        )?;
        self.copy_string(
            Value {
                raw: global.as_pointer_value().into(),
                ty: Type::String,
            },
            span,
        )
    }

    /// Whether a value of this type owns allocations that must be copied with it
    /// and released after it: strings, results of strings, and records that
    /// contain either, however deeply nested.
    pub(super) fn owns(&self, ty: &Type) -> bool {
        match ty {
            Type::String | Type::Result(ScalarType::String) => true,
            Type::Record(id) => self
                .records
                .get(*id)
                .is_some_and(|layout| layout.fields().iter().any(|field| self.owns(&field.ty))),
            _ => false,
        }
    }

    /// An independent copy of a value: every allocation it owns is copied, so
    /// changing or releasing either one never affects the other.
    pub(super) fn copy_value(
        &mut self,
        value: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        match &value.ty {
            Type::String => self.copy_string(value, span),
            Type::Result(ScalarType::String) => self.copy_result(value, span),
            Type::Record(id) if self.owns(&value.ty) => {
                let id = *id;
                let layout = self
                    .records
                    .get(id)
                    .ok_or(invalid("Record type has no layout", span))?
                    .clone();
                let BasicValueEnum::StructValue(mut aggregate) = value.raw else {
                    return Err(invalid("Expected record LLVM value", span));
                };
                // Fields that own nothing are already copied by the value itself.
                let owning: Vec<_> = layout
                    .fields()
                    .iter()
                    .filter(|field| self.owns(&field.ty))
                    .cloned()
                    .collect();
                for field in &owning {
                    let raw = build(
                        self.builder
                            .build_extract_value(aggregate, field.index, &field.name),
                        span,
                    )?;
                    let copy = self.copy_value(Value { raw, ty: field.ty }, span)?;
                    aggregate = build(
                        self.builder.build_insert_value(
                            aggregate,
                            copy.raw,
                            field.index,
                            &field.name,
                        ),
                        span,
                    )?
                    .into_struct_value();
                }
                Ok(Value {
                    raw: aggregate.into(),
                    ty: value.ty,
                })
            }
            _ => Ok(value),
        }
    }

    /// Releases every allocation a value owns. The value must not be used again.
    pub(super) fn release_value(&self, value: Value<'ctx>, span: Span) -> Result<(), CodegenError> {
        match &value.ty {
            Type::Result(ScalarType::String) => {
                let payload = self.result_payload(value, span)?;
                self.release_value(payload, span)?;
            }
            Type::String => {
                let pointer = self.string_pointer(value, span)?;
                let free = self.external(
                    "free",
                    self.context
                        .void_type()
                        .fn_type(&[pointer.get_type().into()], false),
                )?;
                build(self.builder.build_call(free, &[pointer.into()], ""), span)?;
            }
            Type::Record(id) if self.owns(&value.ty) => {
                let id = *id;
                let layout = self
                    .records
                    .get(id)
                    .ok_or(invalid("Record type has no layout", span))?;
                let BasicValueEnum::StructValue(aggregate) = value.raw else {
                    return Err(invalid("Expected record LLVM value", span));
                };
                for field in layout.fields().iter().filter(|field| self.owns(&field.ty)) {
                    let raw = build(
                        self.builder
                            .build_extract_value(aggregate, field.index, &field.name),
                        span,
                    )?;
                    self.release_value(Value { raw, ty: field.ty }, span)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Releases what the storage at `pointer` owns, leaving the storage itself
    /// stale: the caller must store a new value or stop using it.
    pub(super) fn release_at(
        &self,
        pointer: PointerValue<'ctx>,
        ty: Type,
        span: Span,
    ) -> Result<(), CodegenError> {
        if self.owns(&ty) {
            let raw = build(
                self.builder
                    .build_load(self.llvm_type(&ty, span)?, pointer, "release"),
                span,
            )?;
            self.release_value(Value { raw, ty }, span)?;
        }
        Ok(())
    }

    pub(super) fn release_binding(
        &self,
        binding: Binding<'ctx>,
        span: Span,
    ) -> Result<(), CodegenError> {
        self.release_at(binding.pointer, binding.ty, span)
    }

    pub(super) fn cleanup_strings(&self, all_scopes: bool, span: Span) -> Result<(), CodegenError> {
        self.set_location(span)?;
        let scopes = if all_scopes {
            self.scopes.as_slice()
        } else {
            &self.scopes[self.scopes.len() - 1..]
        };
        for scope in scopes.iter().rev() {
            for binding in scope.values() {
                self.release_binding(binding.clone(), span)?;
            }
        }
        Ok(())
    }

    pub(super) fn string_binary(
        &mut self,
        op: Op,
        left: Value<'ctx>,
        right: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let a = self.string_pointer(left.clone(), span)?;
        let b = self.string_pointer(right.clone(), span)?;
        let result = match op {
            Op::Add => {
                let a_len = self.string_bytes(a, span)?;
                let b_len = self.string_bytes(b, span)?;
                let length = self.checked_int(Op::Add, a_len, b_len, span)?;
                let destination = self.allocate_string(length, span)?;
                build(self.builder.build_memcpy(destination, 1, a, 1, a_len), span)?;
                // SAFETY: allocation contains a_len + b_len + 1 bytes. The
                // offset is within it, including when either operand is empty.
                let tail = unsafe {
                    build(
                        self.builder.build_gep(
                            self.context.i8_type(),
                            destination,
                            &[a_len],
                            "string.tail",
                        ),
                        span,
                    )?
                };
                let b_bytes = build(
                    self.builder.build_int_add(
                        b_len,
                        self.context.i64_type().const_int(1, false),
                        "string.tail.bytes",
                    ),
                    span,
                )?;
                build(self.builder.build_memcpy(tail, 1, b, 1, b_bytes), span)?;
                Value {
                    raw: destination.into(),
                    ty: Type::String,
                }
            }
            Op::Equal | Op::NotEqual => {
                let compare = self.external(
                    "strcmp",
                    self.context
                        .i32_type()
                        .fn_type(&[a.get_type().into(), b.get_type().into()], false),
                )?;
                let ordering = self
                    .call_value(compare, &[a.into(), b.into()], span)?
                    .into_int_value();
                let equal = build(
                    self.builder.build_int_compare(
                        if op == Op::Equal {
                            IntPredicate::EQ
                        } else {
                            IntPredicate::NE
                        },
                        ordering,
                        self.context.i32_type().const_zero(),
                        "string.compare",
                    ),
                    span,
                )?;
                Value {
                    raw: equal.into(),
                    ty: Type::Bool,
                }
            }
            _ => return Err(invalid("Strings support +, ==, and !=", span)),
        };
        self.release_value(left, span)?;
        self.release_value(right, span)?;
        Ok(result)
    }

    /// Count Unicode scalar values, not bytes or user-perceived grapheme clusters.
    /// Valid UTF-8 comes from literals and concatenation, so count non-continuation
    /// bytes. This loop keeps temporary storage bounded independently of length.
    pub(super) fn string_len(
        &mut self,
        value: Value<'ctx>,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let pointer = self.string_pointer(value.clone(), span)?;
        let function = self.function(span)?;
        let entry = self
            .builder
            .get_insert_block()
            .ok_or(CodegenError::InvalidInsertionPoint { span })?;
        let test = self.context.append_basic_block(function, "string.len.test");
        let body = self.context.append_basic_block(function, "string.len.body");
        let done = self.context.append_basic_block(function, "string.len.done");
        build(self.builder.build_unconditional_branch(test), span)?;
        self.builder.position_at_end(test);
        let offset = build(
            self.builder
                .build_phi(self.context.i64_type(), "string.offset"),
            span,
        )?;
        let count = build(
            self.builder
                .build_phi(self.context.i64_type(), "string.characters"),
            span,
        )?;
        let zero = self.context.i64_type().const_zero();
        offset.add_incoming(&[(&zero, entry)]);
        count.add_incoming(&[(&zero, entry)]);
        // SAFETY: offset advances through a valid NUL-terminated allocation;
        // the terminator is checked before advancing to the next byte.
        let address = unsafe {
            build(
                self.builder.build_gep(
                    self.context.i8_type(),
                    pointer,
                    &[offset.as_basic_value().into_int_value()],
                    "string.byte",
                ),
                span,
            )?
        };
        let byte = build(
            self.builder
                .build_load(self.context.i8_type(), address, "utf8.byte"),
            span,
        )?
        .into_int_value();
        let end = build(
            self.builder.build_int_compare(
                IntPredicate::EQ,
                byte,
                self.context.i8_type().const_zero(),
                "string.end",
            ),
            span,
        )?;
        build(self.builder.build_conditional_branch(end, done, body), span)?;
        self.builder.position_at_end(body);
        let tag = build(
            self.builder.build_and(
                byte,
                self.context.i8_type().const_int(0xc0, false),
                "utf8.tag",
            ),
            span,
        )?;
        let leading = build(
            self.builder.build_int_compare(
                IntPredicate::NE,
                tag,
                self.context.i8_type().const_int(0x80, false),
                "utf8.leading",
            ),
            span,
        )?;
        let increment = build(
            self.builder
                .build_int_z_extend(leading, self.context.i64_type(), "utf8.increment"),
            span,
        )?;
        let next_count = build(
            self.builder.build_int_add(
                count.as_basic_value().into_int_value(),
                increment,
                "string.next.count",
            ),
            span,
        )?;
        let next_offset = build(
            self.builder.build_int_add(
                offset.as_basic_value().into_int_value(),
                self.context.i64_type().const_int(1, false),
                "string.next.offset",
            ),
            span,
        )?;
        build(self.builder.build_unconditional_branch(test), span)?;
        offset.add_incoming(&[(&next_offset, body)]);
        count.add_incoming(&[(&next_count, body)]);
        self.builder.position_at_end(done);
        self.release_value(value, span)?;
        Ok(Value {
            raw: count.as_basic_value(),
            ty: Type::Int,
        })
    }
}
