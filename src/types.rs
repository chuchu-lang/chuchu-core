#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Bool,
    String,
    Result(ScalarType),
    Array {
        element: NumericType,
        len: usize,
    },
    /// A user-defined record, identified by its position among the declarations.
    Record(RecordId),
}

/// Identifies a declared record. Ids are handed out by the type checker in
/// declaration order, which keeps [`Type`] `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordId(pub usize);

/// The first result types carry one scalar. Nested results and array payloads
/// are deliberately excluded until the language has general aggregate types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarType {
    Int,
    Float,
    Bool,
    String,
}

impl ScalarType {
    pub fn ty(self) -> Type {
        match self {
            Self::Int => Type::Int,
            Self::Float => Type::Float,
            Self::Bool => Type::Bool,
            Self::String => Type::String,
        }
    }
    pub fn from_type(ty: &Type) -> Option<Self> {
        match ty {
            Type::Int => Some(Self::Int),
            Type::Float => Some(Self::Float),
            Type::Bool => Some(Self::Bool),
            Type::String => Some(Self::String),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericType {
    Int,
    Float,
}

impl NumericType {
    pub fn scalar(self) -> Type {
        match self {
            Self::Int => Type::Int,
            Self::Float => Type::Float,
        }
    }
}

impl Type {
    pub fn owns_string(&self) -> bool {
        matches!(
            self,
            Self::String | Self::Result(ScalarType::String) | Self::Record(_)
        )
    }
    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Int | Self::Float)
    }
    pub fn accepts(&self, source: &Self) -> bool {
        self == source || (*self == Self::Float && *source == Self::Int)
    }
}

pub const MAX_ARRAY_LEN: usize = 256;
