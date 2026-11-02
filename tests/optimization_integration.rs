mod support;
use chuchu::codegen::generate_program;
use chuchu::lexer::tokenize;
use chuchu::optimization::{
    BuildOptions, Optimization, OptimizationError, optimize, target_machine,
};
use chuchu::parser::Parser;
use inkwell::context::Context;

#[test]
fn standard_pipelines_verify_and_promote_locals() {
    for level in [
        Optimization::O0,
        Optimization::O1,
        Optimization::O2,
        Optimization::O3,
    ] {
        let context = Context::create();
        let statements =
            Parser::new(tokenize("assign x as 10\nreassign x as x + 20\nshow x").unwrap())
                .parse_program()
                .unwrap();
        let module = generate_program(&context, &statements).unwrap();
        let options = BuildOptions {
            optimization: level,
            ..BuildOptions::default()
        };
        let machine = target_machine(&module, &options).unwrap();
        let before = module.print_to_string().to_string();
        assert!(before.contains("alloca"));
        optimize(&module, &machine, &options).unwrap();
        module.verify().unwrap();
        let after = module.print_to_string().to_string();
        if level == Optimization::O0 {
            assert_eq!(before, after);
        } else {
            assert!(!after.contains("alloca"));
        }
    }
}

#[test]
fn invalid_modules_produce_structured_verification_errors() {
    let context = Context::create();
    let module = context.create_module("invalid");
    let function = module.add_function("main", context.i32_type().fn_type(&[], false), None);
    context.append_basic_block(function, "unterminated");
    let options = BuildOptions::default();
    let machine = target_machine(&module, &options).unwrap();
    assert!(matches!(
        optimize(&module, &machine, &options),
        Err(OptimizationError::Verification(_))
    ));
}

mod native {
    use super::support::{executable_name, native_output};
    use std::fs;
    use std::path::PathBuf;
    use std::process::{Command, Output};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            for _ in 0..100 {
                let path = std::env::temp_dir().join(format!(
                    "cheeku optimization {} {}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("{error}"),
                }
            }
            panic!("Cannot allocate test directory");
        }
        fn command(&self) -> Command {
            let mut command = Command::new(env!("CARGO_BIN_EXE_cheeku"));
            command.current_dir(&self.0);
            command
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn signature(output: &Output) -> (Option<i32>, &[u8], &[u8]) {
        (output.status.code(), &output.stdout, &output.stderr)
    }

    #[test]
    fn optimization_matrix_preserves_exact_observable_behavior() {
        let cases = [
            (
                "//TODO: compiler ignores annotations\nassign/* name */ x as 12 //NOTE: initial value\n/* block\ncomment */reassign x as x / /* divisor */ 3\nshow x\nshow \"// and /* text */\"",
                Some("4\n// and /* text */\n"),
            ),
            ("show 10 + 20 * 2\nshow -7 / 2", Some("50\n-3\n")),
            (
                "assign x as 1\nif true { reassign x as 2 } else { reassign x as 3 }\nshow x",
                Some("2\n"),
            ),
            (
                "assign i as 0\nassign total as 0\nwhile i < 3 {\nassign j as 0\nwhile j < 2 {\nreassign total as total + i * j\nreassign j as j + 1\n}\nreassign i as i + 1\n}\nshow total",
                Some("3\n"),
            ),
            (
                "function f(x: int) be int { return x * x }\nshow f(f(3))",
                Some("81\n"),
            ),
            (
                "function f(n: int) be int {\nif n <= 1 { return 1 } else { return n * f(n - 1) }\n}\nshow f(6)",
                Some("720\n"),
            ),
            (
                "function even(n: int) be bool {\nif n == 0 { return true } else { return odd(n - 1) }\n}\nfunction odd(n: int) be bool {\nif n == 0 { return false } else { return even(n - 1) }\n}\nshow even(4)",
                Some("true\n"),
            ),
            (
                "assign a as [1, 2, 3]\nassign b as a\nreassign a as [4, 5, 6]\nshow b[0]\nshow sum(a * b)\nshow length(a)",
                Some("1\n32\n3\n"),
            ),
            (
                "show 0.1 + 0.2\nshow -0.0\nshow sqrt(-1.0) != sqrt(-1.0)\nshow min(sqrt(-1.0), 2.0)",
                Some("0.3\n-0\ntrue\n2\n"),
            ),
            ("show -9223372036854775808", Some("-9223372036854775808\n")),
            ("", Some("")),
            ("show 9223372036854775807 + 1", None),
            ("show -9223372036854775808 - 1", None),
            ("show 9223372036854775807 * 2", None),
            ("assign x as -9223372036854775808\nshow -x", None),
            ("show -9223372036854775808 / -1", None),
            ("show abs(-9223372036854775808)", None),
            ("assign x as 0\nshow 1 / x", None),
            ("show 1.0 / -0.0", None),
            ("show [1, 2][-1]", None),
            ("assign i as 2\nshow [1, 2][i]", None),
            ("show sum([9223372036854775807, 1])", None),
            (
                "show sum([9223372036854775807, 1, 9223372036854775807] * [1, 1, 2])",
                None,
            ),
        ];
        let directory = Directory::new();
        for (index, (source, expected)) in cases.iter().enumerate() {
            let input = directory.0.join(format!("case {index}.chuchu"));
            fs::write(&input, source).unwrap();
            let mut baseline = None;
            for level in ["-O0", "-O1", "-O2", "-O3"] {
                let executable = directory
                    .0
                    .join(executable_name(&format!("program {level}")));
                let compiled = directory
                    .command()
                    .arg(&input)
                    .arg("-o")
                    .arg(&executable)
                    .arg(level)
                    .output()
                    .unwrap();
                assert!(
                    compiled.status.success(),
                    "{source}: {}",
                    String::from_utf8_lossy(&compiled.stderr)
                );
                assert!(compiled.stdout.is_empty());
                assert!(compiled.stderr.is_empty());
                let output = Command::new(executable).output().unwrap();
                if let Some(expected) = expected {
                    assert_eq!(
                        signature(&output),
                        (Some(0), native_output(expected).as_slice(), b"".as_slice()),
                        "{source} at {level}"
                    );
                } else {
                    assert_eq!(output.status.code(), Some(1));
                    assert!(String::from_utf8_lossy(&output.stderr).contains("cheeku runtime:"));
                }
                if let Some(previous) = &baseline {
                    assert_eq!(
                        signature(&output),
                        signature(previous),
                        "{source} at {level}"
                    );
                } else {
                    baseline = Some(output);
                }
            }
        }
    }

    #[test]
    fn cli_diagnostics_and_invalid_options_preserve_existing_files() {
        let directory = Directory::new();
        let input = directory.0.join("input.chuchu");
        fs::write(&input, "show 42").unwrap();
        let output = directory.0.join(executable_name("program"));
        fs::write(&output, b"previous").unwrap();
        for extra in [vec!["-O9"], vec!["--cpu", "unknown"], vec!["-O0", "-O3"]] {
            let result = directory
                .command()
                .arg(&input)
                .arg("-o")
                .arg(&output)
                .args(extra)
                .output()
                .unwrap();
            assert!(!result.status.success());
            assert_eq!(fs::read(&output).unwrap(), b"previous");
        }
        let result = directory
            .command()
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .arg("--emit-ir")
            .arg(&input)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert_eq!(fs::read_to_string(&input).unwrap(), "show 42");
        let result = directory
            .command()
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .arg("--emit-ir")
            .arg(&output)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert_eq!(fs::read(&output).unwrap(), b"previous");

        let ir = directory.0.join("output.ll");
        let assembly = directory.0.join("output.s");
        let result = directory
            .command()
            .arg(&input)
            .arg("-O3")
            .arg("-o")
            .arg(&output)
            .arg("--emit-ir")
            .arg(&ir)
            .arg("--emit-assembly")
            .arg(&assembly)
            .arg("--time-compilation")
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(result.stdout.is_empty());
        let timing = String::from_utf8(result.stderr).unwrap();
        for field in [
            "lex_s",
            "parse_s",
            "type_s",
            "ir_s",
            "optimization_s",
            "object_s",
            "link_s",
            "total_s",
        ] {
            assert!(timing.contains(field));
        }
        assert!(fs::read_to_string(ir).unwrap().contains("@main"));
        assert!(!fs::read(assembly).unwrap().is_empty());
        assert_eq!(
            Command::new(output).output().unwrap().stdout,
            native_output("42\n").as_slice()
        );
    }
}
