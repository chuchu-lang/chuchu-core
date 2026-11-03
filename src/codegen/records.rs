//! LLVM representations of validated records. Each record becomes one named struct
//! type whose fields keep declaration order, so a field's index is its position in
//! the declaration and never depends on hashing or on how the program is lowered.
//!
//! Only `int`, `float`, and `bool` fields have a layout so far. This module builds
//! types and field indices only; initializing, reading, writing, copying, and
//! cleaning up records are not lowered, so no record value is ever produced.
use inkwell::context::Context;
use inkwell::types::{BasicTypeEnum, StructType};

use super::CodegenError;
use crate::records::RecordTable;
use crate::token::Span;
use crate::types::{RecordId, Type};

/// A field's place in its record's LLVM struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldLayout {
    pub name: String,
    /// The LLVM element index: the field's position in the declaration.
    pub index: u32,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct RecordLayout<'ctx> {
    id: RecordId,
    name: String,
    struct_type: StructType<'ctx>,
    fields: Vec<FieldLayout>,
}

impl<'ctx> RecordLayout<'ctx> {
    pub fn id(&self) -> RecordId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The named LLVM struct, `%chuchu.record.<name>`.
    pub fn struct_type(&self) -> StructType<'ctx> {
        self.struct_type
    }
    /// The fields in declaration order; `fields()[i].index == i`.
    pub fn fields(&self) -> &[FieldLayout] {
        &self.fields
    }
    pub fn field(&self, name: &str) -> Option<&FieldLayout> {
        self.fields.iter().find(|field| field.name == name)
    }
    pub fn field_index(&self, name: &str) -> Option<u32> {
        self.field(name).map(|field| field.index)
    }
}

/// The layouts of every record of a program, indexed by [`RecordId`].
#[derive(Debug, Clone, Default)]
pub struct RecordLayouts<'ctx> {
    layouts: Vec<RecordLayout<'ctx>>,
}

impl<'ctx> RecordLayouts<'ctx> {
    /// Builds one struct type per record. Types are created in `context`; LLVM
    /// renames a struct whose name that context already holds, so build the layouts
    /// of one program once per context.
    pub fn new(context: &'ctx Context, table: &RecordTable) -> Result<Self, CodegenError> {
        let mut layouts = Vec::with_capacity(table.len());
        for (id, record) in table.iter() {
            let struct_type = context.opaque_struct_type(&format!("chuchu.record.{}", record.name));
            let mut types = Vec::with_capacity(record.fields.len());
            let mut fields = Vec::with_capacity(record.fields.len());
            for (position, field) in record.fields.iter().enumerate() {
                types.push(field_type(context, field.ty, field.span)?);
                fields.push(FieldLayout {
                    name: field.name.clone(),
                    index: u32::try_from(position)
                        .map_err(|_| invalid("Too many record fields", field.span))?,
                    ty: field.ty,
                });
            }
            // Not packed: LLVM applies the target's natural alignment.
            struct_type.set_body(&types, false);
            layouts.push(RecordLayout {
                id,
                name: record.name.clone(),
                struct_type,
                fields,
            });
        }
        Ok(Self { layouts })
    }
    pub fn len(&self) -> usize {
        self.layouts.len()
    }
    pub fn is_empty(&self) -> bool {
        self.layouts.is_empty()
    }
    pub fn get(&self, id: RecordId) -> Option<&RecordLayout<'ctx>> {
        self.layouts.get(id.0)
    }
    pub fn by_name(&self, name: &str) -> Option<&RecordLayout<'ctx>> {
        self.layouts.iter().find(|layout| layout.name == name)
    }
    pub fn iter(&self) -> impl Iterator<Item = &RecordLayout<'ctx>> {
        self.layouts.iter()
    }
}

fn invalid(message: &'static str, span: Span) -> CodegenError {
    CodegenError::InvalidType { message, span }
}

/// The LLVM type of a field: the same scalar types as locals, so a later load or
/// store of a field uses the type the program already uses for that value.
fn field_type<'ctx>(
    context: &'ctx Context,
    ty: Type,
    span: Span,
) -> Result<BasicTypeEnum<'ctx>, CodegenError> {
    match ty {
        Type::Int => Ok(context.i64_type().into()),
        Type::Float => Ok(context.f64_type().into()),
        Type::Bool => Ok(context.bool_type().into()),
        _ => Err(invalid(
            "Record layout supports only int, float, and bool fields",
            span,
        )),
    }
}
