use std::collections::{HashMap, HashSet, hash_map::Entry};

use crate::types::Type;

#[derive(Debug, PartialEq, Eq)]
pub enum SymbolError {
    DuplicateDeclaration { name: String },
}

/// What a name refers to in one scope.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Slot {
    /// Declared with `assign`: the type is known and the value always exists.
    Value(Type),
    /// Declared with `declare`: the type comes from the first assignment, and
    /// `assigned` records whether every path reaching this point has made one.
    Deferred { ty: Option<Type>, assigned: bool },
}

/// How a name resolves for assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Value(Type),
    /// A `declare`d variable; the type is `None` until its first assignment.
    Deferred(Option<Type>),
}

#[derive(Debug, Default)]
pub struct SymbolTable {
    symbols: HashMap<String, Slot>,
    scopes: Vec<HashMap<String, Slot>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, name: String, slot: Slot) -> Result<(), SymbolError> {
        let symbols = self.scopes.last_mut().unwrap_or(&mut self.symbols);
        match symbols.entry(name) {
            Entry::Vacant(entry) => {
                entry.insert(slot);
                Ok(())
            }
            Entry::Occupied(entry) => Err(SymbolError::DuplicateDeclaration {
                name: entry.key().clone(),
            }),
        }
    }

    pub fn declare(&mut self, name: String, ty: Type) -> Result<(), SymbolError> {
        self.insert(name, Slot::Value(ty))
    }

    /// Declares a variable whose type is decided by its first assignment.
    pub fn declare_deferred(&mut self, name: String) -> Result<(), SymbolError> {
        self.insert(
            name,
            Slot::Deferred {
                ty: None,
                assigned: false,
            },
        )
    }

    fn slot(&self, name: &str) -> Option<&Slot> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.symbols.get(name))
    }

    fn slot_mut(&mut self, name: &str) -> Option<&mut Slot> {
        match self
            .scopes
            .iter()
            .rposition(|scope| scope.contains_key(name))
        {
            Some(index) => self.scopes[index].get_mut(name),
            None => self.symbols.get_mut(name),
        }
    }

    /// The type of a variable that may be read now. A `declare`d variable only
    /// counts once every path to this point has assigned it.
    pub fn lookup(&self, name: &str) -> Option<Type> {
        match self.slot(name)? {
            Slot::Value(ty) => Some(*ty),
            Slot::Deferred {
                ty: Some(ty),
                assigned: true,
            } => Some(*ty),
            Slot::Deferred { .. } => None,
        }
    }

    /// True for a `declare`d variable that cannot be read yet.
    pub fn is_unassigned(&self, name: &str) -> bool {
        matches!(
            self.slot(name),
            Some(Slot::Deferred {
                assigned: false,
                ..
            }) | Some(Slot::Deferred { ty: None, .. })
        )
    }

    /// Resolves a name as the target of an assignment.
    pub fn resolve(&self, name: &str) -> Option<Resolved> {
        match self.slot(name)? {
            Slot::Value(ty) => Some(Resolved::Value(*ty)),
            Slot::Deferred { ty, .. } => Some(Resolved::Deferred(*ty)),
        }
    }

    /// Records an assignment to a `declare`d variable, fixing its type on the
    /// first one.
    pub fn assign_deferred(&mut self, name: &str, value: Type) {
        if let Some(Slot::Deferred { ty, assigned }) = self.slot_mut(name) {
            ty.get_or_insert(value);
            *assigned = true;
        }
    }

    /// The `declare`d variables assigned on every path so far, identified by
    /// scope depth and name.
    pub fn assigned_set(&self) -> HashSet<(usize, String)> {
        let mut set = HashSet::new();
        let global = std::iter::once((0, &self.symbols));
        let nested = self
            .scopes
            .iter()
            .enumerate()
            .map(|(index, scope)| (index + 1, scope));
        for (depth, scope) in global.chain(nested) {
            for (name, slot) in scope {
                if matches!(slot, Slot::Deferred { assigned: true, .. }) {
                    set.insert((depth, name.clone()));
                }
            }
        }
        set
    }

    /// Makes exactly the variables in `set` count as assigned. Types learned
    /// from earlier assignments are kept.
    pub fn set_assigned(&mut self, set: &HashSet<(usize, String)>) {
        let global = std::iter::once((0, &mut self.symbols));
        let nested = self
            .scopes
            .iter_mut()
            .enumerate()
            .map(|(index, scope)| (index + 1, scope));
        for (depth, scope) in global.chain(nested) {
            for (name, slot) in scope.iter_mut() {
                if let Slot::Deferred { assigned, .. } = slot {
                    *assigned = set.contains(&(depth, name.clone()));
                }
            }
        }
    }

    pub fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    pub fn exit_scope(&mut self) {
        self.scopes.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_empty_symbol_table() {
        assert!(SymbolTable::new().symbols.is_empty());
        assert!(SymbolTable::default().symbols.is_empty());
    }

    #[test]
    fn declares_and_looks_up_variable() {
        let mut symbols = SymbolTable::new();
        symbols
            .declare("x".to_owned(), Type::Int)
            .expect("Variable should be declared");

        assert_eq!(symbols.lookup("x"), Some(Type::Int));
    }

    #[test]
    fn undeclared_variable_is_absent() {
        let mut symbols = SymbolTable::new();
        symbols
            .declare("x".to_owned(), Type::Int)
            .expect("Variable should be declared");

        assert_eq!(symbols.lookup("unknown"), None);
    }

    #[test]
    fn rejects_duplicate_without_overwriting_existing_entry() {
        let mut symbols = SymbolTable::new();
        symbols
            .declare("x".to_owned(), Type::Int)
            .expect("First declaration should succeed");
        let original = symbols.symbols.clone();

        assert_eq!(
            symbols.declare("x".to_owned(), Type::Int),
            Err(SymbolError::DuplicateDeclaration {
                name: "x".to_owned(),
            })
        );
        assert_eq!(symbols.symbols, original);
        assert_eq!(symbols.lookup("x"), Some(Type::Int));
    }
}
