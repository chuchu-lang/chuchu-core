//! Type rules for the file built-ins (Issue #9, stage 3). These tests stop at the
//! type checker on purpose: execution arrives with code generation, so nothing
//! here compiles or runs a program except the diagnostic-rendering test, which
//! fails before code generation is reached.
use chuchu::compiler::compile_source;
use chuchu::diagnostic::render;
use chuchu::lexer::tokenize;
use chuchu::parser::Parser;
use chuchu::symbol_table::SymbolTable;
use chuchu::token::Span;
use chuchu::type_checker::{TypeChecker, TypeError, reserved_function};
use std::path::Path;

fn check(source: &str) -> Result<(), TypeError> {
    let statements = Parser::new(tokenize(source).unwrap())
        .parse_program()
        .unwrap_or_else(|error| panic!("{source:?} should parse: {error:?}"));
    TypeChecker::new(SymbolTable::new()).check_program(&statements)
}

/// Asserts that `source` fails inside a call to `name`, at exactly `call`, with
/// a message containing `message`.
fn assert_call_error(source: &str, call: &str, name: &str, message: &str) {
    let start = source.find(call).expect("call text is in the source");
    let expected = Span {
        start,
        end: start + call.len(),
    };
    match check(source) {
        Err(TypeError::InvalidFunction {
            name: actual_name,
            message: actual_message,
            span,
        }) => {
            assert_eq!(actual_name, name, "{source}");
            assert!(
                actual_message.contains(message),
                "{source}: {actual_message:?} should contain {message:?}"
            );
            assert_eq!(span, expected, "{source}");
        }
        other => panic!("{source}: expected a function error, got {other:?}"),
    }
}

const FILE_FUNCTIONS: [&str; 4] = ["read_file", "try_read_file", "write_file", "try_write_file"];

#[test]
fn each_builtin_has_exactly_its_specified_return_type() {
    let signatures = [
        ("string", r#"read_file("a.txt")"#),
        ("result<string>", r#"try_read_file("a.txt")"#),
        ("int", r#"write_file("a.txt", "text")"#),
        ("result<int>", r#"try_write_file("a.txt", "text")"#),
    ];
    let annotations = [
        "string",
        "int",
        "bool",
        "float",
        "result<string>",
        "result<int>",
        "result<float>",
        "result<bool>",
    ];
    for (right, expression) in signatures {
        let program =
            |annotation: &str| format!("function f() be {annotation} {{ return {expression} }}");
        assert_eq!(
            check(&program(right)),
            Ok(()),
            "{expression} should be {right}"
        );
        for wrong in annotations {
            // Integers already widen to floats in every return position.
            if wrong == right || (right == "int" && wrong == "float") {
                continue;
            }
            assert!(
                check(&program(wrong)).is_err(),
                "{expression} must not be accepted as {wrong}"
            );
        }
    }
}

#[test]
fn results_support_the_existing_result_operations() {
    for source in [
        r#"assign r as try_read_file("a.txt")
show is_ok(r)
show is_eof(r)
show error_code(r) + 1
show error_message(r) + "!"
if is_ok(r) { show unwrap(r) + "\n" }"#,
        r#"assign r as try_write_file("a.txt", "text")
show is_ok(r)
show is_eof(r)
show error_code(r) == 4
show error_message(r)
if is_ok(r) { show unwrap(r) + 1 }"#,
        // Payload types flow through unwrap into ordinary expressions.
        r#"show length(unwrap(try_read_file("a.txt")))"#,
        r#"show unwrap(try_write_file("a.txt", "x")) * 2"#,
        r#"show to_string(unwrap(try_write_file("a.txt", "x")))"#,
        // A failed result's code can be re-wrapped for either payload type.
        r#"assign r as try_read_file("a.txt")
show error_code(err<int>(error_code(r)))
show error_code(err<string>(error_code(r)))"#,
        // A result is a first-class value.
        r#"assign r as try_read_file("a.txt")
assign copy as r
reassign r as try_read_file("b.txt")
show is_ok(copy)"#,
    ] {
        assert_eq!(check(source), Ok(()), "{source}");
    }
}

#[test]
fn results_work_with_variables_functions_and_control_flow() {
    for source in [
        r#"assign text as read_file("a.txt")
reassign text as read_file("b.txt")
reassign text as text + "!"
show length(text)"#,
        r#"assign bytes as write_file("a.txt", "text")
reassign bytes as write_file("b.txt", "more")
reassign bytes as bytes + 1
assign wide as 1.5
reassign wide as write_file("c.txt", "x")"#,
        r#"function load(path: string) be result<string> { return try_read_file(path) }
function save(path: string, text: string) be int { return write_file(path, text) }
function try_save(path: string, text: string) be result<int> { return try_write_file(path, text) }
function relay(r: result<int>) be result<int> { return r }
show unwrap(relay(try_save("a.txt", unwrap(load("b.txt")))))
show save("c.txt", read_file("d.txt"))"#,
        r#"function copy_file(from: string, to: string) be result<int> {
    assign source as try_read_file(from)
    if is_ok(source) {
        return try_write_file(to, unwrap(source))
    }
    return err<int>(error_code(source))
}
assign copied as copy_file("a.txt", "b.txt")
if is_ok(copied) { show unwrap(copied) } else { show error_message(copied) }"#,
        // Arguments may be any string expression, including other calls.
        r#"assign name as "log"
show write_file(name + ".txt", read_file("in-" + name + ".txt"))
show write_file(to_string(42), to_string(true))
show length(read_file(input("path? ")))
show unwrap(try_write_file(unwrap(try_read_file("path.txt")), "x"))"#,
        // Loops, branches and nested scopes.
        r#"assign i as 0
while i < 3 {
    assign line as try_read_file("a.txt")
    if is_ok(line) { show write_file("b.txt", unwrap(line)) } else { show error_code(line) }
    reassign i as i + 1
}"#,
    ] {
        assert_eq!(check(source), Ok(()), "{source}");
    }
}

#[test]
fn wrong_argument_counts_name_the_function_and_the_call() {
    for (source, call, name, message) in [
        (
            "show read_file()",
            "read_file()",
            "read_file",
            "expected 1 argument",
        ),
        (
            r#"show read_file("a", "b")"#,
            r#"read_file("a", "b")"#,
            "read_file",
            "expected 1 argument",
        ),
        (
            "show try_read_file()",
            "try_read_file()",
            "try_read_file",
            "expected 1 argument",
        ),
        (
            r#"show try_read_file("a", "b")"#,
            r#"try_read_file("a", "b")"#,
            "try_read_file",
            "expected 1 argument",
        ),
        (
            r#"show write_file("a")"#,
            r#"write_file("a")"#,
            "write_file",
            "expected 2 arguments",
        ),
        (
            "show write_file()",
            "write_file()",
            "write_file",
            "expected 2 arguments",
        ),
        (
            r#"show write_file("a", "b", "c")"#,
            r#"write_file("a", "b", "c")"#,
            "write_file",
            "expected 2 arguments",
        ),
        (
            r#"show try_write_file("a")"#,
            r#"try_write_file("a")"#,
            "try_write_file",
            "expected 2 arguments",
        ),
        (
            "show try_write_file()",
            "try_write_file()",
            "try_write_file",
            "expected 2 arguments",
        ),
        (
            r#"show try_write_file("a", "b", "c")"#,
            r#"try_write_file("a", "b", "c")"#,
            "try_write_file",
            "expected 2 arguments",
        ),
    ] {
        assert_call_error(source, call, name, message);
    }
}

#[test]
fn only_strings_are_accepted_and_the_message_names_the_argument() {
    // Anything that is not exactly a string, including values that other
    // built-ins would convert or widen, in the path position.
    let not_strings = [
        "1",
        "1.5",
        "true",
        "[1, 2]",
        r#"ok("a.txt")"#,
        r#"try_read_file("a.txt")"#,
        "-1",
    ];
    for name in FILE_FUNCTIONS {
        let writes = name.ends_with("write_file");
        for value in not_strings {
            let call = if writes {
                format!(r#"{name}({value}, "text")"#)
            } else {
                format!("{name}({value})")
            };
            assert_call_error(
                &format!("show {call}"),
                &call,
                name,
                "the file path must be a string",
            );
        }
    }
    for name in ["write_file", "try_write_file"] {
        for value in not_strings {
            let call = format!(r#"{name}("a.txt", {value})"#);
            assert_call_error(
                &format!("assign x as {call}"),
                &call,
                name,
                "the text to write must be a string",
            );
        }
    }
    // The path is reported first when both arguments are wrong.
    assert_call_error(
        "show write_file(1, 2)",
        "write_file(1, 2)",
        "write_file",
        "the file path must be a string",
    );
}

#[test]
fn misusing_the_returned_values_is_still_a_type_error() {
    for source in [
        // A string is not a result, and a result is not a string.
        r#"show unwrap(read_file("a.txt"))"#,
        r#"show is_ok(write_file("a.txt", "x"))"#,
        r#"show error_code(read_file("a.txt"))"#,
        r#"show length(try_read_file("a.txt"))"#,
        r#"show try_read_file("a.txt") + "x""#,
        r#"show read_file("a.txt") + 1"#,
        r#"show write_file("a.txt", "x") + "text""#,
        // Results cannot be shown directly or used as conditions.
        r#"show try_read_file("a.txt")"#,
        r#"show try_write_file("a.txt", "x")"#,
        r#"if write_file("a.txt", "x") { show 1 }"#,
        r#"while try_read_file("a.txt") { show 1 }"#,
        // Reassignment cannot change a variable's type.
        r#"assign n as write_file("a.txt", "x")
reassign n as "text""#,
        r#"assign text as read_file("a.txt")
reassign text as write_file("b.txt", "x")"#,
        r#"assign r as try_write_file("a.txt", "x")
reassign r as try_read_file("a.txt")"#,
        r#"assign r as try_read_file("a.txt")
reassign r as read_file("a.txt")"#,
        // Wrong declared types for parameters and returns.
        r#"function f(r: result<string>) be string { return unwrap(r) }
show f(try_write_file("a.txt", "x"))"#,
        r#"function f() be string { return write_file("a.txt", "x") }"#,
        r#"function f() be result<string> { return read_file("a.txt") }"#,
        // The helpers are not callable by their runtime names.
        r#"show cheeku_read_file("a.txt")"#,
        r#"show cheeku_write_file("a.txt", "x")"#,
        // Unrelated names are still undefined.
        r#"show read_files("a.txt")"#,
        r#"show readfile("a.txt")"#,
        r#"show append_file("a.txt", "x")"#,
    ] {
        assert!(check(source).is_err(), "{source}");
    }
}

#[test]
fn file_builtin_names_are_reserved_exactly() {
    for name in FILE_FUNCTIONS {
        assert!(reserved_function(name), "{name}");
        let parameters = if name.ends_with("write_file") {
            "path: string, text: string"
        } else {
            "path: string"
        };
        let source = format!("function {name}({parameters}) be string {{ return path }}");
        match check(&source) {
            Err(TypeError::InvalidFunction {
                name: reported,
                message,
                ..
            }) => {
                assert_eq!(reported, name);
                assert_eq!(message, "duplicate or reserved function name");
            }
            other => panic!("{source}: expected a reserved-name error, got {other:?}"),
        }
    }
    // Only the exact names are taken; near misses and other cases stay available.
    for name in [
        "read",
        "reader",
        "read_files",
        "file_read",
        "try_read",
        "write_file2",
        "try_write_files",
        "Read_File",
        "WRITE_FILE",
    ] {
        assert!(!reserved_function(name), "{name}");
        let source = format!(
            "function {name}(path: string) be string {{ return path }}\nshow {name}(\"x\")"
        );
        assert_eq!(check(&source), Ok(()), "{source}");
    }
}

#[test]
fn existing_builtins_and_functions_keep_their_behaviour() {
    // The generic message is still used for every other built-in.
    for (source, call, name) in [
        ("show parse_int(1)", "parse_int(1)", "parse_int"),
        ("show try_input(1)", "try_input(1)", "try_input"),
        ("show unwrap(1)", "unwrap(1)", "unwrap"),
        ("show length(1)", "length(1)", "length"),
    ] {
        assert_call_error(source, call, name, "invalid argument count or types");
    }
    // User functions are checked as before.
    assert_call_error(
        "function f(a: string) be string { return a }\nshow f(1)",
        "f(1)",
        "f",
        "invalid argument count or types",
    );
    assert_call_error(
        "show missing()",
        "missing()",
        "missing",
        "undefined function",
    );
    // The previously reserved names are still reserved.
    for name in [
        "input",
        "try_input",
        "parse_int",
        "unwrap",
        "length",
        "cheeku_read_line",
    ] {
        assert!(reserved_function(name), "{name}");
    }
}

#[test]
fn rendered_diagnostics_point_at_the_call_and_explain_the_fix() {
    for (source, line, column, fragments) in [
        (
            "assign bytes as write_file(\"a.txt\", 42)",
            "assign bytes as write_file(\"a.txt\", 42)",
            "1:17",
            vec![
                "error: Function 'write_file': the text to write must be a string",
                "to_string(...)",
                "unwrap(...)",
            ],
        ),
        (
            "show read_file()",
            "show read_file()",
            "1:6",
            vec!["error: Function 'read_file': expected 1 argument: the file path"],
        ),
        (
            "function try_read_file(p: string) be string { return p }",
            "function try_read_file(p: string) be string { return p }",
            "1:1",
            vec!["Function 'try_read_file': duplicate or reserved function name"],
        ),
    ] {
        let error = compile_source(source, Path::new("unused-diagnostic-output")).unwrap_err();
        let diagnostic = render(&error, Path::new("files.chuchu"), source);
        assert!(
            diagnostic.contains(&format!("files.chuchu:{column}")),
            "{diagnostic}"
        );
        assert!(diagnostic.contains(&format!("1 | {line}")), "{diagnostic}");
        for fragment in fragments {
            assert!(diagnostic.contains(fragment), "{fragment}\n{diagnostic}");
        }
    }
}
