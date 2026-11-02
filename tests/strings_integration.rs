mod support;
use chuchu::ast::{ExprKind, StmtKind};
use chuchu::compiler::compile_file_with_options;
use chuchu::diagnostic::render;
use chuchu::lexer::{LexErrorKind, tokenize};
use chuchu::optimization::{BuildOptions, DebugInfo, Optimization};
use chuchu::parser::Parser;
use chuchu::symbol_table::SymbolTable;
use chuchu::token::TokenKind;
use chuchu::type_checker::TypeChecker;
use chuchu::types::Type;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cheeku strings {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn string_tokens_decode_escapes_and_keep_original_utf8_byte_spans() {
    let source = r#"show "héllo 🌍\n\t\r\"\\""#;
    let tokens = tokenize(source).unwrap();
    assert_eq!(
        tokens[1].kind,
        TokenKind::String("héllo 🌍\n\t\r\"\\".into())
    );
    assert_eq!(tokens[1].span.start, 5);
    assert_eq!(tokens[1].span.end, source.len());
    assert_eq!(
        tokenize(r#""""#).unwrap()[0].kind,
        TokenKind::String(String::new())
    );
}

#[test]
fn malformed_strings_report_specific_source_diagnostics() {
    for (source, kind, message) in [
        (
            "show \"unfinished",
            LexErrorKind::UnterminatedString,
            "Unterminated string literal",
        ),
        (
            "show \"unfinished\nshow 1",
            LexErrorKind::UnterminatedString,
            "Unterminated string literal",
        ),
        (
            "show \"unfinished\\\n",
            LexErrorKind::UnterminatedString,
            "Unterminated string literal",
        ),
        (
            r#"show "bad\q""#,
            LexErrorKind::InvalidEscape,
            "Invalid string escape \\q",
        ),
        (
            r#"show "bad\0""#,
            LexErrorKind::NulInString,
            "NUL characters are not supported",
        ),
        (
            "show \"bad\0\"",
            LexErrorKind::NulInString,
            "NUL characters are not supported",
        ),
    ] {
        let error = tokenize(source).unwrap_err();
        assert_eq!(error.kind, kind);
        let diagnostic = render(
            &chuchu::compiler::CompileError::Lex(error),
            Path::new("strings.chuchu"),
            source,
        );
        assert!(diagnostic.contains(message), "{diagnostic}");
        assert!(diagnostic.contains("strings.chuchu:1:"));
        assert!(diagnostic.contains('^'));
    }
}

#[test]
fn strings_infer_types_and_support_function_annotations() {
    let source = r#"function echo(value: string) be string { return value }
assign greeting as echo("hello")
reassign greeting as greeting + "!"
show greeting == "hello!"
show length(greeting)"#;
    let statements = Parser::new(tokenize(source).unwrap())
        .parse_program()
        .unwrap();
    let StmtKind::Function {
        parameters,
        return_type,
        ..
    } = &statements[0].kind
    else {
        panic!("expected function")
    };
    assert_eq!(parameters[0].ty, Type::String);
    assert_eq!(*return_type, Type::String);
    let expression = Parser::new(tokenize(r#""text""#).unwrap())
        .parse_expression()
        .unwrap();
    assert!(matches!(expression.kind, ExprKind::String(_)));
    assert_eq!(
        TypeChecker::new(SymbolTable::new()).infer_expression(&expression),
        Ok(Type::String)
    );
    TypeChecker::new(SymbolTable::new())
        .check_program(&statements)
        .unwrap();
}

#[test]
fn rejects_mixed_types_and_unsupported_string_operations() {
    for source in [
        r#"show "text" + 1"#,
        r#"show "1" == 1"#,
        r#"show "a" - "b""#,
        r#"show "a" < "b""#,
        r#"show -"text""#,
        r#"show ["text"]"#,
        r#"assign text as "hello"
reassign text as 1"#,
        r#"function echo(s: string) be string { return 1 }"#,
        r#"function echo(s: string) be string { return s }
show echo(1)"#,
        r#"show "hello"[0]"#,
        r#"if "text" { show 1 }"#,
    ] {
        let statements = Parser::new(tokenize(source).unwrap())
            .parse_program()
            .unwrap();
        assert!(
            TypeChecker::new(SymbolTable::new())
                .check_program(&statements)
                .is_err(),
            "{source}"
        );
    }
}

#[test]
fn native_strings_work_at_every_optimization_level_with_and_without_debugging() {
    let directory = Directory::new();
    let input = directory.0.join("strings with spaces.chuchu");
    let output = directory.0.join(support::executable_name("strings"));
    let source = r#"function greet(name: string) be string {
    assign prefix as "Hello, "
    return prefix + name + "!"
}
assign name as "دنیا 🌍"
assign greeting as greet(name)
assign saved as greeting
reassign greeting as greeting + " again"
show saved
show greeting
show length(name)
show "" + "empty"
show length("")
show length("é中🌍")
show length("é")
show "x" == "x"
show "x" != "y"
show "x" == "y"
show "x" != "x"
show "100% %s %n"
show "quote: \" slash: \\ tab:\t"
show "line1\nline2"
if true {
    assign name as "inner"
    show name
}
show name
reassign name as name
show name
"#;
    fs::write(&input, source.replace('\n', "\r\n")).unwrap();
    let expected = "Hello, دنیا 🌍!\nHello, دنیا 🌍! again\n6\nempty\n0\n3\n2\ntrue\ntrue\nfalse\nfalse\n100% %s %n\nquote: \" slash: \\ tab:\t\nline1\nline2\ninner\nدنیا 🌍\nدنیا 🌍\n";
    for optimization in [
        Optimization::O0,
        Optimization::O1,
        Optimization::O2,
        Optimization::O3,
    ] {
        for debug_info in [DebugInfo::None, DebugInfo::Full] {
            compile_file_with_options(
                &input,
                &output,
                &BuildOptions {
                    optimization,
                    debug_info,
                    ..BuildOptions::default()
                },
            )
            .unwrap();
            let result = Command::new(&output).output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(result.stdout, support::native_output(expected));
            assert!(result.stderr.is_empty());
        }
    }
}

#[test]
fn returns_recursion_scopes_and_reassignment_keep_strings_alive() {
    let directory = Directory::new();
    let input = directory.0.join("ownership.chuchu");
    let output = directory.0.join(support::executable_name("ownership"));
    let source = r#"function recursive(value: string, n: int) be string {
    assign local as value + "!"
    if n == 0 { return local }
    while n > 0 {
        assign temporary as "scope"
        if n == 1 { return local + temporary }
        return recursive(local, n - 1)
    }
    return "unreachable"
}
function consume(value: string) be int { return length(value) }
assign x as "a"
assign y as x
reassign x as "b"
show y
show x
show recursive("start", 3)
show consume("temporary" + " value")
assign count as 0
while count < 2000 {
    assign temporary as recursive("x", 2)
    reassign temporary as temporary + "!"
    reassign count as count + 1
}
show count
"#;
    fs::write(&input, source).unwrap();
    for optimization in [Optimization::O0, Optimization::O3] {
        compile_file_with_options(
            &input,
            &output,
            &BuildOptions {
                optimization,
                ..BuildOptions::default()
            },
        )
        .unwrap();
        let result = Command::new(&output).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            result.stdout,
            support::native_output("a\nb\nstart!!!scope\n15\n2000\n")
        );
    }
}
