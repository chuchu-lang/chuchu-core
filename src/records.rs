//! Validated record metadata, handed from the type checker to code generation.
//!
//! A [`RecordTable`] is only produced for a program whose record declarations all
//! passed validation, so code generation never sees duplicate names, unresolved
//! field types, or records that contain themselves. Records are listed in
//! declaration order, and `RecordId(n)` is the `n`-th record, which is the same
//! id the checker puts in [`Type::Record`].
use crate::token::Span;
use crate::types::{RecordId, Type};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    pub name: String,
    /// The declared type; a record-typed field holds that record's [`Type::Record`].
    pub ty: Type,
    /// Covers the field name through its type.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordInfo {
    pub name: String,
    /// The fields in declaration order.
    pub fields: Vec<FieldInfo>,
    /// The whole declaration.
    pub span: Span,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordTable {
    records: Vec<RecordInfo>,
}

impl RecordTable {
    pub(crate) fn new(records: Vec<RecordInfo>) -> Self {
        Self { records }
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
    pub fn get(&self, id: RecordId) -> Option<&RecordInfo> {
        self.records.get(id.0)
    }
    pub fn id_of(&self, name: &str) -> Option<RecordId> {
        self.records
            .iter()
            .position(|record| record.name == name)
            .map(RecordId)
    }
    /// The records in declaration order, with their ids.
    pub fn iter(&self) -> impl Iterator<Item = (RecordId, &RecordInfo)> {
        self.records
            .iter()
            .enumerate()
            .map(|(index, record)| (RecordId(index), record))
    }
}
