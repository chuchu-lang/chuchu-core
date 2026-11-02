mod support;
use chuchu::compiler::compile_file_with_options;
use chuchu::lexer::tokenize;
use chuchu::optimization::{BuildOptions, DebugInfo, Optimization};
use chuchu::parser::Parser;
use chuchu::symbol_table::SymbolTable;
use chuchu::type_checker::TypeChecker;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Case {
    path: PathBuf,
}
impl Case {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cheeku input {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }
    fn compile(&self, source: &str, optimization: Optimization, debug_info: DebugInfo) -> PathBuf {
        let input = self.path.join("input with spaces.chuchu");
        fs::write(&input, source).unwrap();
        let output = self.path.join(support::executable_name("program"));
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
        assert!(fs::read_dir(&self.path).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".cheeku-")
        }));
        output
    }
}
impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run(program: &Path, input: &[u8]) -> Output {
    // Use a real input file so large pipe writes cannot block against stdout.
    let input_path = program.with_extension("stdin");
    fs::write(&input_path, input).unwrap();
    Command::new(program)
        .stdin(fs::File::open(input_path).unwrap())
        .output()
        .unwrap()
}
fn succeeds(output: &Output, expected: &str) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, support::native_output(expected));
    assert!(output.stderr.is_empty());
}

#[test]
fn result_types_and_constructors_are_checked_before_lowering() {
    for source in [
        "function f() be result<int> { return parse_int(\"1\") }\nshow unwrap(f())",
        "function f(x: result<string>) be result<string> { return x }\nshow unwrap(f(ok(\"hi\")))",
        "show error_code(err<bool>(8))\nshow unwrap(ok(true))",
    ] {
        let statements = Parser::new(tokenize(source).unwrap())
            .parse_program()
            .unwrap();
        TypeChecker::new(SymbolTable::new())
            .check_program(&statements)
            .unwrap();
    }
    for source in [
        "show parse_int(1)",
        "function input() be string { return \"reserved\" }",
        "assign value as input(true)",
        "assign value as input(\"one\", \"two\")",
        "show is_ok(input())",
        "show unwrap(input())",
        "assign line as read_line()",
        "function try_input() be string { return \"reserved\" }",
        "assign x as try_input(true)",
        "assign x as try_input(\"one\", \"two\")",
        "function cheeku_read_line_prompt() be int { return 1 }",
        "show is_ok(1)",
        "show try_input(1)",
        "show unwrap(1)",
        "show to_string(ok(1))",
        "show ok([1, 2])",
        "show ok(ok(1))",
        "show parse_int(\"1\")",
        "function f() be result<float> { return ok(1) }",
        "assign x as ok(1)\nreassign x as ok(1.0)",
        "show err<int>(true)",
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
    assert!(
        Parser::new(tokenize("function f() be result<result<int>> { return ok(1) }").unwrap())
            .parse_program()
            .is_err()
    );
}

#[test]
fn numeric_parsing_is_strict_and_reports_distinct_ranges() {
    let case = Case::new();
    let source = r#"show unwrap(parse_int(" -9223372036854775808 "))
show unwrap(parse_int("+9223372036854775807"))
show unwrap(parse_int(" 0012\t"))
show error_code(parse_int("9223372036854775808"))
show error_code(parse_int("-9223372036854775809"))
show error_code(parse_int("999999999999999999999999bad"))
show error_code(parse_int("1.0"))
show error_code(parse_int("0x10"))
show error_code(parse_int(""))
show error_code(parse_int("1 2"))
show error_code(parse_int("１２"))
show error_code(parse_int(" 1"))
show unwrap(parse_float(" +.5e2\t"))
show unwrap(parse_float("5."))
show unwrap(parse_float("42"))
show error_code(parse_float("1e9999"))
show error_code(parse_float("1e-9999"))
show error_code(parse_float("NaN"))
show error_code(parse_float("inf"))
show error_code(parse_float("0x1p2"))
show error_code(parse_float("1,25"))
show error_code(parse_float("1e"))
show error_message(parse_int("bad"))
show error_message(parse_float("1e9999"))
show error_message(ok(1))
"#;
    for level in [
        Optimization::O0,
        Optimization::O1,
        Optimization::O2,
        Optimization::O3,
    ] {
        let output = case.compile(source, level, DebugInfo::Full);
        succeeds(
            &run(&output, b""),
            "-9223372036854775808\n9223372036854775807\n12\n3\n3\n2\n2\n2\n2\n2\n2\n2\n50\n5\n42\n3\n3\n2\n2\n2\n2\n2\nInvalid number\nValue out of range\nNo error\n",
        );
    }
}

#[test]
fn input_distinguishes_empty_lines_eof_and_partial_final_lines() {
    let case = Case::new();
    let source = r#"assign first as try_input()
show is_ok(first)
show length(unwrap(first))
show unwrap(try_input())
show unwrap(try_input())
show is_eof(try_input())
show error_code(try_input())
show error_message(try_input())
"#;
    for level in [
        Optimization::O0,
        Optimization::O1,
        Optimization::O2,
        Optimization::O3,
    ] {
        let output = case.compile(source, level, DebugInfo::Full);
        succeeds(
            &run(&output, "\r\nدنیا 🌍\r\nlast".as_bytes()),
            "true\n0\nدنیا 🌍\nlast\ntrue\n1\nEnd of input\n",
        );
    }
    let output = case.compile(
        "show is_eof(try_input())",
        Optimization::O2,
        DebugInfo::None,
    );
    succeeds(&run(&output, b""), "true\n");
}

#[test]
fn rejected_input_is_drained_and_the_next_line_remains_available() {
    let case = Case::new();
    let output = case.compile(
        "show error_code(try_input())\nshow unwrap(try_input())\nshow is_eof(try_input())",
        Optimization::O2,
        DebugInfo::Full,
    );
    for bytes in [
        b"\xc0\xaf".as_slice(),
        b"\xed\xa0\x80",
        b"\xf4\x90\x80\x80",
        b"\xe2\x82",
        b"\x80",
    ] {
        let mut input = bytes.to_vec();
        input.extend_from_slice(b"\nvalid\n");
        succeeds(&run(&output, &input), "5\nvalid\ntrue\n");
    }
    succeeds(&run(&output, b"bad\0line\nvalid\n"), "6\nvalid\ntrue\n");
    let output = case.compile(
        "show length(unwrap(try_input()))\nshow is_eof(try_input())",
        Optimization::O2,
        DebugInfo::None,
    );
    succeeds(
        &run(&output, "🌍".repeat(10000).as_bytes()),
        "10000\ntrue\n",
    );
}

#[test]
fn results_keep_string_payloads_alive_across_copies_functions_and_returns() {
    let case = Case::new();
    let source = r#"function echo(value: result<string>) be result<string> {
    assign local as value
    if is_ok(local) { return local }
    return err<string>(8)
}
assign original as try_input()
assign saved as original
reassign original as ok("replacement")
show unwrap(saved)
show unwrap(original)
show unwrap(echo(saved))
show error_code(echo(err<string>(4)))
assign i as 0
while i < 1000 {
    assign copy as echo(saved)
    assign error as err<string>(8)
    reassign error as copy
    reassign copy as err<string>(9)
    reassign i as i + 1
}
show unwrap(ok(true))
show error_code(err<bool>(2147483647))
show error_message(err<int>(8))
"#;
    for level in [Optimization::O0, Optimization::O3] {
        let output = case.compile(source, level, DebugInfo::Full);
        succeeds(
            &run(&output, b"original\n"),
            "original\nreplacement\noriginal\n8\ntrue\n2147483647\nUnknown error\n",
        );
    }
}

#[test]
fn conversions_are_explicit_and_float_formatting_round_trips() {
    let case = Case::new();
    let source = r#"show "count: " + to_string(-9223372036854775808)
show to_string(9223372036854775807)
show to_string(true)
show to_string(false)
show to_string("unchanged")
show to_string(-0.0)
show unwrap(parse_float(to_string(0.1))) == 0.1
show unwrap(parse_float(to_string(1.2345678901234567))) == 1.2345678901234567
show to_string(3.5)
"#;
    for level in [Optimization::O0, Optimization::O3] {
        let output = case.compile(source, level, DebugInfo::Full);
        succeeds(
            &run(&output, b""),
            "count: -9223372036854775808\n9223372036854775807\ntrue\nfalse\nunchanged\n-0\ntrue\ntrue\n3.5\n",
        );
    }
}

#[test]
fn explicit_unwrap_and_invalid_error_codes_have_checked_failures() {
    let case = Case::new();
    for source in [
        "show unwrap(parse_int(\"bad\"))",
        "show error_code(err<int>(0))",
        "show error_code(err<string>(-1))",
        "show error_code(err<float>(2147483648))",
    ] {
        let output = case.compile(source, Optimization::O2, DebugInfo::None);
        let result = run(&output, b"");
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains("cheeku runtime:"));
    }
}

#[test]
fn io_failure_is_recoverable_and_distinct_from_eof() {
    let case = Case::new();
    let output = case.compile("assign line as try_input()\nshow error_code(line)\nshow is_eof(line)\nshow error_message(line)", Optimization::O2, DebugInfo::Full);
    let write_only = fs::File::create(case.path.join("write-only-input")).unwrap();
    let result = Command::new(output)
        .stdin(Stdio::from(write_only))
        .output()
        .unwrap();
    succeeds(&result, "4\nfalse\nInput/output error\n");
}

#[test]
fn interactive_converter_retries_bad_input_and_exits_cleanly() {
    let case = Case::new();
    let source = include_str!("../examples/temperature.chuchu");
    let output = case.compile(source, Optimization::O2, DebugInfo::Full);
    succeeds(
        &run(&output, b"bad\n-300\n10\nquit\n"),
        "Celsius to Fahrenheit. Enter a number or quit.\nCelsius:\nTry again: Invalid number\nCelsius:\nTry again: temperature is below absolute zero\nCelsius:\n50 F\nCelsius:\nDone.\n",
    );
    succeeds(
        &run(&output, b""),
        "Celsius to Fahrenheit. Enter a number or quit.\nCelsius:\nDone.\n",
    );
}

#[test]
fn prompts_are_flushed_before_waiting_for_input() {
    let case = Case::new();
    let output = case.compile(
        "show \"Prompt\"\nshow unwrap(try_input())",
        Optimization::O2,
        DebugInfo::None,
    );
    let mut child = Command::new(output)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader_thread = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(stdout);
        let mut prompt = String::new();
        let _ = reader.read_line(&mut prompt);
        let _ = sender.send(prompt);
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    });
    let prompt = receiver.recv_timeout(std::time::Duration::from_secs(30));
    if prompt.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert_eq!(prompt.unwrap().trim_end(), "Prompt");
    child.stdin.take().unwrap().write_all(b"answer\n").unwrap();
    assert!(child.wait().unwrap().success());
    reader_thread.join().unwrap();
}

#[test]
fn optional_prompts_preserve_text_and_result_handling() {
    let case = Case::new();
    let source = r#"assign prompt as "x = "
assign x as try_input(prompt)
if is_ok(x) { show unwrap(x) } else { show error_message(x) }
assign y as try_input("دنیا 🌍 100%: " + "")
if is_ok(y) { show unwrap(y) } else { show error_message(y) }
show is_eof(try_input(""))
"#;
    for level in [
        Optimization::O0,
        Optimization::O1,
        Optimization::O2,
        Optimization::O3,
    ] {
        let program = case.compile(source, level, DebugInfo::Full);
        succeeds(&run(&program, b"42\n\n"), "x = 42\nدنیا 🌍 100%: \ntrue\n");
        succeeds(
            &run(&program, b""),
            "x = End of input\nدنیا 🌍 100%: End of input\ntrue\n",
        );
    }
}

#[test]
fn same_line_prompt_is_flushed_before_input_is_sent() {
    for source in ["show input(\"x = \")", "show unwrap(try_input(\"x = \"))"] {
        let case = Case::new();
        let program = case.compile(source, Optimization::O2, DebugInfo::None);
        let mut child = Command::new(program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            use std::io::Read;
            let mut prompt = [0; 4];
            let result = stdout.read_exact(&mut prompt).map(|()| prompt);
            let _ = sender.send(result);
            let mut remaining = String::new();
            stdout.read_to_string(&mut remaining).unwrap();
            remaining
        });
        let prompt = receiver.recv_timeout(std::time::Duration::from_secs(30));
        if prompt.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert_eq!(prompt.unwrap().unwrap(), *b"x = ");
        child.stdin.take().unwrap().write_all(b"answer\n").unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            reader.join().unwrap(),
            String::from_utf8(support::native_output("answer\n")).unwrap()
        );
    }
}

#[test]
fn input_returns_owned_strings_without_result_wrappers() {
    let case = Case::new();
    let source = r#"function read_name() be string { return input("name: ") }
assign first as read_name()
assign saved as first
reassign first as input()
show saved
show first
show length(input(""))
"#;
    for level in [
        Optimization::O0,
        Optimization::O1,
        Optimization::O2,
        Optimization::O3,
    ] {
        let program = case.compile(source, level, DebugInfo::Full);
        succeeds(
            &run(&program, "دنیا 🌍\nnew\n\n".as_bytes()),
            "name: دنیا 🌍\nnew\n0\n",
        );
    }
}

#[test]
fn input_reports_runtime_errors_instead_of_exposing_results() {
    let case = Case::new();
    for level in [Optimization::O0, Optimization::O3] {
        let program = case.compile(
            "assign x as input(\"x = \")\nshow x",
            level,
            DebugInfo::Full,
        );
        for (bytes, message) in [
            (b"".as_slice(), "input reached end of input"),
            (b"\xff\n".as_slice(), "input failed"),
        ] {
            let result = run(&program, bytes);
            assert_eq!(result.status.code(), Some(1));
            assert_eq!(result.stdout, b"x = ");
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(error.contains(message), "{error}");
            assert!(error.contains("try_input()"), "{error}");
        }
        let write_only = fs::File::create(case.path.join("invalid-input-handle")).unwrap();
        let result = Command::new(&program)
            .stdin(Stdio::from(write_only))
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains("input failed"));
    }
}
