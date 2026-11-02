//! End-to-end tests for `read_file`, `try_read_file`, `write_file` and
//! `try_write_file` (Issue #9, stage 4). Every program is compiled to a native
//! executable at O0-O3 and run in its own temporary directory, so relative paths,
//! the files left behind, the exit status and both output streams are observed.
mod support;
use chuchu::compiler::compile_file_with_options;
use chuchu::optimization::{BuildOptions, DebugInfo, Optimization};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

const LEVELS: [Optimization; 4] = [
    Optimization::O0,
    Optimization::O1,
    Optimization::O2,
    Optimization::O3,
];

/// A fresh working directory that also holds the compiled program.
struct Case {
    path: PathBuf,
}

impl Case {
    fn new() -> Self {
        // The spaces also prove paths that need quoting reach the program intact.
        let path = std::env::temp_dir().join(format!(
            "cheeku file io {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn compile(&self, source: &str, optimization: Optimization, debug_info: DebugInfo) -> PathBuf {
        let input = self.path.join("program.chuchu");
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
        .unwrap_or_else(|error| panic!("{optimization:?}: {error}\n{source}"));
        output
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct Run {
    label: String,
    output: Output,
    case: Case,
}

impl Run {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).into_owned()
    }
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }
    fn file(&self, name: &str) -> Vec<u8> {
        fs::read(self.case.path.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
    }
    fn succeeded_with(&self, expected: &str) {
        assert!(
            self.output.status.success(),
            "{}: {}\nstdout: {}",
            self.label,
            self.stderr(),
            self.stdout()
        );
        assert_eq!(
            self.output.stdout,
            support::native_output(expected),
            "{}",
            self.label
        );
        assert!(
            self.output.stderr.is_empty(),
            "{}: {}",
            self.label,
            self.stderr()
        );
    }
}

/// Compiles and runs a program once per configuration. `source` receives the
/// working directory (for tests that need an absolute path) and `prepare`
/// populates it before the program starts.
fn run_matrix(
    configurations: &[(Optimization, DebugInfo)],
    source: impl Fn(&Path) -> String,
    prepare: impl Fn(&Path),
) -> Vec<Run> {
    configurations
        .iter()
        .map(|&(optimization, debug_info)| {
            let case = Case::new();
            prepare(&case.path);
            let program = case.compile(&source(&case.path), optimization, debug_info);
            let output = Command::new(&program)
                .current_dir(&case.path)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            Run {
                label: format!("{optimization:?}/{debug_info:?}"),
                output,
                case,
            }
        })
        .collect()
}

fn levels() -> Vec<(Optimization, DebugInfo)> {
    LEVELS
        .iter()
        .map(|&level| (level, DebugInfo::None))
        .collect()
}

/// The four levels plus debug metadata at both ends of the range.
fn levels_with_debug_info() -> Vec<(Optimization, DebugInfo)> {
    let mut all = levels();
    all.push((Optimization::O0, DebugInfo::Full));
    all.push((Optimization::O3, DebugInfo::Full));
    all
}

fn fixed(source: &'static str) -> impl Fn(&Path) -> String {
    move |_| source.to_owned()
}

/// Escapes a path for a Chuchu string literal. Forward slashes keep Windows paths
/// free of backslash escapes, and Windows accepts them.
fn literal(path: &Path) -> String {
    path.to_str()
        .unwrap()
        .replace('\\', "/")
        .replace('"', "\\\"")
}

fn nothing(_: &Path) {}

#[test]
fn text_round_trips_through_write_and_read() {
    let source = "assign written as write_file(\"notes.txt\", \"h\u{e9}llo \u{1f30d}\\n\")\n\
                  show written\n\
                  assign text as read_file(\"notes.txt\")\n\
                  show length(text)\n\
                  show text == \"h\u{e9}llo \u{1f30d}\\n\"\n\
                  show text";
    for run in run_matrix(&levels_with_debug_info(), |_| source.to_owned(), nothing) {
        // 12 bytes, but 8 characters: write_file counts bytes, length counts scalars.
        run.succeeded_with("12\n8\ntrue\nh\u{e9}llo \u{1f30d}\n\n");
        assert_eq!(run.file("notes.txt"), "h\u{e9}llo \u{1f30d}\n".as_bytes());
    }
}

#[test]
fn writing_creates_overwrites_and_truncates() {
    let source = r#"assign path as "f.txt"
show write_file(path, "a much longer first version")
show write_file(path, "short")
show length(read_file(path))
show write_file(path, "")
show length(read_file(path))
show read_file(path) == ""
show write_file("other.txt", "new")"#;
    for run in run_matrix(&levels(), fixed(source), nothing) {
        run.succeeded_with("27\n5\n5\n0\n0\ntrue\n3\n");
        assert_eq!(run.file("f.txt"), b"");
        assert_eq!(run.file("other.txt"), b"new");
    }
}

#[test]
fn newlines_and_byte_order_marks_are_preserved() {
    let source = r#"assign text as read_file("crlf.txt")
show length(text)
show text == "one\r\ntwo\rthree\nfour"
show length(read_file("bom.txt"))
show write_file("copy.txt", read_file("bom.txt"))
show write_file("crlf-copy.txt", text)"#;
    let prepare = |directory: &Path| {
        fs::write(directory.join("crlf.txt"), b"one\r\ntwo\rthree\nfour").unwrap();
        fs::write(directory.join("bom.txt"), b"\xEF\xBB\xBFx").unwrap();
    };
    for run in run_matrix(&levels(), fixed(source), prepare) {
        run.succeeded_with("19\ntrue\n2\n4\n19\n");
        // Bytes come back exactly: the byte-order mark is neither dropped nor added.
        assert_eq!(run.file("copy.txt"), b"\xEF\xBB\xBFx");
        assert_eq!(run.file("crlf-copy.txt"), b"one\r\ntwo\rthree\nfour");
    }
}

#[test]
fn try_variants_return_recoverable_errors_and_keep_running() {
    let source = r#"assign missing as try_read_file("missing.txt")
show is_ok(missing)
show error_code(missing)
show error_message(missing)
show error_code(try_read_file("a directory"))
assign utf8 as try_read_file("bad-utf8.bin")
show error_code(utf8)
show error_message(utf8)
assign nul as try_read_file("nul.bin")
show error_code(nul)
show error_message(nul)
assign empty as try_read_file("empty.txt")
show is_ok(empty)
show length(unwrap(empty))
show is_eof(empty)
assign nowhere as try_write_file("no such dir/out.txt", "x")
show error_code(nowhere)
show is_eof(nowhere)
show error_code(try_write_file("a directory", "x"))
assign fine as try_write_file("out.txt", "abc")
show is_ok(fine)
show unwrap(fine)
show unwrap(try_read_file("out.txt"))
show "still running""#;
    let prepare = |directory: &Path| {
        fs::create_dir(directory.join("a directory")).unwrap();
        fs::write(directory.join("bad-utf8.bin"), b"ok\xFF").unwrap();
        fs::write(directory.join("nul.bin"), b"a\0b").unwrap();
        fs::write(directory.join("empty.txt"), b"").unwrap();
    };
    let expected = "false\n4\nInput/output error\n4\n5\nInput is not valid UTF-8\n6\n\
                    Input contains a NUL byte\ntrue\n0\nfalse\n4\nfalse\n4\ntrue\n3\nabc\n\
                    still running\n";
    for run in run_matrix(&levels(), fixed(source), prepare) {
        run.succeeded_with(expected);
        assert!(!run.case.path.join("no such dir").exists());
    }
}

#[test]
fn plain_variants_stop_with_exit_code_1_and_a_source_span() {
    // (call, text the diagnostic must contain)
    let cases: [(&str, &str); 7] = [
        (
            r#"read_file("missing.txt")"#,
            "read_file failed: the file could not be opened or read",
        ),
        (
            r#"read_file("a directory")"#,
            "read_file failed: the file could not be opened or read",
        ),
        (
            r#"read_file("bad-utf8.bin")"#,
            "read_file failed: the file is not valid UTF-8",
        ),
        (
            r#"read_file("nul.bin")"#,
            "read_file failed: the file contains a NUL byte",
        ),
        (
            r#"write_file("no such dir/out.txt", "x")"#,
            "write_file failed: the file could not be created, written or closed",
        ),
        (
            r#"write_file("a directory", "x")"#,
            "write_file failed: the file could not be created, written or closed",
        ),
        (
            r#"write_file("", "x")"#,
            "write_file failed: the file could not be created, written or closed",
        ),
    ];
    for (call, fragment) in cases {
        let source = format!("show \"before\"\nassign result as {call}\nshow \"after\"");
        let prepare = |directory: &Path| {
            fs::create_dir(directory.join("a directory")).unwrap();
            fs::write(directory.join("bad-utf8.bin"), b"ok\xFF").unwrap();
            fs::write(directory.join("nul.bin"), b"a\0b").unwrap();
        };
        let start = source.find(call).unwrap();
        let span = format!("at {start}..{}", start + call.len());
        for run in run_matrix(&levels(), |_| source.clone(), prepare) {
            assert_eq!(run.output.status.code(), Some(1), "{}: {call}", run.label);
            // Output produced before the failure survives; nothing after it runs.
            assert_eq!(
                run.output.stdout,
                support::native_output("before\n"),
                "{}: {call}",
                run.label
            );
            let stderr = run.stderr();
            assert!(
                stderr.starts_with("cheeku runtime: "),
                "{}: {stderr}",
                run.label
            );
            assert!(stderr.contains(fragment), "{}: {call}: {stderr}", run.label);
            assert!(stderr.contains("use try_"), "{}: {stderr}", run.label);
            assert!(stderr.contains(&span), "{}: {call}: {stderr}", run.label);
            assert!(!run.case.path.join("no such dir").exists());
        }
    }
}

#[test]
fn arguments_are_evaluated_left_to_right_before_the_file_is_touched() {
    let source = r#"show write_file("same.txt", read_file("same.txt"))
show unwrap(try_write_file(to_string(write_file("order.txt", "one")) + ".txt", read_file("order.txt")))"#;
    let prepare = |directory: &Path| {
        fs::write(directory.join("same.txt"), "keep me \u{e9}\n").unwrap();
    };
    for run in run_matrix(&levels(), fixed(source), prepare) {
        // The file is read completely before it is truncated and rewritten.
        run.succeeded_with("11\n3\n");
        assert_eq!(run.file("same.txt"), "keep me \u{e9}\n".as_bytes());
        // The left argument (a write that returns 3) runs before the right one
        // reads the file it wrote; the reverse order would fail to open it.
        assert_eq!(run.file("3.txt"), b"one");
    }
}

#[test]
fn file_functions_compose_with_variables_functions_and_results() {
    let source = r#"function copy_file(from: string, to: string) be result<int> {
    assign source as try_read_file(from)
    if is_ok(source) {
        return try_write_file(to, unwrap(source))
    }
    return err<int>(error_code(source))
}
function load(path: string) be string { return read_file(path) }
function save(path: string, text: string) be int { return write_file(path, text) }
function relay(r: result<string>) be result<string> { return r }
assign copied as copy_file("source.txt", "copy.txt")
show unwrap(copied)
show load("copy.txt") == load("source.txt")
assign missing as copy_file("missing.txt", "never.txt")
show error_code(missing)
assign i as 0
while i < 3 {
    show save("part-" + to_string(i) + ".txt", "line " + to_string(i) + "\n")
    reassign i as i + 1
}
assign joined as ""
reassign i as 0
while i < 3 {
    reassign joined as joined + load("part-" + to_string(i) + ".txt")
    reassign i as i + 1
}
show length(joined)
assign again as relay(try_read_file("part-2.txt"))
reassign again as relay(try_read_file("part-0.txt"))
show unwrap(again)
assign twice as read_file("copy.txt")
reassign twice as twice + read_file("copy.txt")
show length(twice)"#;
    let prepare = |directory: &Path| {
        fs::write(directory.join("source.txt"), "hello \u{1f30d}\n").unwrap();
    };
    // 11 bytes copied; a missing source gives code 4; three 7-byte parts make 21
    // characters; the reassigned result holds "line 0\n"; two 8-character copies are 16.
    let expected = "11\ntrue\n4\n7\n7\n7\n21\nline 0\n\n16\n";
    for run in run_matrix(&levels(), fixed(source), prepare) {
        run.succeeded_with(expected);
        assert_eq!(run.file("copy.txt"), "hello \u{1f30d}\n".as_bytes());
        assert_eq!(run.file("part-1.txt"), b"line 1\n");
        assert!(!run.case.path.join("never.txt").exists());
    }
}

/// Written with escapes so the program and the assertions use identical bytes
/// (the accented letters are precomposed).
const UNICODE_NAME: &str = "donn\u{e9}es-日本語 \u{1f30d}.txt";
const NESTED_NAME: &str = "sub dir/\u{f1}and\u{fa} 中文.txt";

#[test]
fn unicode_relative_and_absolute_paths_work() {
    let source = |directory: &Path| {
        format!(
            r#"assign unicode as "{UNICODE_NAME}"
assign nested as "{NESTED_NAME}"
assign absolute as "{}/absolute path.txt"
show write_file(unicode, "Привет")
show write_file(nested, "relative")
show write_file(absolute, "absolute")
show read_file(unicode)
show read_file(nested) + " " + read_file(absolute)
show read_file("./" + nested) == read_file(nested)
show is_ok(try_read_file("sub dir/missing.txt"))"#,
            literal(directory)
        )
    };
    let prepare = |directory: &Path| fs::create_dir(directory.join("sub dir")).unwrap();
    for run in run_matrix(&levels(), source, prepare) {
        run.succeeded_with("12\n8\n8\nПривет\nrelative absolute\ntrue\nfalse\n");
        assert_eq!(run.file(UNICODE_NAME), "Привет".as_bytes());
        assert_eq!(run.file(NESTED_NAME), b"relative");
        assert_eq!(run.file("absolute path.txt"), b"absolute");
    }
}

#[test]
fn large_files_round_trip() {
    // 16 bytes doubled sixteen times is exactly 1 MiB.
    let source = r#"assign data as "0123456789abcdef"
assign i as 0
while i < 16 {
    reassign data as data + data
    reassign i as i + 1
}
show length(data)
show write_file("big.txt", data)
assign back as read_file("big.txt")
show length(back)
show back == data"#;
    for run in run_matrix(&levels(), fixed(source), nothing) {
        run.succeeded_with("1048576\n1048576\n1048576\ntrue\n");
        assert_eq!(run.file("big.txt").len(), 1_048_576);
    }
}

#[test]
fn repeated_use_does_not_exhaust_handles_or_change_results() {
    let source = r#"assign i as 0
assign good as 0
assign bad as 0
assign failed as 0
while i < 2000 {
    assign wrote as write_file("loop.txt", "round " + to_string(i))
    assign text as read_file("loop.txt")
    if length(text) == wrote { reassign good as good + 1 }
    if is_ok(try_read_file("bad-utf8.bin")) { reassign bad as bad + 1 }
    if is_ok(try_read_file("a directory")) { reassign bad as bad + 1 }
    if is_ok(try_write_file("a directory", "x")) { reassign bad as bad + 1 }
    reassign i as i + 1
}
show good
show bad
show read_file("loop.txt")"#;
    let prepare = |directory: &Path| {
        fs::create_dir(directory.join("a directory")).unwrap();
        fs::write(directory.join("bad-utf8.bin"), b"\xFF").unwrap();
    };
    for run in run_matrix(&levels(), fixed(source), prepare) {
        run.succeeded_with("2000\n0\nround 1999\n");
    }
}
