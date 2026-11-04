//! LLVM representations of validated records. Each record becomes one named struct
//! type whose fields keep declaration order, so a field's index is its position in
//! the declaration and never depends on hashing or on how the program is lowered.
//!
//! Fields are `int` (`i64`), `float` (`f64`), `bool` (`i1`), `string` (a pointer to
//! an owned allocation), or another record, embedded by value as that record's own
//! struct. Records cannot contain themselves, so nesting is finite. This module
//! builds types and field indices only; initializing, reading, writing, copying,
//! and releasing record values is lowered elsewhere.
use inkwell::AddressSpace;
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
    ///
    /// Every struct is created first, so records may refer to ones declared later.
    /// A struct embedded by value must be complete before the struct that embeds it,
    /// so bodies are filled in dependency order.
    pub fn new(context: &'ctx Context, table: &RecordTable) -> Result<Self, CodegenError> {
        let mut layouts = Vec::with_capacity(table.len());
        for (id, record) in table.iter() {
            let mut fields = Vec::with_capacity(record.fields.len());
            for (position, field) in record.fields.iter().enumerate() {
                fields.push(FieldLayout {
                    name: field.name.clone(),
                    index: u32::try_from(position)
                        .map_err(|_| invalid("Too many record fields", field.span))?,
                    ty: field.ty,
                });
            }
            layouts.push(RecordLayout {
                id,
                name: record.name.clone(),
                struct_type: context.opaque_struct_type(&format!("chuchu.record.{}", record.name)),
                fields,
            });
        }
        let structs: Vec<StructType<'ctx>> =
            layouts.iter().map(|layout| layout.struct_type).collect();
        let mut done = vec![false; layouts.len()];
        for (id, _) in table.iter() {
            define(context, table, &structs, &mut done, &mut Vec::new(), id)?;
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

/// Sets the body of `id` after the bodies of every record it embeds.
/// `path` holds the records being defined, to refuse a cycle instead of recursing
/// forever; the checker rejects cycles first, so reaching one is a compiler bug.
fn define<'ctx>(
    context: &'ctx Context,
    table: &RecordTable,
    structs: &[StructType<'ctx>],
    done: &mut [bool],
    path: &mut Vec<RecordId>,
    id: RecordId,
) -> Result<(), CodegenError> {
    if done[id.0] {
        return Ok(());
    }
    let record = table.get(id).ok_or(invalid(
        "Record type has no layout",
        Span { start: 0, end: 0 },
    ))?;
    if path.contains(&id) {
        return Err(invalid("Record contains itself", record.span));
    }
    path.push(id);
    let mut types = Vec::with_capacity(record.fields.len());
    for field in &record.fields {
        if let Type::Record(inner) = field.ty {
            define(context, table, structs, done, path, inner)?;
        }
        types.push(field_type(context, structs, field.ty, field.span)?);
    }
    path.pop();
    // Not packed: LLVM applies the target's natural alignment.
    structs[id.0].set_body(&types, false);
    done[id.0] = true;
    Ok(())
}

/// The LLVM type of a field: the same types as locals, so a later load or store
/// of a field uses the type the program already uses for that value. A string is
/// the pointer to its allocation, and a record is embedded by value.
fn field_type<'ctx>(
    context: &'ctx Context,
    structs: &[StructType<'ctx>],
    ty: Type,
    span: Span,
) -> Result<BasicTypeEnum<'ctx>, CodegenError> {
    match ty {
        Type::Int => Ok(context.i64_type().into()),
        Type::Float => Ok(context.f64_type().into()),
        Type::Bool => Ok(context.bool_type().into()),
        Type::String => Ok(context.ptr_type(AddressSpace::default()).into()),
        Type::Record(id) => structs
            .get(id.0)
            .map(|inner| (*inner).into())
            .ok_or(invalid("Record type has no layout", span)),
        _ => Err(invalid(
            "Record fields must be int, float, bool, string, or a record",
            span,
        )),
    }
}
