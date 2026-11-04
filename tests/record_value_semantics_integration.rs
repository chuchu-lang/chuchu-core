//! Record value semantics (Issue #11, steps 7C to 7F): field mutation, copying and
//! reassignment, string and nested-record fields, ownership and cleanup, and
//! records as function parameters and results.
//!
//! Every program below runs twice. First through the real `cheeku` binary at -O0
//! through -O3, with and without debug information, where the output must be
//! identical everywhere. Then under AddressSanitizer and LeakSanitizer, where any
//! leak, double free, invalid free, or use after free fails the test. The
//! sanitizer run needs the clang that matches the LLVM the compiler was built
//! against; without one the sanitizer half is skipped with a message, unless
//! `CHUCHU_REQUIRE_SANITIZER` is set, which makes a missing sanitizer an error.
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use support::{executable_name, native_output};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Case {
    directory: PathBuf,
}

impl Case {
    fn new() -> Self {
        for _ in 0..100 {
            let path = std::env::temp_dir().join(format!(
                "cheeku record values {} {}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self { directory: path },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("Cannot create test directory: {error}"),
            }
        }
        panic!("Cannot allocate test directory");
    }
    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }
    fn compile(&self, source: &str, flags: &[&str]) -> Output {
        let input = self.path("case.chuchu");
        fs::write(&input, source).unwrap();
        Command::new(env!("CARGO_BIN_EXE_cheeku"))
            .arg(&input)
            .arg("-o")
            .arg(self.path(&executable_name("program")))
            .args(flags)
            .current_dir(&self.directory)
            .output()
            .unwrap()
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Every optimization level, each with and without debug information.
fn builds() -> Vec<Vec<&'static str>> {
    let mut builds = Vec::new();
    for level in ["-O0", "-O1", "-O2", "-O3"] {
        builds.push(vec![level]);
        builds.push(vec![level, "-g"]);
    }
    builds
}

/// Runs `source` natively in every build and requires exactly `expected`.
fn prints(source: &str, expected: &str) {
    for flags in builds() {
        let case = Case::new();
        let compiled = case.compile(source, &flags);
        assert!(
            compiled.status.success(),
            "{flags:?}\n{source}\n{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(case.path(&executable_name("program")))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&native_output(expected)),
            "{flags:?}\n{source}"
        );
        assert!(output.stderr.is_empty(), "{flags:?}");
        assert_eq!(output.status.code(), Some(0), "{flags:?}");
    }
}

// ----------------------------------------------------------------------------
// Sanitizer support
// ----------------------------------------------------------------------------

/// The clang that matches the LLVM the compiler uses: the one in the LLVM prefix
/// the build was configured with.
fn matching_clang() -> Option<PathBuf> {
    let prefix = std::env::var_os("LLVM_SYS_231_PREFIX")?;
    let clang =
        PathBuf::from(prefix)
            .join("bin")
            .join(if cfg!(windows) { "clang.exe" } else { "clang" });
    clang.exists().then_some(clang)
}

/// Whether programs can be built and run under AddressSanitizer on this host.
fn sanitizer() -> Option<&'static Path> {
    static CLANG: OnceLock<Option<PathBuf>> = OnceLock::new();
    CLANG
        .get_or_init(|| {
            let supported = cfg!(any(target_os = "linux", target_os = "macos"))
                && cfg!(any(target_arch = "x86_64", target_arch = "aarch64"));
            let clang = matching_clang().filter(|_| supported);
            let usable = clang.as_ref().is_some_and(|clang| {
                let case = Case::new();
                let source = case.path("probe.c");
                fs::write(&source, "int main(void) { return 0; }\n").unwrap();
                let program = case.path("probe");
                Command::new(clang)
                    .args(["-fsanitize=address"])
                    .arg(&source)
                    .arg("-o")
                    .arg(&program)
                    .output()
                    .is_ok_and(|built| built.status.success())
                    && Command::new(&program)
                        .output()
                        .is_ok_and(|run| run.status.success())
            });
            if usable {
                clang
            } else {
                assert!(
                    std::env::var_os("CHUCHU_REQUIRE_SANITIZER").is_none(),
                    "CHUCHU_REQUIRE_SANITIZER is set but AddressSanitizer is unavailable"
                );
                eprintln!("note: AddressSanitizer is unavailable; skipping sanitizer runs");
                None
            }
        })
        .as_deref()
}

/// The IR with every function marked for AddressSanitizer instrumentation. LLVM
/// only instruments functions that carry the attribute, and IR written by the
/// compiler does not. With debug information a definition ends in `!dbg !N {`, and
/// the attribute belongs before the metadata.
fn instrumented(ir: &str) -> String {
    ir.lines()
        .map(|line| {
            if line.starts_with("define ") && line.ends_with(" {") {
                let head = &line[..line.len() - 2];
                match head.rfind(" !dbg ") {
                    Some(at) => format!("{} sanitize_address{} {{", &head[..at], &head[at..]),
                    None => format!("{head} sanitize_address {{"),
                }
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Builds `ir` (a module for `main`) with the Cheeku runtime under AddressSanitizer
/// and runs it with leak detection on.
fn run_instrumented(case: &Case, clang: &Path, ir: &str) -> Output {
    fs::write(case.path("module.ll"), ir).unwrap();
    fs::write(case.path("runtime.c"), chuchu::runtime::c_source()).unwrap();
    let program = case.path("sanitized");
    let built = Command::new(clang)
        .args(["-g", "-O0", "-fsanitize=address", "-Wno-override-module"])
        .arg(case.path("module.ll"))
        .arg(case.path("runtime.c"))
        .arg("-o")
        .arg(&program)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}\n{ir}",
        String::from_utf8_lossy(&built.stderr)
    );
    Command::new(&program)
        .env(
            "ASAN_OPTIONS",
            "detect_leaks=1:detect_stack_use_after_return=1",
        )
        .output()
        .unwrap()
}

/// The IR `cheeku` generates for `source` with the given flags.
fn emit_ir(case: &Case, source: &str, flags: &[&str]) -> String {
    let ir = case.path("out.ll");
    let mut flags = flags.to_vec();
    flags.extend(["--emit-ir", ir.to_str().unwrap()]);
    let compiled = case.compile(source, &flags);
    assert!(
        compiled.status.success(),
        "{source}\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::read_to_string(ir).unwrap()
}

/// Runs `source` under AddressSanitizer and LeakSanitizer at -O0, -O2, and -O3 with
/// debug information, and requires `expected` output and no sanitizer report of
/// any kind.
fn is_memory_safe(source: &str, expected: &str) {
    let Some(clang) = sanitizer() else {
        return;
    };
    for flags in [vec!["-O0"], vec!["-O2"], vec!["-O3", "-g"]] {
        let case = Case::new();
        let ir = instrumented(&emit_ir(&case, source, &flags));
        let output = run_instrumented(&case, clang, &ir);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stderr.is_empty(),
            "{flags:?}: status {:?}\n{stderr}\n{source}",
            output.status.code()
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            expected,
            "{flags:?}\n{source}"
        );
    }
}

/// A program that must produce `expected` natively everywhere and be memory safe.
fn check(source: &str, expected: &str) {
    prints(source, expected);
    is_memory_safe(source, expected);
}

// ----------------------------------------------------------------------------
// Shared declarations
// ----------------------------------------------------------------------------

const TYPES: &str = "record Address {\n    city: string\n    zip: int\n}\nrecord Student {\n    name: string\n    age: int\n    gpa: float\n    active: bool\n    address: Address\n}\n";

const ALI: &str = "assign ali as Student {\n    name: \"Ali\"\n    age: 22\n    gpa: 3.5\n    active: true\n    address: Address {\n        city: \"Lahore\"\n        zip: 54000\n    }\n}\n";

fn program(body: &str) -> String {
    format!("{TYPES}{ALI}{body}")
}

// ----------------------------------------------------------------------------
// 7C: field mutation
// ----------------------------------------------------------------------------

#[test]
fn scalar_fields_can_be_mutated_without_touching_the_others() {
    check(
        &program(
            "reassign ali.age as 25\nreassign ali.gpa as 4\nreassign ali.active as false\nshow ali.name\nshow ali.age\nshow ali.gpa\nshow ali.active\nshow ali.address.city\nshow ali.address.zip",
        ),
        "Ali\n25\n4\nfalse\nLahore\n54000\n",
    );
}

#[test]
fn string_fields_can_be_replaced_and_extended() {
    check(
        &program(
            "reassign ali.name as \"Sara\"\nshow ali.name\nreassign ali.name as ali.name + \" Khan\"\nshow ali.name\nshow length(ali.name)",
        ),
        "Sara\nSara Khan\n9\n",
    );
}

#[test]
fn repeated_string_overwrites_do_not_leak() {
    check(
        &program(
            "assign i as 0\nwhile i < 500 {\n    reassign ali.name as \"n\" + \"x\"\n    reassign ali.address.city as ali.name + \"-city\"\n    reassign i as i + 1\n}\nshow ali.name\nshow ali.address.city",
        ),
        "nx\nnx-city\n",
    );
}

#[test]
fn nested_fields_can_be_mutated() {
    check(
        &program(
            "reassign ali.address.city as \"Karachi\"\nreassign ali.address.zip as ali.address.zip + 1\nshow ali.address.city\nshow ali.address.zip\nshow ali.name\nshow ali.age",
        ),
        "Karachi\n54001\nAli\n22\n",
    );
}

#[test]
fn a_whole_nested_record_field_can_be_replaced() {
    check(
        &program(
            "assign home as Address {\n    city: \"Multan\"\n    zip: 60000\n}\nreassign ali.address as home\nreassign home.city as \"Changed\"\nshow ali.address.city\nshow home.city\nreassign ali.address as Address {\n    city: \"Temp\"\n    zip: 1\n}\nshow ali.address.city\nshow ali.address.zip\nshow ali.name",
        ),
        "Multan\nChanged\nTemp\n1\nAli\n",
    );
}

#[test]
fn mutation_values_follow_the_existing_numeric_rules() {
    check(
        &program(
            "reassign ali.gpa as ali.age\nshow ali.gpa\nreassign ali.gpa as 1 / 2\nshow ali.gpa\nreassign ali.age as 2 * 3 + 1\nshow ali.age",
        ),
        "22\n0\n7\n",
    );
}

#[test]
fn the_mutated_value_is_evaluated_once_and_in_order() {
    check(
        &format!(
            "function noisy(n: int) be int {{\n    show n\n    return n\n}}\n{TYPES}{ALI}reassign ali.age as noisy(5)\nshow ali.age\nreassign ali.address.zip as noisy(ali.age + 1)\nshow ali.address.zip"
        ),
        "5\n5\n6\n6\n",
    );
}

// ----------------------------------------------------------------------------
// 7D: copying and reassignment
// ----------------------------------------------------------------------------

#[test]
fn a_copy_is_independent_of_the_original() {
    check(
        &program(
            "assign bo as ali\nreassign bo.name as \"Bo\"\nreassign bo.age as 30\nreassign bo.address.city as \"Quetta\"\nshow ali.name\nshow ali.age\nshow ali.address.city\nshow bo.name\nshow bo.age\nshow bo.address.city\nreassign ali.name as \"Changed\"\nreassign ali.address.zip as 7\nshow bo.name\nshow bo.address.zip\nshow ali.address.zip",
        ),
        "Ali\n22\nLahore\nBo\n30\nQuetta\nBo\n54000\n7\n",
    );
}

#[test]
fn copies_of_copies_stay_independent() {
    check(
        &program(
            "assign b as ali\nassign c as b\nassign d as c\nreassign d.name as \"D\"\nreassign b.name as \"B\"\nshow ali.name\nshow b.name\nshow c.name\nshow d.name",
        ),
        "Ali\nB\nAli\nD\n",
    );
}

#[test]
fn a_whole_record_can_be_reassigned() {
    check(
        &program(
            "assign bo as Student {\n    name: \"Bo\"\n    age: 1\n    gpa: 1.0\n    active: false\n    address: Address {\n        city: \"B\"\n        zip: 2\n    }\n}\nreassign ali as bo\nreassign bo.name as \"Zed\"\nshow ali.name\nshow ali.address.city\nshow ali.active\nshow bo.name\nassign i as 0\nwhile i < 200 {\n    reassign ali as bo\n    reassign bo.age as bo.age + 1\n    reassign i as i + 1\n}\nshow ali.age\nshow bo.age",
        ),
        "Bo\nB\nfalse\nZed\n200\n201\n",
    );
}

#[test]
fn self_assignment_is_safe() {
    check(
        &program(
            "reassign ali as ali\nshow ali.name\nshow ali.address.city\nreassign ali.address as ali.address\nreassign ali.name as ali.name\nshow ali.name\nshow ali.address.city\nreassign ali as ali\nreassign ali as ali\nshow ali.address.zip",
        ),
        "Ali\nLahore\nAli\nLahore\n54000\n",
    );
}

#[test]
fn a_record_can_be_reassigned_from_an_initializer() {
    check(
        &program(
            "reassign ali as Student {\n    name: ali.name + \"2\"\n    age: ali.age + 1\n    gpa: ali.gpa\n    active: false\n    address: ali.address\n}\nshow ali.name\nshow ali.age\nshow ali.active\nshow ali.address.city",
        ),
        "Ali2\n23\nfalse\nLahore\n",
    );
}

// ----------------------------------------------------------------------------
// 7E: strings, nested records, and memory
// ----------------------------------------------------------------------------

#[test]
fn deeply_nested_records_are_copied_and_mutated_at_every_depth() {
    check(
        "record L4 {\n    s: string\n    n: int\n}\nrecord L3 {\n    s: string\n    inner: L4\n}\nrecord L2 {\n    s: string\n    inner: L3\n}\nrecord L1 {\n    s: string\n    inner: L2\n}\nassign a as L1 {\n    s: \"a\"\n    inner: L2 {\n        s: \"b\"\n        inner: L3 {\n            s: \"c\"\n            inner: L4 {\n                s: \"d\"\n                n: 4\n            }\n        }\n    }\n}\nassign b as a\nreassign b.inner.inner.inner.s as \"D\"\nreassign b.inner.inner.inner.n as 40\nreassign b.inner.s as \"B\"\nshow a.s\nshow a.inner.s\nshow a.inner.inner.s\nshow a.inner.inner.inner.s\nshow a.inner.inner.inner.n\nshow b.s\nshow b.inner.s\nshow b.inner.inner.s\nshow b.inner.inner.inner.s\nshow b.inner.inner.inner.n\nreassign a as b\nreassign b.inner.inner.s as \"changed\"\nshow a.inner.inner.s",
        "a\nb\nc\nd\n4\na\nB\nc\nD\n40\nc\n",
    );
}

#[test]
fn records_with_several_strings_own_each_one() {
    check(
        "record Pair {\n    left: string\n    right: string\n}\nassign p as Pair {\n    left: \"L\"\n    right: \"R\"\n}\nassign q as p\nreassign q.left as q.left + q.right\nshow p.left\nshow q.left\nshow p.right\nshow length(q.left)\nreassign p.right as q.left + p.left\nshow p.right\nshow q.right",
        "L\nLR\nR\n2\nLRL\nR\n",
    );
}

#[test]
fn records_without_strings_are_plain_values() {
    check(
        "record Point {\n    x: int\n    y: float\n    on: bool\n}\nassign a as Point {\n    x: 1\n    y: 2\n    on: true\n}\nassign b as a\nreassign b.x as 10\nreassign a.y as 0.5\nshow a.x\nshow a.y\nshow b.x\nshow b.y\nreassign a as b\nshow a.x\nshow a.on",
        "1\n0.5\n10\n2\n10\ntrue\n",
    );
}

#[test]
fn records_that_leave_scope_are_released() {
    check(
        &format!(
            "{TYPES}{ALI}assign i as 0\nwhile i < 100 {{\n    assign tmp as Student {{\n        name: \"t\" + \"x\"\n        age: i\n        gpa: 0.0\n        active: false\n        address: Address {{\n            city: \"c\" + \"d\"\n            zip: i\n        }}\n    }}\n    if i == 99 {{\n        show tmp.address.city\n        show tmp.address.zip\n    }}\n    reassign i as i + 1\n}}\nif true {{\n    assign inner as ali\n    reassign inner.name as \"In\"\n    show inner.name\n}}\nshow ali.name\nfunction scoped(n: int) be int {{\n    assign r as Address {{\n        city: \"f\" + \"g\"\n        zip: n\n    }}\n    return r.zip\n}}\nshow scoped(5)"
        ),
        "cd\n99\nIn\nAli\n5\n",
    );
}

#[test]
fn records_released_at_the_end_of_the_program_and_by_early_return() {
    check(
        &format!(
            "{TYPES}function early(flag: bool, s: Student) be string {{\n    assign extra as s\n    reassign extra.name as extra.name + \"!\"\n    if flag {{\n        return extra.name\n    }}\n    while true {{\n        assign inside as extra\n        return inside.address.city\n    }}\n    return \"unreachable\"\n}}\n{ALI}show early(true, ali)\nshow early(false, ali)\nassign last as ali\nreassign last.name as \"Last\"\nshow last.name"
        ),
        "Ali!\nLahore\nLast\n",
    );
}

#[test]
fn temporaries_are_released_after_use() {
    check(
        &format!(
            "{TYPES}function make(n: string) be Student {{\n    return Student {{\n        name: n\n        age: 1\n        gpa: 0.5\n        active: true\n        address: Address {{\n            city: n + \"-city\"\n            zip: 9\n        }}\n    }}\n}}\nshow make(\"Tmp\").name\nshow make(\"Tmp\").address.city\nshow length(make(\"abc\").name)\nshow make(\"A\").age + make(\"B\").age\nassign t as make(\"T\")\nshow t.address.zip\nshow Student {{\n    name: \"lit\" + \"eral\"\n    age: 0\n    gpa: 0.0\n    active: false\n    address: Address {{\n        city: \"x\"\n        zip: 0\n    }}\n}}.name"
        ),
        "Tmp\nTmp-city\n3\n2\n9\nliteral\n",
    );
}

#[test]
fn a_string_field_of_a_temporary_survives_the_temporary() {
    check(
        &format!(
            "{TYPES}function city(n: string) be Address {{\n    return Address {{\n        city: n + n\n        zip: 0\n    }}\n}}\nassign kept as city(\"ab\").city\nassign joined as kept + city(\"c\").city\nshow kept\nshow joined"
        ),
        "abab\nababcc\n",
    );
}

// ----------------------------------------------------------------------------
// 7F: parameters and results
// ----------------------------------------------------------------------------

#[test]
fn arguments_are_passed_by_value() {
    check(
        &format!(
            "{TYPES}function bump(s: Student) be int {{\n    reassign s.age as s.age + 100\n    reassign s.name as \"Mutated\"\n    reassign s.address.city as \"Nowhere\"\n    return s.age\n}}\n{ALI}show bump(ali)\nshow ali.name\nshow ali.age\nshow ali.address.city\nshow bump(ali) + bump(ali)\nshow ali.age"
        ),
        "122\nAli\n22\nLahore\n244\n22\n",
    );
}

#[test]
fn a_locally_built_record_can_be_returned() {
    check(
        &format!(
            "{TYPES}function build(n: string, z: int) be Address {{\n    assign a as Address {{\n        city: n + \"!\"\n        zip: z\n    }}\n    return a\n}}\nassign x as build(\"X\", 1)\nassign y as build(\"Y\", 2)\nshow x.city\nshow y.city\nshow x.zip + y.zip\nreassign x as build(\"Z\", 3)\nshow x.city\nshow x.zip\nshow y.city"
        ),
        "X!\nY!\n3\nZ!\n3\nY!\n",
    );
}

#[test]
fn records_pass_through_functions_unchanged() {
    check(
        &format!(
            "{TYPES}function id(s: Student) be Student {{\n    return s\n}}\nfunction city_of(a: Address) be string {{\n    return a.city\n}}\nfunction move(s: Student, c: string) be Student {{\n    reassign s.address.city as c\n    return s\n}}\n{ALI}assign moved as move(ali, \"Moved\")\nshow city_of(moved.address)\nshow moved.name\nshow ali.address.city\nshow id(id(id(ali))).name\nshow city_of(id(moved).address)"
        ),
        "Moved\nAli\nLahore\nAli\nMoved\n",
    );
}

#[test]
fn functions_may_take_several_records() {
    check(
        &format!(
            "{TYPES}function older(a: Student, b: Student) be string {{\n    if a.age > b.age {{\n        return a.name\n    }}\n    return b.name\n}}\nfunction pick(flag: bool, a: Address, b: Address) be Address {{\n    if flag {{\n        return a\n    }}\n    return b\n}}\n{ALI}assign bo as ali\nreassign bo.name as \"Bo\"\nreassign bo.age as 40\nshow older(ali, bo)\nshow older(bo, ali)\nshow pick(true, ali.address, bo.address).city\nreassign bo.address.city as \"Other\"\nshow pick(false, ali.address, bo.address).city\nshow pick(true, bo.address, ali.address).city"
        ),
        "Bo\nBo\nLahore\nOther\nOther\n",
    );
}

#[test]
fn records_work_through_recursion() {
    check(
        "record Acc {\n    text: string\n    count: int\n}\nfunction grow(n: int, a: Acc) be Acc {\n    if n == 0 {\n        return a\n    }\n    reassign a.text as a.text + \"x\"\n    reassign a.count as a.count + 1\n    return grow(n - 1, a)\n}\nassign r as grow(5, Acc {\n    text: \"s\"\n    count: 0\n})\nshow r.text\nshow r.count\nassign again as grow(3, r)\nshow again.text\nshow r.text",
        "sxxxxx\n5\nsxxxxxxxx\nsxxxxx\n",
    );
}

#[test]
fn nested_mutation_and_reads_work_inside_functions() {
    check(
        "record Inner {\n    label: string\n    n: int\n}\nrecord Outer {\n    inner: Inner\n    tag: string\n}\nfunction relabel(o: Outer, l: string) be Outer {\n    reassign o.inner.label as l\n    reassign o.inner.n as o.inner.n * 2\n    reassign o.tag as o.tag + o.inner.label\n    return o\n}\nassign o as Outer {\n    inner: Inner {\n        label: \"in\"\n        n: 21\n    }\n    tag: \"t\"\n}\nassign p as relabel(o, \"new\")\nshow o.inner.label\nshow o.inner.n\nshow o.tag\nshow p.inner.label\nshow p.inner.n\nshow p.tag",
        "in\n21\nt\nnew\n42\ntnew\n",
    );
}

#[test]
fn int_converts_to_float_in_record_arguments_and_results() {
    check(
        "record Measure {\n    value: float\n    unit: string\n}\nfunction scale(m: Measure, k: int) be Measure {\n    reassign m.value as m.value * k\n    return m\n}\nassign m as Measure {\n    value: 3\n    unit: \"cm\"\n}\nshow scale(m, 2).value / 4\nshow m.value\nreassign m.value as m.value + 1\nshow m.value",
        "1.5\n3\n4\n",
    );
}

#[test]
fn a_function_can_build_and_return_a_record_with_a_string_from_its_arguments() {
    check(
        "record Tag {\n    text: string\n    size: int\n}\nfunction tag(t: string) be Tag {\n    return Tag {\n        text: t + t\n        size: length(t)\n    }\n}\nassign a as tag(\"ab\")\nassign b as tag(a.text)\nshow a.text\nshow a.size\nshow b.text\nshow b.size",
        "abab\n2\nabababab\n4\n",
    );
}

// ----------------------------------------------------------------------------
// Safety nets
// ----------------------------------------------------------------------------

#[test]
fn runtime_checks_still_apply_to_record_values() {
    let source = program("show 1\nreassign ali.age as 9223372036854775807\nshow ali.age + 1");
    for flags in builds() {
        let case = Case::new();
        assert!(case.compile(&source, &flags).status.success(), "{flags:?}");
        let output = Command::new(case.path(&executable_name("program")))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{flags:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&native_output("1\n"))
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("integer overflow"));
    }
}

#[test]
fn printing_a_record_is_still_refused() {
    let case = Case::new();
    let compiled = case.compile(&program("show ali"), &["-O0"]);
    assert!(!compiled.status.success());
    assert!(
        String::from_utf8_lossy(&compiled.stderr)
            .contains("Records do not support printing a record yet")
    );
    assert!(!case.path(&executable_name("program")).exists());
}

#[test]
fn invalid_record_operations_are_reported_precisely() {
    for (body, message) in [
        (
            "reassign ali.nope as 1",
            "Record 'Student' has no field 'nope'",
        ),
        (
            "reassign ali.age as \"x\"",
            "Field 'age' of record 'Student' expects int, found string",
        ),
        (
            "reassign ali.address as ali",
            "Field 'address' of record 'Student' expects Address, found Student",
        ),
        (
            "assign t as ali\nreassign t as 5",
            "Type mismatch: expected Student, found int",
        ),
        (
            "show ali.name.length",
            "Cannot read field 'length' of a value of type string",
        ),
    ] {
        let case = Case::new();
        let compiled = case.compile(&program(body), &["-O0"]);
        assert!(!compiled.status.success(), "{body}");
        let stderr = String::from_utf8_lossy(&compiled.stderr);
        assert!(stderr.contains(message), "{message}\n{stderr}");
        assert!(!case.path(&executable_name("program")).exists());
    }
    // Bad signatures.
    for (source, message) in [
        (
            "function f(p: Missing) be int {\n    return 1\n}\nshow 1",
            "Unknown record 'Missing'",
        ),
        (
            "function f() be Missing {\n    return 1\n}\nshow 1",
            "Unknown record 'Missing'",
        ),
        (
            "record P {\n    x: int\n}\nfunction f(p: P) be int {\n    return p\n}\nshow 1",
            "Type mismatch: expected int, found P",
        ),
        (
            "record P {\n    x: int\n}\nrecord Q {\n    x: int\n}\nfunction f(p: P) be Q {\n    return p\n}\nshow 1",
            "Type mismatch: expected Q, found P",
        ),
        (
            "record P {\n    x: int\n}\nfunction f(p: P) be int {\n    return p.x\n}\nshow f(1)",
            "invalid argument count or types",
        ),
    ] {
        let case = Case::new();
        let compiled = case.compile(source, &["-O0"]);
        assert!(!compiled.status.success(), "{source}");
        let stderr = String::from_utf8_lossy(&compiled.stderr);
        assert!(stderr.contains(message), "{message}\n{stderr}");
    }
}

// ----------------------------------------------------------------------------
// The sanitizer harness itself
// ----------------------------------------------------------------------------

/// A program that owns several strings through nested records.
const OWNING: &str = "record In {\n    s: string\n}\nrecord Out {\n    t: string\n    inner: In\n}\nassign a as Out {\n    t: \"x\" + \"y\"\n    inner: In {\n        s: \"p\" + \"q\"\n    }\n}\nassign b as a\nreassign b.inner.s as \"z\" + \"w\"\nshow a.inner.s\nshow b.inner.s\n";

#[test]
fn the_sanitizer_run_accepts_the_correct_program() {
    let Some(clang) = sanitizer() else {
        return;
    };
    let case = Case::new();
    let ir = emit_ir(&case, OWNING, &["-O0"]);
    let output = run_instrumented(&case, clang, &instrumented(&ir));
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "pq\nzw\n");
}

#[test]
fn the_sanitizer_run_detects_a_leak() {
    let Some(clang) = sanitizer() else {
        return;
    };
    let case = Case::new();
    let ir = emit_ir(&case, OWNING, &["-O0"]);
    // Drop every release: a correct program has become one that never frees.
    let frees = ir
        .lines()
        .filter(|l| l.contains("call void @free("))
        .count();
    assert!(frees >= 4, "the program should free its strings: {frees}");
    let leaky: String = ir
        .lines()
        .filter(|line| !line.contains("call void @free("))
        .collect::<Vec<_>>()
        .join("\n");
    let output = run_instrumented(&case, clang, &instrumented(&leaky));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "a leak must fail the run");
    assert!(stderr.contains("LeakSanitizer"), "{stderr}");
}

#[test]
fn the_sanitizer_run_detects_a_double_free() {
    let Some(clang) = sanitizer() else {
        return;
    };
    let case = Case::new();
    let ir = emit_ir(&case, OWNING, &["-O0"]);
    // Release every allocation twice.
    let doubled: String = ir
        .lines()
        .flat_map(|line| {
            let repeat = line.contains("call void @free(");
            std::iter::once(line).chain(repeat.then_some(line))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let output = run_instrumented(&case, clang, &instrumented(&doubled));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "a double free must fail the run");
    assert!(stderr.contains("double-free"), "{stderr}");
}

#[test]
fn the_sanitizer_run_detects_a_use_after_free() {
    let Some(clang) = sanitizer() else {
        return;
    };
    // The attribute is what makes LLVM check memory accesses at all.
    let ir = "declare ptr @malloc(i64)\ndeclare void @free(ptr)\ndefine i32 @main() {\n  %p = call ptr @malloc(i64 8)\n  call void @free(ptr %p)\n  %v = load i8, ptr %p\n  %r = zext i8 %v to i32\n  ret i32 %r\n}\n";
    let case = Case::new();
    let output = run_instrumented(&case, clang, &instrumented(ir));
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("heap-use-after-free"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
