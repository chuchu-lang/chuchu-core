use chuchu::compiler::{CompileError, compile_source};
use chuchu::diagnostic::render;
use chuchu::lexer::tokenize;
use chuchu::parser::{ParseError, Parser};
use chuchu::token::Span;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn parse_error(source: &str) -> CompileError {
    CompileError::Parse(
        Parser::new(tokenize(source).unwrap())
            .parse_program()
            .unwrap_err(),
    )
}

#[test]
fn missing_operand_names_the_operator_and_points_to_the_source_line() {
    let source = "assign counter as 0\nwhile counter < 5 {\n    show counter\n    reassign counter as counter + \n";
    let diagnostic = render(&parse_error(source), Path::new("loops.chuchu"), source);
    assert!(
        diagnostic.starts_with("error: Expected expression after '+'\n"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("loops.chuchu:4:35"), "{diagnostic}");
    assert!(diagnostic.contains("4 |     reassign counter as counter + \n"));
    assert!(diagnostic.contains("^ missing operand"));
    assert!(diagnostic.contains("help: Add a number, variable, or expression after '+'."));
    assert!(!diagnostic.contains("85..86"));
}

#[test]
fn eof_delimiters_include_the_opening_line() {
    for (source, delimiter, opening) in [
        (
            "while true {\n    show 1\n",
            "'}'",
            "block opened on line 1",
        ),
        ("show (1 + 2", "')'", "parentheses opened on line 1"),
        ("show [1, 2", "']'", "array or index opened on line 1"),
    ] {
        let diagnostic = render(&parse_error(source), Path::new("case.chuchu"), source);
        assert!(
            diagnostic.contains(&format!("Expected {delimiter}, found end of file")),
            "{diagnostic}"
        );
        assert!(diagnostic.contains(opening), "{diagnostic}");
        assert!(diagnostic.contains('^'));
    }
}

#[test]
fn tabs_crlf_and_non_ascii_errors_keep_the_caret_aligned() {
    let source = "show 1\r\n\tshow 2 +\r\n";
    let diagnostic = render(&parse_error(source), Path::new("case.chuchu"), source);
    assert!(diagnostic.contains("case.chuchu:2:13"), "{diagnostic}");
    assert!(diagnostic.contains("2 |     show 2 +\n"), "{diagnostic}");
    assert!(!diagnostic.contains('\r'));
    assert!(!diagnostic.contains('\t'));
    let source = "show é";
    let error = CompileError::Lex(tokenize(source).unwrap_err());
    let diagnostic = render(&error, Path::new("case.chuchu"), source);
    assert!(diagnostic.contains("case.chuchu:1:6"));
    assert!(diagnostic.contains("1 | show é"));
    assert!(diagnostic.contains("     ^ this character"));
}

#[test]
fn empty_source_and_invalid_spans_do_not_panic() {
    let error = CompileError::Parse(ParseError::UnexpectedEnd);
    let diagnostic = render(&error, Path::new("empty.chuchu"), "");
    assert!(diagnostic.contains("empty.chuchu:1:1"));
    assert!(diagnostic.contains("^ the program is incomplete"));
    for start in [1, 3, usize::MAX] {
        let error = CompileError::Parse(ParseError::ExpectedExpression {
            span: Span { start, end: start },
        });
        let diagnostic = render(&error, Path::new("case.chuchu"), "é");
        assert!(!diagnostic.is_empty());
    }
}

#[test]
fn type_errors_include_the_source_and_actionable_guidance() {
    for (source, message, help) in [
        (
            "show missing",
            "Undefined variable 'missing'",
            "assign missing as <value>",
        ),
        (
            "assign x as 1\nassign x as 2",
            "Duplicate declaration of 'x'",
            "reassign x as <value>",
        ),
        (
            "while 1 { show 1 }",
            "Condition must be bool",
            "counter < 5",
        ),
    ] {
        let error = compile_source(source, Path::new("unused-diagnostic-output")).unwrap_err();
        let diagnostic = render(&error, Path::new("case.chuchu"), source);
        assert!(diagnostic.contains(message), "{diagnostic}");
        assert!(diagnostic.contains(help), "{diagnostic}");
        assert!(diagnostic.contains('^'));
    }
}

#[test]
fn calling_len_suggests_the_renamed_length_builtin() {
    let source = "assign numbers as [1, 2, 3]\nshow len(numbers)";
    let error = compile_source(source, Path::new("unused-diagnostic-output")).unwrap_err();
    let diagnostic = render(&error, Path::new("case.chuchu"), source);
    assert!(
        diagnostic.contains("Function 'len': undefined function"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("help: `len` was renamed to `length`. Did you mean `length`?"),
        "{diagnostic}"
    );
    let source = "show missing(1)";
    let error = compile_source(source, Path::new("unused-diagnostic-output")).unwrap_err();
    let diagnostic = render(&error, Path::new("case.chuchu"), source);
    assert!(!diagnostic.contains("length"), "{diagnostic}");
}

#[test]
fn control_characters_are_printed_without_terminal_escape_sequences() {
    let source = "show \u{1b}";
    let error = CompileError::Lex(tokenize(source).unwrap_err());
    let diagnostic = render(&error, Path::new("case.chuchu"), source);
    assert!(!diagnostic.contains('\u{1b}'));
    assert!(diagnostic.contains("show \\u{1b}"), "{diagnostic}");
}

#[test]
fn cli_uses_source_diagnostics_and_preserves_the_existing_output() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "cheeku diagnostic {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let input = directory.join("broken source.chuchu");
    let output = directory.join("previous-program");
    fs::write(&input, "show 1 +\n").unwrap();
    fs::write(&output, "preserve this executable").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_cheeku"))
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    let diagnostic = String::from_utf8(result.stderr).unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(
        diagnostic.contains("cheeku: error: Expected expression after '+'"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("broken source.chuchu:1:9"));
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        "preserve this executable"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn showing_a_result_explains_how_to_print_its_payload() {
    let source = "assign x as try_input()\nshow x";
    let error = compile_source(source, Path::new("unused-diagnostic-output")).unwrap_err();
    let diagnostic = render(&error, Path::new("input.chuchu"), source);
    assert!(diagnostic.contains("Cannot print a result directly"));
    assert!(diagnostic.contains("is_ok(value)"));
    assert!(diagnostic.contains("show unwrap(value)"));
    assert!(diagnostic.contains("show error_message(value)"));
}
