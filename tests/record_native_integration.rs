//! Native execution of records whose fields are `int`, `float`, or `bool`
//! (Issue #11, step 7B): initialization, field reading, and the guarantee that
//! every record operation that is not supported yet fails safely at compile time.
//! Programs run through the real `cheeku` binary at -O0 through -O3, with and
//! without debug information, and must behave identically everywhere.
mod support;

use chuchu::codegen::{CodegenError, generate_program_with_records};
use chuchu::lexer::tokenize;
use chuchu::optimization::{BuildOptions, DebugInfo, Optimization};
use chuchu::parser::Parser;
use chuchu::source::SourceFile;
use chuchu::symbol_table::SymbolTable;
use chuchu::type_checker::TypeChecker;
use inkwell::context::Context;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
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
                "cheeku record native {} {}",
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
    fn input(&self, source: &str) -> PathBuf {
        let input = self.directory.join("case.chuchu");
        fs::write(&input, source).unwrap();
        input
    }
    fn executable(&self) -> PathBuf {
        self.directory.join(executable_name("program"))
    }
    fn compile(&self, source: &str, flags: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cheeku"))
            .arg(self.input(source))
            .arg("-o")
            .arg(self.executable())
            .args(flags)
            .current_dir(&self.directory)
            .output()
            .unwrap()
    }
    fn run(&self) -> Output {
        Command::new(self.executable()).output().unwrap()
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

/// Compiles and runs `source` in every build and requires exactly `expected` on
/// standard output, nothing on standard error, and a successful exit.
fn prints(source: &str, expected: &str) {
    for flags in builds() {
        let case = Case::new();
        let compiled = case.compile(source, &flags);
        assert!(
            compiled.status.success(),
            "{flags:?}\n{source}\n{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        assert!(compiled.stdout.is_empty(), "{flags:?}");
        let output = case.run();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&native_output(expected)),
            "{flags:?}\n{source}"
        );
        assert!(output.stderr.is_empty(), "{flags:?}");
        assert_eq!(output.status.code(), Some(0), "{flags:?}");
    }
}

/// Compiles and runs `source` in every build; it must stop with a runtime error
/// (exit status 1 and a `cheeku runtime:` message) after printing `before`.
fn fails_at_runtime(source: &str, before: &str, message: &str) {
    for flags in builds() {
        let case = Case::new();
        let compiled = case.compile(source, &flags);
        assert!(
            compiled.status.success(),
            "{flags:?}\n{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = case.run();
        assert_eq!(output.status.code(), Some(1), "{flags:?}\n{source}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&native_output(before)),
            "{flags:?}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cheeku runtime:") && stderr.contains(message),
            "{flags:?}: {stderr}"
        );
    }
}

#[test]
fn the_issue_example_prints_its_expected_output() {
    prints(
        "record Student {\n    age: int\n    active: bool\n}\n\nassign student as Student {\n    age: 22\n    active: true\n}\n\nshow student.age\nshow student.active\n",
        "22\ntrue\n",
    );
}

#[test]
fn every_supported_field_type_round_trips() {
    prints(
        "record Everything {\n    big: int\n    small: int\n    ratio: float\n    negative: float\n    on: bool\n    off: bool\n}\nassign e as Everything {\n    big: 9223372036854775807\n    small: -9223372036854775808\n    ratio: 0.1\n    negative: -2.5\n    on: true\n    off: false\n}\nshow e.big\nshow e.small\nshow e.ratio\nshow e.negative\nshow e.on\nshow e.off",
        "9223372036854775807\n-9223372036854775808\n0.1\n-2.5\ntrue\nfalse\n",
    );
}

#[test]
fn field_declaration_order_is_kept_whatever_order_the_initializer_uses() {
    let record =
        "record Person {\n    id: int\n    score: float\n    active: bool\n    level: int\n}\n";
    let shown = |name: &str| {
        format!("show {name}.id\nshow {name}.score\nshow {name}.active\nshow {name}.level\n")
    };
    let source = format!(
        "{record}assign a as Person {{\n    id: 1\n    score: 2.5\n    active: true\n    level: 4\n}}\nassign b as Person {{\n    level: 4\n    active: true\n    score: 2.5\n    id: 1\n}}\nassign c as Person {{\n    active: false\n    id: 9\n    level: 8\n    score: 0.5\n}}\n{}{}{}",
        shown("a"),
        shown("b"),
        shown("c")
    );
    prints(
        &source,
        "1\n2.5\ntrue\n4\n1\n2.5\ntrue\n4\n9\n0.5\nfalse\n8\n",
    );
}

#[test]
fn field_values_are_evaluated_as_written_and_stored_as_declared() {
    // `noisy` prints, so the order its calls run in is visible.
    prints(
        "function noisy(n: int) be int {\n    show n\n    return n\n}\nrecord Pair {\n    first: int\n    second: int\n}\nassign p as Pair {\n    second: noisy(2)\n    first: noisy(1)\n}\nshow p.first\nshow p.second",
        "2\n1\n1\n2\n",
    );
    // The same order for three fields and for values that read other variables.
    prints(
        "function noisy(n: int) be int {\n    show n\n    return n\n}\nrecord Triple {\n    a: int\n    b: int\n    c: int\n}\nassign base as 10\nassign t as Triple {\n    c: noisy(base + 3)\n    a: noisy(base + 1)\n    b: noisy(base + 2)\n}\nshow t.a\nshow t.b\nshow t.c",
        "13\n11\n12\n11\n12\n13\n",
    );
}

#[test]
fn an_int_value_widens_to_a_float_field() {
    prints(
        "record Measure {\n    value: float\n    limit: float\n}\nassign n as 7\nassign m as Measure {\n    value: 3\n    limit: n\n}\nshow m.value / 2.0\nshow m.limit / 2.0\nshow m.value + m.limit\nassign neg as Measure {\n    value: -4\n    limit: 7 / 2\n}\nshow neg.value\nshow neg.limit",
        "1.5\n3.5\n10\n-4\n3\n",
    );
}

#[test]
fn temporary_records_can_be_read_directly() {
    prints(
        "record Point {\n    x: int\n    y: int\n}\nshow Point {\n    x: 3\n    y: 4\n}.x\nshow Point {\n    y: 4\n    x: 3\n}.y\nshow Point {\n    x: 10\n    y: 20\n}.x + Point {\n    x: 1\n    y: 2\n}.y\nif (Point {\n    x: 1\n    y: 2\n}).y == 2 {\n    show 100\n}\nassign n as 5\nshow Point {\n    x: n * 2\n    y: n\n}.x",
        "3\n4\n12\n100\n10\n",
    );
}

#[test]
fn records_work_in_loops_functions_and_blocks() {
    // A new record each iteration of a loop.
    prints(
        "record Counter {\n    n: int\n    even: bool\n}\nassign i as 0\nassign total as 0\nwhile i < 5 {\n    assign c as Counter {\n        n: i * i\n        even: i / 2 * 2 == i\n    }\n    reassign total as total + c.n\n    if c.even {\n        show c.n\n    }\n    reassign i as i + 1\n}\nshow total",
        "0\n4\n16\n30\n",
    );
    // A record created inside a function.
    prints(
        "record Box {\n    w: int\n    h: int\n}\nfunction area(w: int, h: int) be int {\n    assign b as Box {\n        w: w\n        h: h\n    }\n    return b.w * b.h\n}\nshow area(3, 4)\nshow area(5, 6)",
        "12\n30\n",
    );
    // An inner block can shadow a record variable without touching the outer one.
    prints(
        "record Box {\n    w: int\n    h: int\n}\nassign p as Box {\n    w: 1\n    h: 2\n}\nif true {\n    assign p as Box {\n        w: 10\n        h: 20\n    }\n    show p.w\n}\nshow p.w",
        "10\n1\n",
    );
    // Recursion, each call with its own record.
    prints(
        "record Step {\n    n: int\n    acc: int\n}\nfunction countdown(n: int) be int {\n    assign s as Step {\n        n: n\n        acc: n\n    }\n    if s.n <= 1 {\n        return s.acc\n    }\n    return s.acc + countdown(s.n - 1)\n}\nshow countdown(10)",
        "55\n",
    );
}

#[test]
fn several_records_can_share_field_names() {
    prints(
        "record A {\n    v: int\n}\nrecord B {\n    v: float\n    w: bool\n}\nassign a as A {\n    v: 7\n}\nassign b as B {\n    w: true\n    v: 7\n}\nshow a.v\nshow b.v\nshow b.v / 2.0\nshow b.w",
        "7\n7\n3.5\ntrue\n",
    );
    // Records declared after the code that uses them, and between functions.
    prints(
        "function f() be int {\n    return 1\n}\nassign p as P {\n    x: 4\n}\nshow p.x\nrecord P {\n    x: int\n}\nshow f()",
        "4\n1\n",
    );
}

#[test]
fn many_mixed_fields_keep_their_values_despite_padding() {
    // Bools between wider fields exercise the target's padding rules.
    let kinds = [
        "bool", "int", "bool", "float", "bool", "int", "float", "bool", "int", "bool", "float",
        "int",
    ];
    let mut record = String::from("record Wide {\n");
    let mut initializer = String::from("assign w as Wide {\n");
    let mut shown = String::new();
    let mut expected = String::new();
    for (index, kind) in kinds.iter().enumerate() {
        record += &format!("    f{index}: {kind}\n");
        let (literal, text) = match *kind {
            "bool" => {
                let value = index % 4 == 0;
                (value.to_string(), value.to_string())
            }
            "int" => (
                format!("{}", index * 1000 + 7),
                format!("{}", index * 1000 + 7),
            ),
            _ => (format!("{index}.25"), format!("{index}.25")),
        };
        initializer += &format!("    f{index}: {literal}\n");
        shown += &format!("show w.f{index}\n");
        expected += &format!("{text}\n");
    }
    prints(&format!("{record}}}\n{initializer}}}\n{shown}"), &expected);
}

#[test]
fn field_reads_take_part_in_expressions_and_control_flow() {
    prints(
        "record Config {\n    limit: int\n    scale: float\n    verbose: bool\n}\nassign c as Config {\n    limit: 3\n    scale: 1.5\n    verbose: true\n}\nassign i as 0\nwhile i < c.limit {\n    show i * c.limit\n    reassign i as i + 1\n}\nshow c.scale * 2.0\nif c.verbose {\n    show 1\n} else {\n    show 0\n}\nshow c.limit == 3\nshow c.verbose == false\nassign xs as [10, 20, 30]\nshow xs[c.limit - 1]\nfunction twice(n: int) be int {\n    return n * 2\n}\nshow twice(c.limit)\nassign copy as c.limit + 1\nreassign copy as copy * c.limit\nshow copy",
        "0\n3\n6\n3\n1\ntrue\nfalse\n30\n6\n12\n",
    );
}

#[test]
fn runtime_checks_still_apply_to_values_read_from_records() {
    let record = "record R {\n    big: int\n    zero: int\n    i: int\n}\nassign r as R {\n    big: 9223372036854775807\n    zero: 0\n    i: 5\n}\nshow 1\n";
    fails_at_runtime(
        &format!("{record}show r.big + 1"),
        "1\n",
        "integer overflow",
    );
    fails_at_runtime(
        &format!("{record}show 10 / r.zero"),
        "1\n",
        "division by zero",
    );
    fails_at_runtime(
        &format!("{record}show [1, 2, 3][r.i]"),
        "1\n",
        "array index out of bounds",
    );
}

#[test]
fn records_do_not_change_programs_that_do_not_use_them() {
    // A program that declares records it never initializes behaves as without them.
    prints(
        "record Unused {\n    a: int\n    b: bool\n}\nassign x as 10\nassign y as 20\nshow x + y\nreassign x as 30\nshow x + y",
        "30\n50\n",
    );
}

#[test]
fn the_generated_llvm_represents_a_record_as_a_struct_in_declaration_order() {
    let case = Case::new();
    let ir = case.directory.join("out.ll");
    let source = "record Sample {\n    count: int\n    ratio: float\n    ready: bool\n}\nassign n as 5\nassign s as Sample {\n    ready: n > 2\n    count: n\n    ratio: 1.5\n}\nshow s.count\nshow s.ratio\nshow s.ready\n";
    let compiled = case.compile(source, &["-O0", "--emit-ir", ir.to_str().unwrap()]);
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ir = fs::read_to_string(&ir).unwrap();
    assert!(
        ir.contains("%chuchu.record.Sample = type { i64, double, i1 }"),
        "{ir}"
    );
    // The record lives in one local of the struct type...
    assert!(ir.contains("alloca %chuchu.record.Sample"), "{ir}");
    // ...is built field by field in declaration order, whatever the order written...
    let inserted: Vec<u32> = ir
        .lines()
        .filter(|line| line.contains("insertvalue %chuchu.record.Sample"))
        .map(|line| line.rsplit(", ").next().unwrap().trim().parse().unwrap())
        .collect();
    assert_eq!(inserted, [0, 1, 2], "{ir}");
    // ...and each read extracts the element at the field's index.
    let extracted: Vec<u32> = ir
        .lines()
        .filter(|line| line.contains("extractvalue %chuchu.record.Sample"))
        .map(|line| line.rsplit(", ").next().unwrap().trim().parse().unwrap())
        .collect();
    assert_eq!(extracted, [0, 1, 2], "{ir}");
    // The compiled program agrees.
    let output = case.run();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&native_output("5\n1.5\ntrue\n"))
    );
}

#[test]
fn the_optimizer_removes_a_record_that_only_holds_constants() {
    let case = Case::new();
    let ir = case.directory.join("out.ll");
    let source = "record Student {\n    age: int\n    active: bool\n}\nassign student as Student {\n    age: 22\n    active: true\n}\nshow student.age\nshow student.active\n";
    let compiled = case.compile(source, &["-O2", "--emit-ir", ir.to_str().unwrap()]);
    assert!(compiled.status.success());
    let ir = fs::read_to_string(&ir).unwrap();
    let main = &ir[ir.find("define").unwrap()..];
    assert!(!main.contains("alloca"), "{main}");
    assert!(!main.contains("insertvalue"), "{main}");
    assert!(!main.contains("extractvalue"), "{main}");
}

#[test]
fn debug_information_describes_a_record_with_its_fields() {
    let source = "record Sample {\n    count: int\n    ratio: float\n    ready: bool\n}\nassign s as Sample {\n    count: 1\n    ratio: 2.5\n    ready: true\n}\nshow s.count\n";
    let statements = Parser::new(tokenize(source).unwrap())
        .parse_program()
        .unwrap();
    let mut checker = TypeChecker::new(SymbolTable::new());
    checker.check_program(&statements).unwrap();
    let table = checker.record_table().unwrap();
    let file = SourceFile::new(
        std::env::temp_dir().join("record debug.chuchu"),
        source.into(),
    );
    for optimization in [
        Optimization::O0,
        Optimization::O1,
        Optimization::O2,
        Optimization::O3,
    ] {
        let options = BuildOptions {
            debug_info: DebugInfo::Full,
            optimization,
            ..BuildOptions::default()
        };
        let context = Context::create();
        let module =
            generate_program_with_records(&context, &statements, &table, Some(&file), &options)
                .unwrap();
        module.verify().unwrap();
        let ir = module.print_to_string().to_string();
        for fragment in [
            "DICompositeType(tag: DW_TAG_structure_type, name: \"Sample\"",
            "DIDerivedType(tag: DW_TAG_member, name: \"count\"",
            "DIDerivedType(tag: DW_TAG_member, name: \"ratio\"",
            "DIDerivedType(tag: DW_TAG_member, name: \"ready\"",
            "DILocalVariable(name: \"s\"",
        ] {
            assert!(ir.contains(fragment), "{optimization:?}: {fragment}\n{ir}");
        }
        // The second field follows the 8-byte integer.
        assert!(ir.contains("offset: 64"), "{optimization:?}");
    }
}

#[test]
fn unsupported_record_operations_fail_to_compile_and_leave_no_executable() {
    let record =
        "record P {\n    x: int\n    f: float\n}\nassign s as P {\n    x: 1\n    f: 2.5\n}\n";
    for (body, message) in [
        // Operations that are not supported yet.
        (
            "assign t as s\n",
            "Records do not support copying a record yet",
        ),
        (
            "reassign s as P {\n    x: 2\n    f: 1.0\n}\n",
            "Records do not support reassigning a record variable yet",
        ),
        (
            "reassign s.x as 5\n",
            "Records do not support assigning a record field yet",
        ),
        ("show s\n", "Records do not support printing a record yet"),
        (
            "show P {\n    x: 1\n    f: 1.0\n}\n",
            "Records do not support printing a record yet",
        ),
    ] {
        refuses(&format!("{record}{body}"), message);
    }
    // Fields that are not int, float, or bool, even when the record is never used.
    refuses(
        "record Named {\n    name: string\n    n: int\n}\nshow 1\n",
        "Record 'Named' cannot be compiled yet",
    );
    refuses(
        "record Inner {\n    n: int\n}\nrecord Outer {\n    inner: Inner\n}\nshow 1\n",
        "Record 'Outer' cannot be compiled yet",
    );
    refuses(
        "record Named {\n    name: string\n}\nassign n as Named {\n    name: \"x\"\n}\nshow 1\n",
        "Record 'Named' cannot be compiled yet",
    );
    // Real errors are still reported as themselves.
    refuses(
        &format!("{record}show s.missing\n"),
        "Record 'P' has no field 'missing'",
    );
    refuses(
        &format!("{record}assign t as P {{\n    x: \"no\"\n    f: 1.0\n}}\n"),
        "Field 'x' of record 'P' expects int, found string",
    );
}

/// A failed compile reports `message` and neither creates nor replaces the output.
fn refuses(source: &str, message: &str) {
    for flags in [vec!["-O0"], vec!["-O2", "-g"]] {
        let case = Case::new();
        let compiled = case.compile(source, &flags);
        assert!(!compiled.status.success(), "{source}");
        assert!(compiled.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&compiled.stderr);
        assert!(stderr.contains(message), "{message}\n{stderr}");
        assert!(!case.executable().exists(), "{source}");
        // An executable that was already there is left alone.
        fs::write(case.executable(), b"previous").unwrap();
        assert!(!case.compile(source, &flags).status.success());
        assert_eq!(fs::read(case.executable()).unwrap(), b"previous");
    }
}

/// Parses `declarations` and `rest`, checks only the declarations, and lowers the
/// statements without the checker, to show lowering rejects malformed record code
/// by itself.
fn lower_unchecked(declarations: &str, rest: &str) -> Result<(), CodegenError> {
    let mut checker = TypeChecker::new(SymbolTable::new());
    let declared = Parser::new(tokenize(declarations).unwrap())
        .parse_program()
        .unwrap();
    checker.check_program(&declared).unwrap();
    let table = checker.record_table().unwrap();
    let statements = Parser::new(tokenize(&format!("{declarations}{rest}")).unwrap())
        .parse_program()
        .unwrap();
    let context = Context::create();
    generate_program_with_records(
        &context,
        &statements,
        &table,
        None,
        &BuildOptions::default(),
    )
    .map(|_| ())
}

#[test]
fn lowering_refuses_malformed_record_code_even_without_the_checker() {
    let declarations = "record P {\n    x: int\n    f: float\n}\n";
    for (rest, message) in [
        // A field left out, given twice, or not in the record.
        (
            "assign s as P {\n    x: 1\n}",
            "Record field is not initialized",
        ),
        (
            "assign s as P {\n    x: 1\n    x: 2\n    f: 1.0\n}",
            "Record field is initialized twice",
        ),
        (
            "assign s as P {\n    x: 1\n    f: 1.0\n    z: 3\n}",
            "Record has no such field",
        ),
        // An unknown record, and a field read of something that is not a record.
        ("assign s as Q {\n    x: 1\n}", "Record type has no layout"),
        (
            "assign s as P {\n    x: 1\n    f: 1.0\n}\nshow s.z",
            "Record has no such field",
        ),
        ("assign n as 1\nshow n.x", "Field access requires a record"),
        // A value that does not fit the field.
        (
            "assign s as P {\n    x: 1.5\n    f: 1.0\n}",
            "Incompatible value type",
        ),
        (
            "assign s as P {\n    x: true\n    f: 1.0\n}",
            "Incompatible value type",
        ),
    ] {
        let result = lower_unchecked(declarations, rest);
        assert!(
            matches!(&result, Err(CodegenError::InvalidType { message: found, .. })
                if *found == message),
            "{rest:?}: {result:?}"
        );
    }
    // The same record, well formed, lowers.
    lower_unchecked(
        declarations,
        "assign s as P {\n    f: 1.0\n    x: 1\n}\nshow s.x",
    )
    .unwrap();
}
