use crate::token::Span;
use crate::types::{ScalarType, Type};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    Integer(String),
    Float(String),
    String(String),
    Bool(bool),
    Error {
        payload: ScalarType,
        code: Box<Expr>,
    },
    Negate(Box<Expr>),
    Call {
        name: String,
        arguments: Vec<Expr>,
    },
    Array(Vec<Expr>),
    Index {
        array: Box<Expr>,
        index: Box<Expr>,
    },
    Identifier(String),
    /// Reads a record field, written `object.field`. The span ends with the field
    /// name, which is therefore the last `field.len()` bytes of it.
    FieldAccess {
        object: Box<Expr>,
        field: String,
    },
    /// A record value written as `Name { field: expression ... }`.
    RecordInit {
        name: String,
        fields: Vec<FieldInit>,
    },
    Binary {
        left: Box<Expr>,
        operator: BinaryOperator,
        right: Box<Expr>,
    },
}

/// One `field: expression` entry of a record initializer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInit {
    pub name: String,
    pub value: Expr,
    /// Covers the field name through the end of its value.
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl BinaryOperator {
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Equal
                | Self::NotEqual
                | Self::Less
                | Self::LessEqual
                | Self::Greater
                | Self::GreaterEqual
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    /// The annotation as written; a record name is resolved by the type checker.
    pub ty: FieldType,
    /// Covers the parameter name.
    pub span: Span,
    /// Covers the type annotation.
    pub ty_span: Span,
}

/// The type written after a record field's `:`. Built-in types reuse [`Type`];
/// any other name is kept as written so nested records can resolve it later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldType {
    Builtin(Type),
    Named(String),
}

/// A built-in annotation compares equal to the [`Type`] it names, so code that
/// only deals with built-in types can keep comparing against `Type` directly.
impl PartialEq<Type> for FieldType {
    fn eq(&self, other: &Type) -> bool {
        matches!(self, Self::Builtin(ty) if ty == other)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordField {
    pub name: String,
    pub ty: FieldType,
    /// Covers the field name through its type.
    pub span: Span,
}

/// One `.field` of a [`FieldTarget`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldName {
    pub name: String,
    /// Covers only the field name, not the dot.
    pub span: Span,
}

/// The place written by `reassign variable.field...`: a field of a record
/// variable, possibly nested. Only this shape can be a target, so a call result or
/// a temporary record cannot be represented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldTarget {
    pub variable: String,
    pub variable_span: Span,
    /// The fields from the variable outward; never empty.
    pub fields: Vec<FieldName>,
    /// Covers the variable through the last field.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StmtKind {
    /// Declares a variable using `assign name as value`.
    Set {
        name: String,
        value: Expr,
    },
    Show(Expr),
    Assign {
        name: String,
        value: Expr,
    },
    /// Replaces a record field using `reassign variable.field as value`.
    AssignField {
        target: FieldTarget,
        value: Expr,
    },
    If {
        condition: Expr,
        then_body: Vec<Stmt>,
        else_body: Vec<Stmt>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    Function {
        name: String,
        parameters: Vec<Parameter>,
        return_type: FieldType,
        /// Covers the return type annotation.
        return_span: Span,
        body: Vec<Stmt>,
    },
    Return(Expr),
    /// Declares a record type using `record Name { field: type ... }`.
    Record {
        name: String,
        fields: Vec<RecordField>,
    },
}
