pub mod ast;
pub mod codegen;
pub mod compiler;
pub mod diagnostic;
pub mod lexer;
pub mod modules;
pub mod native;
pub mod optimization;
pub mod parser;
pub mod records;
pub mod runtime;
pub mod source;
pub mod symbol_table;
pub mod token;
pub mod type_checker;
pub mod types;

pub fn run() {
    println!("Chuchu compiler initialized");
}
