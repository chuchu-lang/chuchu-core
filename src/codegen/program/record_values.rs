//! Lowering of record initializers and field reads for records whose fields are
//! `int`, `float`, or `bool`. A record value is an LLVM struct value of the record's
//! layout, built with `insertvalue` and read with `extractvalue`, so it is a plain
//! value: copies cannot alias, and nothing is allocated or needs cleanup.
use super::*;
use crate::ast::FieldInit;

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
            *slot = Some(self.cast(value, field.ty, initializer.value.span)?.raw);
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

    /// Lowers `object.field` by reading one element of the object's struct value.
    /// The object may be a record variable or a temporary such as an initializer.
    pub(super) fn field_access(
        &mut self,
        object: &Expr,
        field: &str,
        span: Span,
    ) -> Result<Value<'ctx>, CodegenError> {
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
        Ok(Value {
            raw: build(
                self.builder
                    .build_extract_value(structure, field.index, &field.name),
                span,
            )?,
            ty: field.ty,
        })
    }
}
