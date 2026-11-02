//! Tests the native file helpers directly. The language cannot call them yet, so
//! each test compiles `chuchu::runtime::c_source()` with a small C driver using
//! the host Clang (honouring `CHEEKU_LINKER`, like the compiler) and runs it
//! against real files. Set `CHEEKU_TEST_CFLAGS` (for example
//! `-fsanitize=address,undefined`) to add compiler options on non-Windows hosts.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// Command-line paths go through files so Windows' ANSI argv cannot alter them.
const DRIVER: &str = r##"
#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef __linux__
#include <dirent.h>
#endif
#ifndef _WIN32
#include <sys/resource.h>
#endif

int32_t cheeku_read_file(const char *path, char **out);
int32_t cheeku_write_file(const char *path, const char *content, int64_t *out);
#ifdef CHEEKU_TEST_ALLOC
void cheeku_test_free(void *value);
#define PAYLOAD_FREE cheeku_test_free
#else
#define PAYLOAD_FREE free
#endif

static char *slurp(const char *name) {
    FILE *file = fopen(name, "rb");
    if (!file) { perror(name); exit(90); }
    size_t capacity = 1024, length = 0;
    char *buffer = (char *)malloc(capacity);
    for (;;) {
        if (capacity - length < 2) { capacity *= 2; buffer = (char *)realloc(buffer, capacity); }
        size_t got = fread(buffer + length, 1, capacity - length - 1, file);
        length += got;
        if (got == 0) break;
    }
    fclose(file);
    buffer[length] = 0;
    return buffer;
}

static int open_descriptors(void) {
#ifdef __linux__
    DIR *directory = opendir("/proc/self/fd");
    if (!directory) return -1;
    int count = 0;
    while (readdir(directory)) ++count;
    closedir(directory);
    return count;
#else
    return -1;
#endif
}

static int read_once(const char *path, const char *output_name) {
    char *buffer = (char *)0x1; /* the helper must overwrite this with NULL or a payload */
    int32_t status = cheeku_read_file(path, &buffer);
    if (status == 0 ? buffer == NULL : buffer != NULL) { puts("CONTRACT"); exit(1); }
    if (status == 0 && output_name) {
        FILE *output = fopen(output_name, "wb");
        fwrite(buffer, 1, strlen(buffer), output);
        fclose(output);
        printf("%d %zu\n", (int)status, strlen(buffer));
    } else if (output_name) {
        printf("%d -\n", (int)status);
    }
    if (status == 0) PAYLOAD_FREE(buffer);
    return (int)status;
}

static int write_once(const char *path, const char *content, int print) {
    int64_t written = 99; /* must be reset to zero on failure */
    int32_t status = cheeku_write_file(path, content, &written);
    if (status != 0 && written != 0) { puts("CONTRACT"); exit(1); }
    if (print) printf("%d %lld\n", (int)status, (long long)written);
    return (int)status;
}

int main(int argc, char **argv) {
#ifndef _WIN32
    struct rlimit limit;
    if (getrlimit(RLIMIT_NOFILE, &limit) == 0 && limit.rlim_cur > 64) {
        limit.rlim_cur = 64; /* a leaked handle surfaces within a few dozen calls */
        setrlimit(RLIMIT_NOFILE, &limit);
    }
#endif
    char *path = NULL, *content = NULL;
    if (argc >= 3) path = slurp(argv[2]);
    if (argc == 4 && !strcmp(argv[1], "read")) {
        read_once(path, argv[3]);
    } else if (argc == 4 && !strcmp(argv[1], "write")) {
        content = slurp(argv[3]);
        write_once(path, content, 1);
    } else if (argc == 4 && !strcmp(argv[1], "reads")) {
        int count = atoi(argv[3]), before = open_descriptors(), first = 0, same = 0;
        for (int i = 0; i < count; ++i) {
            int status = read_once(path, NULL);
            if (i == 0) first = status;
            if (status == first) ++same;
        }
        printf("%d %d %d %d\n", first, same, before, open_descriptors());
    } else if (argc == 5 && !strcmp(argv[1], "writes")) {
        content = slurp(argv[3]);
        int count = atoi(argv[4]), before = open_descriptors(), first = 0, same = 0;
        for (int i = 0; i < count; ++i) {
            int status = write_once(path, content, 0);
            if (i == 0) first = status;
            if (status == first) ++same;
        }
        printf("%d %d %d %d\n", first, same, before, open_descriptors());
    } else {
        fputs("bad driver arguments\n", stderr);
        return 2;
    }
    free(path);
    free(content);
    return 0;
}
"##;

/// Counts allocations made by the runtime, fails the Nth one on request, and can
/// report the size of the last successful allocation.
const ALLOCATION_SHIM: &str = r##"
#include <stdio.h>
#include <stdlib.h>
static size_t live, runtime_calls, fail_at, last_size;
static int initialized;
static void check(void) {
    if (live) { fprintf(stderr, "allocation imbalance live=%zu\n", live); abort(); }
    if (getenv("CHEEKU_TEST_REPORT")) fprintf(stderr, "last_allocation=%zu\n", last_size);
}
static void initialize(void) {
    if (!initialized) {
        initialized = 1;
        const char *failure = getenv("CHEEKU_TEST_FAIL_RUNTIME");
        fail_at = failure ? (size_t)strtoul(failure, NULL, 10) : 0;
        if (atexit(check)) abort();
    }
}
void *cheeku_test_runtime_malloc(size_t bytes) {
    initialize();
    if (++runtime_calls == fail_at) return NULL;
    void *value = malloc(bytes);
    if (value) { ++live; last_size = bytes; }
    return value;
}
void *cheeku_test_realloc(void *value, size_t bytes) {
    initialize();
    if (++runtime_calls == fail_at) return NULL;
    int was_null = value == NULL;
    void *grown = realloc(value, bytes);
    if (grown && was_null) ++live;
    if (grown) last_size = bytes;
    return grown;
}
void cheeku_test_free(void *value) {
    if (!value) return;
    if (!live) abort();
    --live;
    free(value);
}
"##;

const TRACKING_MACROS: &str = "#define malloc cheeku_test_runtime_malloc\n#define free cheeku_test_free\n#define realloc cheeku_test_realloc\n";

struct Case {
    path: PathBuf,
}

struct Read {
    status: i32,
    data: Option<Vec<u8>>,
    stderr: String,
}

impl Case {
    fn new() -> Self {
        // The spaces also exercise paths that need quoting.
        let path = std::env::temp_dir().join(format!(
            "cheeku file runtime {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn build(&self, tracking: bool) -> PathBuf {
        let mut runtime = chuchu::runtime::c_source();
        if tracking {
            runtime.insert_str(0, TRACKING_MACROS);
        }
        let mut sources = vec![self.path.join("runtime.c"), self.path.join("driver.c")];
        fs::write(&sources[0], runtime).unwrap();
        fs::write(&sources[1], DRIVER).unwrap();
        let mut defines = Vec::new();
        if tracking {
            sources.push(self.path.join("shim.c"));
            fs::write(&sources[2], ALLOCATION_SHIM).unwrap();
            defines.push("CHEEKU_TEST_ALLOC");
        }
        let output = self.path.join(if cfg!(windows) {
            "driver.exe"
        } else {
            "driver"
        });
        compile_c(&self.path, &sources, &output, &defines);
        output
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, bytes).unwrap();
        path
    }

    fn argument_file(&self, bytes: &[u8]) -> String {
        let path = self.path.join(format!("argument-{}.bin", next_id()));
        fs::write(&path, bytes).unwrap();
        text(&path)
    }

    fn read(&self, driver: &Path, path: &str) -> Read {
        self.read_in(driver, path, &self.path, &[])
    }

    fn read_in(&self, driver: &Path, path: &str, cwd: &Path, env: &[(&str, String)]) -> Read {
        let output_file = self.path.join(format!("payload-{}.bin", next_id()));
        let output = execute(
            driver,
            &[
                "read",
                &self.argument_file(path.as_bytes()),
                &text(&output_file),
            ],
            cwd,
            env,
        );
        assert!(output.status.success(), "{}", describe(&output));
        let stdout = String::from_utf8(output.stdout).unwrap();
        let status = stdout.split_whitespace().next().unwrap().parse().unwrap();
        let data = (status == 0).then(|| fs::read(&output_file).unwrap());
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Read {
            status,
            data,
            stderr,
        }
    }

    fn write(&self, driver: &Path, path: &str, content: &[u8]) -> (i32, i64) {
        self.write_in(driver, path, content, &self.path)
    }

    fn write_in(&self, driver: &Path, path: &str, content: &[u8], cwd: &Path) -> (i32, i64) {
        let output = execute(
            driver,
            &[
                "write",
                &self.argument_file(path.as_bytes()),
                &self.argument_file(content),
            ],
            cwd,
            &[],
        );
        assert!(output.status.success(), "{}", describe(&output));
        let stdout = String::from_utf8(output.stdout).unwrap();
        let mut fields = stdout.split_whitespace();
        (
            fields.next().unwrap().parse().unwrap(),
            fields.next().unwrap().parse().unwrap(),
        )
    }

    /// Runs `count` calls and returns (first status, calls with that status,
    /// descriptors before, descriptors after). Descriptor counts are -1 off Linux.
    fn repeat(
        &self,
        driver: &Path,
        mode: &str,
        path: &str,
        content: &[u8],
        count: u32,
    ) -> [i64; 4] {
        let count = count.to_string();
        let path_argument = self.argument_file(path.as_bytes());
        let output = if mode == "reads" {
            execute(driver, &[mode, &path_argument, &count], &self.path, &[])
        } else {
            let content_argument = self.argument_file(content);
            execute(
                driver,
                &[mode, &path_argument, &content_argument, &count],
                &self.path,
                &[],
            )
        };
        assert!(output.status.success(), "{}", describe(&output));
        let stdout = String::from_utf8(output.stdout).unwrap();
        let fields: Vec<i64> = stdout
            .split_whitespace()
            .map(|field| field.parse().unwrap())
            .collect();
        fields.try_into().unwrap()
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn next_id() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn text(path: &Path) -> String {
    path.to_str().expect("temporary paths are UTF-8").to_owned()
}

fn describe(output: &Output) -> String {
    format!(
        "status {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A bounded run: a runtime that fails to stop reading an endless source must
/// fail the test rather than hang it.
fn execute(program: &Path, arguments: &[&str], cwd: &Path, env: &[(&str, String)]) -> Output {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(60) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native file helper did not finish within 60 seconds");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    child.wait_with_output().unwrap()
}

#[cfg_attr(not(windows), allow(unused_variables))]
fn compile_c(directory: &Path, sources: &[PathBuf], output: &Path, defines: &[&str]) {
    let linker = std::env::var_os("CHEEKU_LINKER");
    #[cfg(windows)]
    let mut command = {
        let driver = linker
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("LLVM_SYS_231_PREFIX")
                    .map(|prefix| PathBuf::from(prefix).join("bin").join("clang-cl.exe"))
                    .filter(|path| path.is_file())
            })
            .unwrap_or_else(|| "clang-cl.exe".into());
        let mut command = Command::new(driver);
        let mut objects = std::ffi::OsString::from("/Fo");
        objects.push(directory);
        objects.push("/"); // matches native.rs: clang-cl requires a directory for /Fo
        let mut executable = std::ffi::OsString::from("/Fe");
        executable.push(output);
        let mut link_output = std::ffi::OsString::from("/OUT:");
        link_output.push(output);
        command.args(["/nologo", "/MD", "/O2", "/std:c11"]);
        for define in defines {
            command.arg(format!("/D{define}"));
        }
        command.arg(objects).args(sources).arg(executable);
        command.arg("/link").arg(link_output);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = match linker {
            Some(linker) => Command::new(linker),
            None if cfg!(target_os = "macos") => {
                let mut command = Command::new("xcrun");
                command.arg("clang");
                command
            }
            None => Command::new("clang"),
        };
        command.args(["-O2", "-std=c11"]);
        for define in defines {
            command.arg(format!("-D{define}"));
        }
        if let Ok(flags) = std::env::var("CHEEKU_TEST_CFLAGS") {
            command.args(flags.split_whitespace());
        }
        command.args(sources).arg("-o").arg(output);
        if cfg!(target_os = "linux") {
            command.arg("-lm");
        }
        command
    };
    let result = command.output().expect("a C compiler is required");
    assert!(
        result.status.success(),
        "C compilation failed\n{}",
        describe(&result)
    );
}

fn assert_read(result: &Read, status: i32, data: Option<&[u8]>) {
    assert_eq!(result.status, status);
    assert_eq!(result.data.as_deref(), data);
}

#[test]
fn text_round_trips_byte_for_byte_including_bom_and_newlines() {
    let case = Case::new();
    let driver = case.build(false);
    let path = text(&case.path.join("round trip.txt"));
    for content in [
        "",
        "plain ASCII",
        "héllo wörld 日本語 🌍\n",
        "unix\nwindows\r\nlone\rcarriage\n",
        "\u{feff}starts with a byte-order mark",
        "\u{feff}",
        "tab\t quote\" backslash\\ percent %s %d",
        "no trailing newline",
        "trailing newlines\n\n\n",
        "U+10FFFF \u{10ffff} and U+D7FF \u{d7ff}",
    ] {
        let (status, written) = case.write(&driver, &path, content.as_bytes());
        assert_eq!((status, written), (0, content.len() as i64), "{content:?}");
        assert_eq!(fs::read(&path).unwrap(), content.as_bytes());
        assert_read(&case.read(&driver, &path), 0, Some(content.as_bytes()));
    }
    // A byte-order mark is content: it is neither stripped on read nor added on write.
    case.file("bom.txt", b"\xEF\xBB\xBFx");
    assert_read(
        &case.read(&driver, &text(&case.path.join("bom.txt"))),
        0,
        Some(b"\xEF\xBB\xBFx"),
    );
}

#[test]
fn writing_creates_overwrites_and_truncates() {
    let case = Case::new();
    let driver = case.build(false);
    let path = case.path.join("target.txt");
    let name = text(&path);
    assert!(!path.exists());
    assert_eq!(
        case.write(&driver, &name, b"first, longer content"),
        (0, 21)
    );
    assert_eq!(fs::read(&path).unwrap(), b"first, longer content");
    assert_eq!(case.write(&driver, &name, b"short"), (0, 5));
    assert_eq!(fs::read(&path).unwrap(), b"short");
    assert_eq!(case.write(&driver, &name, b""), (0, 0));
    assert_eq!(fs::read(&path).unwrap(), b"");
    assert_read(&case.read(&driver, &name), 0, Some(b""));
}

#[test]
fn open_and_io_failures_report_code_4_without_payload() {
    let case = Case::new();
    let driver = case.build(false);
    let missing = text(&case.path.join("missing.txt"));
    assert_read(&case.read(&driver, &missing), 4, None);
    assert_read(&case.read(&driver, ""), 4, None);
    assert_read(&case.read(&driver, &"a".repeat(5000)), 4, None);
    assert_read(&case.read(&driver, &text(&case.path)), 4, None);

    assert_eq!(case.write(&driver, "", b"x"), (4, 0));
    assert_eq!(case.write(&driver, &"a".repeat(5000), b"x"), (4, 0));
    assert_eq!(case.write(&driver, &text(&case.path), b"x"), (4, 0));
    let orphan = text(&case.path.join("no such directory").join("file.txt"));
    assert_eq!(case.write(&driver, &orphan, b"x"), (4, 0));
    assert!(!case.path.join("no such directory").exists());
}

#[cfg(unix)]
#[test]
fn permission_errors_report_code_4() {
    use std::os::unix::fs::PermissionsExt;
    let case = Case::new();
    let driver = case.build(false);
    let secret = case.file("secret.txt", b"hidden");
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&secret).is_ok() {
        eprintln!("skipping the unreadable-file case: running with elevated privileges");
    } else {
        assert_read(&case.read(&driver, &text(&secret)), 4, None);
    }
    let locked = case.file("locked.txt", b"keep");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o444)).unwrap();
    if fs::OpenOptions::new().write(true).open(&locked).is_ok() {
        eprintln!("skipping the read-only-file case: running with elevated privileges");
    } else {
        assert_eq!(case.write(&driver, &text(&locked), b"changed"), (4, 0));
        assert_eq!(fs::read(&locked).unwrap(), b"keep");
    }
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_full_device_is_reported_at_write_flush_or_close() {
    if !Path::new("/dev/full").exists() {
        return;
    }
    let case = Case::new();
    let driver = case.build(false);
    assert_eq!(case.write(&driver, "/dev/full", b"data"), (4, 0));
}

#[cfg(windows)]
#[test]
fn invalid_windows_names_report_code_4() {
    let case = Case::new();
    let driver = case.build(false);
    let invalid = text(&case.path.join("bad<name>?.txt"));
    assert_read(&case.read(&driver, &invalid), 4, None);
    assert_eq!(case.write(&driver, &invalid, b"x"), (4, 0));
}

#[test]
fn invalid_utf8_and_nul_use_the_existing_input_error_codes() {
    let case = Case::new();
    let driver = case.build(false);
    let cases: [(&str, &[u8], i32); 20] = [
        ("lone continuation", b"\x80", 5),
        ("0xFF byte", b"abc\xFF", 5),
        ("overlong NUL", b"\xC0\x80", 5),
        ("overlong slash", b"\xE0\x80\xAF", 5),
        ("surrogate", b"\xED\xA0\x80", 5),
        ("above U+10FFFF", b"\xF4\x90\x80\x80", 5),
        ("0xF5 lead byte", b"\xF5\x80\x80\x80", 5),
        ("truncated two-byte", b"ok\xC3", 5),
        ("truncated three-byte at end", b"ok\xE2\x82", 5),
        ("truncated four-byte at end", b"ok\xF0\x9F\x8C", 5),
        ("bad continuation byte", b"\xE2\x28\xA1", 5),
        ("embedded NUL", b"ok\0ok", 6),
        ("leading NUL", b"\0", 6),
        ("trailing NUL", b"text\0", 6),
        ("UTF-16LE with BOM", b"\xFF\xFEh\0i\0", 6),
        ("NUL wins over invalid UTF-8", b"\xFF\0", 6),
        ("NUL wins when it comes first", b"\0\xFF", 6),
        ("only a BOM", b"\xEF\xBB\xBF", 0),
        ("U+10FFFF", b"\xF4\x8F\xBF\xBF", 0),
        ("U+D7FF", b"\xED\x9F\xBF", 0),
    ];
    for (label, bytes, status) in cases {
        let path = case.file("content.bin", bytes);
        let result = case.read(&driver, &text(&path));
        let expected = (status == 0).then_some(bytes);
        assert_eq!(result.status, status, "{label}");
        assert_eq!(result.data.as_deref(), expected, "{label}");
    }
}

#[test]
fn unicode_relative_and_absolute_paths_work() {
    let case = Case::new();
    let driver = case.build(false);
    let name = "données-日本語 🌍 ü.txt";
    let absolute = case.path.join(name);
    let greeting = "Привет, мир".as_bytes();
    assert_eq!(case.write(&driver, &text(&absolute), greeting), (0, 20));
    assert_eq!(fs::read(&absolute).unwrap(), greeting);
    assert_read(&case.read(&driver, &text(&absolute)), 0, Some(greeting));

    // Relative paths follow the working directory, not the executable's location.
    let nested = "sub dir/ñandú 中文.txt";
    case.file("sub dir/placeholder", b"");
    assert_eq!(
        case.write_in(&driver, nested, b"relative", &case.path),
        (0, 8)
    );
    assert_eq!(fs::read(case.path.join(nested)).unwrap(), b"relative");
    let child = case.path.join("sub dir");
    let result = case.read_in(&driver, "ñandú 中文.txt", &child, &[]);
    assert_read(&result, 0, Some(b"relative"));
    let parent = case.read_in(&driver, "../données-日本語 🌍 ü.txt", &child, &[]);
    assert_read(&parent, 0, Some(greeting));
    assert_eq!(case.read_in(&driver, nested, &child, &[]).status, 4);
}

#[test]
fn large_files_and_buffer_growth_boundaries_round_trip() {
    let case = Case::new();
    let driver = case.build(false);
    // Sizes straddle the 4096-byte initial capacity and each doubling.
    for size in [
        0, 1, 4093, 4094, 4095, 4096, 4097, 8190, 8191, 8192, 8193, 12_345,
    ] {
        let bytes = vec![b'a'; size];
        let path = case.file("boundary.txt", &bytes);
        assert_read(&case.read(&driver, &text(&path)), 0, Some(&bytes));
    }
    let unit = "héllo wörld 日本語 🌍 line\r\n";
    let mut large = String::new();
    while large.len() < 16 * 1024 * 1024 + 7 {
        large.push_str(unit);
    }
    let path = text(&case.path.join("large.txt"));
    assert_eq!(
        case.write(&driver, &path, large.as_bytes()),
        (0, large.len() as i64)
    );
    assert_eq!(fs::read(&path).unwrap(), large.as_bytes());
    assert_read(&case.read(&driver, &path), 0, Some(large.as_bytes()));
}

#[cfg(target_os = "linux")]
#[test]
fn sources_with_unreported_or_unbounded_size_are_handled() {
    let case = Case::new();
    let driver = case.build(false);
    // /proc files report size zero but contain text.
    let proc = case.read(&driver, "/proc/self/status");
    assert_eq!(proc.status, 0);
    assert!(!proc.data.unwrap().is_empty());
    assert_read(&case.read(&driver, "/dev/null"), 0, Some(b""));
    // An endless source of NUL bytes must stop at the first NUL.
    assert_read(&case.read(&driver, "/dev/zero"), 6, None);
}

#[test]
fn file_handles_are_closed_on_every_success_and_failure_path() {
    let case = Case::new();
    let driver = case.build(false);
    let good = text(&case.file("good.txt", b"fine"));
    let bad_utf8 = text(&case.file("bad-utf8.bin", b"\xFF"));
    let nul = text(&case.file("nul.bin", b"a\0b"));
    let directory = text(&case.path);
    for (path, status) in [(&good, 0), (&bad_utf8, 5), (&nul, 6), (&directory, 4)] {
        let [first, same, before, after] = case.repeat(&driver, "reads", path, b"", 3000);
        assert_eq!((first, same), (status, 3000), "{path}");
        assert_eq!(before, after, "descriptors leaked reading {path}");
    }
    let target = text(&case.path.join("repeated.txt"));
    let [first, same, before, after] = case.repeat(&driver, "writes", &target, b"payload", 3000);
    assert_eq!((first, same), (0, 3000));
    assert_eq!(before, after, "descriptors leaked while writing");
    let [first, same, before, after] = case.repeat(&driver, "writes", &directory, b"payload", 3000);
    assert_eq!((first, same), (4, 3000));
    assert_eq!(before, after);
}

#[cfg(not(windows))]
#[test]
fn allocation_failures_are_recoverable_and_leave_nothing_allocated() {
    let case = Case::new();
    let driver = case.build(true);
    let content = "héllo 日本 🌍\n".repeat(60_000);
    let path = text(&case.file("large.txt", content.as_bytes()));
    let mut statuses = Vec::new();
    for fail_at in 1..=40 {
        // A non-zero exit would mean a crash or an allocation-balance failure.
        let result = case.read_in(
            &driver,
            &path,
            &case.path,
            &[("CHEEKU_TEST_FAIL_RUNTIME", fail_at.to_string())],
        );
        match result.status {
            0 => assert_eq!(result.data.as_deref(), Some(content.as_bytes())),
            7 => assert!(result.data.is_none()),
            other => panic!("allocation failure {fail_at} produced status {other}"),
        }
        statuses.push(result.status);
    }
    assert_eq!(statuses[0], 7, "the first allocation can fail");
    assert_eq!(*statuses.last().unwrap(), 0, "later calls succeed");
    assert!(
        statuses.iter().filter(|&&status| status == 7).count() >= 5,
        "growth reallocations are checked: {statuses:?}"
    );
    // Once the injected failure is beyond the last allocation, it stays beyond it.
    let first_success = statuses.iter().position(|&status| status == 0).unwrap();
    assert!(statuses[first_success..].iter().all(|&status| status == 0));
    // Failing the final best-effort shrink still returns the complete file.
    assert!(first_success >= 1);
}

#[cfg(not(windows))]
#[test]
fn unused_buffer_capacity_is_returned_to_the_allocator() {
    let case = Case::new();
    let driver = case.build(true);
    // The read buffer starts at 4096 bytes and doubles. Whatever the file size,
    // the block handed back must be exactly the text plus its terminator.
    for size in [0usize, 10, 4094, 5000, 8191, 70_000] {
        let path = text(&case.file("sized.txt", &vec![b'x'; size]));
        let result = case.read_in(
            &driver,
            &path,
            &case.path,
            &[("CHEEKU_TEST_REPORT", "1".to_owned())],
        );
        assert_eq!(result.status, 0, "size {size}");
        assert!(
            result
                .stderr
                .contains(&format!("last_allocation={}", size + 1)),
            "a {size}-byte file kept an oversized buffer: {}",
            result.stderr
        );
    }
}

#[test]
fn utf8_validation_agrees_with_rust_on_pseudorandom_inputs() {
    let case = Case::new();
    let driver = case.build(false);
    // xorshift64 with a fixed seed, so any failure is reproducible.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // Whole sequences on each side of every UTF-8 boundary. Uniformly random
    // bytes almost never form a surrogate or an overlong sequence, so inputs are
    // built from these fragments and then sometimes damaged by one random byte.
    // NUL is excluded because it is reported as code 6, not 5.
    let fragments: [&[u8]; 30] = [
        b"a",
        b"\xC2\x80",
        b"\xDF\xBF",
        b"\xE0\xA0\x80",
        b"\xED\x9F\xBF",
        b"\xEE\x80\x80",
        b"\xEF\xBF\xBF",
        b"\xF0\x90\x80\x80",
        b"\xF4\x8F\xBF\xBF",
        b"\xED\xA0\x80",
        b"\xED\xBF\xBF",
        b"\xE0\x80\x80",
        b"\xE0\x9F\xBF",
        b"\xF0\x80\x80\x80",
        b"\xF0\x8F\xBF\xBF",
        b"\xF4\x90\x80\x80",
        b"\xC0\xAF",
        b"\xC1\xBF",
        b"\xF5\x80\x80\x80",
        b"\xF8\x88\x80\x80\x80",
        b"\x80",
        b"\xBF",
        b"\xC2",
        b"\xE2\x82",
        b"\xF0\x9F\x8C",
        b"\xFE",
        b"\xFF",
        b"\xE2\x82\xAC",
        b"\xF0\x9D\x84\x9E",
        b"\xC3\xA9",
    ];
    let (mut valid, mut invalid) = (0, 0);
    for _ in 0..400 {
        let mut bytes = Vec::new();
        for _ in 0..1 + next() % 3 {
            bytes.extend_from_slice(fragments[(next() % fragments.len() as u64) as usize]);
        }
        if next() % 3 == 0 {
            let index = (next() % bytes.len() as u64) as usize;
            // Any byte except NUL.
            bytes[index] = 1 + (next() % 255) as u8;
        }
        let path = text(&case.file("random.bin", &bytes));
        let result = case.read(&driver, &path);
        if std::str::from_utf8(&bytes).is_ok() {
            valid += 1;
            assert_read(&result, 0, Some(&bytes));
        } else {
            invalid += 1;
            assert_read(&result, 5, None);
        }
    }
    assert!(
        valid >= 40 && invalid >= 40,
        "valid {valid}, invalid {invalid}"
    );
}
