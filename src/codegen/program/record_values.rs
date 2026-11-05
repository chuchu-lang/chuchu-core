//! Lowering of record initializers, field reads, and field writes. A record value
//! is an LLVM struct value of the record's layout, built with `insertvalue`. A
//! variable keeps it in one stack slot, and a field of a variable is addressed in
//! place with `getelementptr`, so reading or writing one field never copies or
//! touches the others. Strings in fields are owned allocations: a record that
//! holds them is copied and released field by field (see `strings.rs`).
use super::*;
use crate::ast::{FieldInit, FieldTarget};

impl<'ctx> Generator<'ctx> {
    /// Lowers `Name { field: value ... }`. Field values are evaluated in the order
    /// they are written, like any other expressions with side effects, and each is
    /// stored at its declared position, so the layout never depends on the order of
    /// the initializer. Every declared field must be given exactly once; no field is
    /// ever left undefined.
    pub(super) fn record_init(
        &mut self,
        name: &str,
        initializers: &[FieldInit],
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        let layout = self
            .records
            .by_name(name)
            .ok_or(invalid("Record type has no layout", span))?
            .clone();
        let mut slots: Vec<Option<BasicValueEnum<'ctx>>> = vec![None; layout.fields().len()];
        for initializer in initializers {
            let field = layout
                .field(&initializer.name)
                .ok_or(invalid("Record has no such field", initializer.span))?;
            let slot = slots
                .get_mut(field.index as usize)
                .ok_or(invalid("Record field index out of range", initializer.span))?;
            if slot.is_some() {
                return Err(invalid(
                    "Record field is initialized twice",
                    initializer.span,
                ));
            }
            let value = self.expression(&initializer.value)?;
            // The same conversion as elsewhere: an int fills a float field.
            *slot = Some(self.cast(value, &field.ty, initializer.value.span)?.raw);
        }
        let mut aggregate = layout.struct_type().get_undef();
        for (field, slot) in layout.fields().iter().zip(slots) {
            let raw = slot.ok_or(invalid("Record field is not initialized", span))?;
            aggregate = build(
                self.builder
                    .build_insert_value(aggregate, raw, field.index, &field.name),
                span,
            )?
            .into_struct_value();
        }
        Ok(Value {
            raw: aggregate.into(),
            ty: Type::Record(layout.id()),
        })
    }

    /// The address and type of a place that can be read or written in place: a
    /// variable, or a field of a place. `None` for anything else, such as a call
    /// result or an initializer, which exists only as a temporary value.
    fn place(&mut self, expr: &Expr) -> Result<Option<(PointerValue<'ctx>, Type)>, CodegenError> {
        match &expr.kind {
            ExprKind::Identifier(name) => {
                let binding = self.lookup(name, expr.span)?;
                Ok(Some((binding.pointer, binding.ty)))
            }
            ExprKind::FieldAccess { target, field } => match self.place(target)? {
                Some((pointer, ty)) => self.field_pointer(pointer, ty, field, expr.span).map(Some),
                None => Ok(None),
            },
            _ => Ok(None),
        }
    }

    /// The address of `field` inside the record of type `ty` stored at `pointer`,
    /// with the field's type.
    fn field_pointer(
        &self,
        pointer: PointerValue<'ctx>,
        ty: Type,
        field: &str,
        span: Span,
    ) -> Result<(PointerValue<'ctx>, Type), CodegenError> {
        let Type::Record(id) = ty else {
            return Err(invalid("Field access requires a record", span));
        };
        let layout = self
            .records
            .get(id)
            .ok_or(invalid("Record type has no layout", span))?;
        let field = layout
            .field(field)
            .ok_or(invalid("Record has no such field", span))?;
        let address = build(
            self.builder
                .build_struct_gep(layout.struct_type(), pointer, field.index, &field.name),
            span,
        )?;
        Ok((address, field.ty))
    }

    /// Lowers `object.field`. A field of a variable is read in place and only that
    /// field is copied. A field of a temporary, such as an initializer or a call
    /// result, is copied out of the temporary, which is then released.
    pub(super) fn field_access(
        &mut self,
        object: &Expr,
        field: &str,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
        if let Some((pointer, ty)) = self.place(object)? {
            let (address, field_ty) = self.field_pointer(pointer, ty, field, span)?;
            let raw = build(
                self.builder
                    .build_load(self.llvm_type(&field_ty, span)?, address, field),
                span,
            )?;
            return self.copy_value(Value { raw, ty: field_ty }, span);
        }
        let object = self.expression(object)?;
        let Type::Record(id) = object.ty else {
            return Err(invalid("Field access requires a record", span));
        };
        let BasicValueEnum::StructValue(structure) = object.raw else {
            return Err(invalid("Expected record LLVM value", span));
        };
        let field = self
            .records
            .get(id)
            .ok_or(invalid("Record type has no layout", span))?
            .field(field)
            .ok_or(invalid("Record has no such field", span))?
            .clone();
        let raw = build(
            self.builder
                .build_extract_value(structure, field.index, &field.name),
            span,
        )?;
        let copy = self.copy_value(Value { raw, ty: field.ty }, span)?;
        self.release_value(object, span)?;
        Ok(copy)
    }

    /// Lowers `reassign variable.field... as value`. The value is evaluated first,
    /// so it may read the field it replaces; it is an independent copy, so the
    /// old contents of the field can then be released and the new ones stored.
    /// Every other field of the record is left as it was.
    pub(super) fn assign_field(
        &mut self,
        target: &FieldTarget,
        value: &Expr,
        span: Span,
    ) -> Result<(), CodegenError> {
        let value = self.expression(value)?;
        let binding = self.lookup(&target.variable, target.variable_span)?;
        let (mut pointer, mut ty) = (binding.pointer, binding.ty);
        for field in &target.fields {
            (pointer, ty) = self.field_pointer(pointer, ty, &field.name, field.span)?;
        }
        let value = self.cast(value, &ty, span)?;
        self.release_at(pointer, ty, span)?;
        build(self.builder.build_store(pointer, value.raw), span)?;
        Ok(())
    }
}
