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
    FieldAccess {
        target: Box<Expr>,
        field: String,
    },
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
    /// Declares a variable without a value using `declare name`. Its type is
    /// fixed by the first `reassign name as value` that follows.
    Declare {
        name: String,
    },
    Show(Expr),
    Assign {
        name: String,
        value: Expr,
    },
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

/// File-level syntax; module directives never enter executable statement lowering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleSyntax {
    pub name: Option<(String, Span)>,
    pub imports: Vec<Import>,
    pub exports: Vec<(String, Span)>,
    pub statements: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Relative source path derived from `use module library.math`.
    pub path: String,
    /// The last module component (`math`), used for qualified access.
    pub alias: String,
    pub span: Span,
}
