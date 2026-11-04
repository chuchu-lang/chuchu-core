use std::collections::{HashMap, hash_map::Entry};

use crate::types::Type;

#[derive(Debug, PartialEq, Eq)]
pub enum SymbolError {
    DuplicateDeclaration { name: String },
}

#[derive(Debug, Default)]
pub struct SymbolTable {
    symbols: HashMap<String, Type>,
    scopes: Vec<HashMap<String, Type>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn declare(&mut self, name: String, ty: Type) -> Result<(), SymbolError> {
        let symbols = self.scopes.last_mut().unwrap_or(&mut self.symbols);
        match symbols.entry(name) {
            Entry::Vacant(entry) => {
                entry.insert(ty);
                Ok(())
            }
            Entry::Occupied(entry) => Err(SymbolError::DuplicateDeclaration {
                name: entry.key().clone(),
            }),
        }
    }

    pub fn lookup(&self, name: &str) -> Option<Type> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .or_else(|| self.symbols.get(name).cloned())
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
