mod support;
use chuchu::compiler::{
    compile_file_with_options, compile_named_source_with_options, compile_source,
};
use chuchu::optimization::{BuildOptions, DebugInfo, Optimization};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Project(PathBuf);
impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "chuchu module native {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self(root)
    }
    fn executable(&self) -> PathBuf {
        self.0.join(support::executable_name("program"))
    }
    fn compile(
        &self,
        options: &BuildOptions,
    ) -> Result<chuchu::compiler::CompilationTimings, chuchu::compiler::CompileError> {
        compile_file_with_options(&self.0.join("main.chuchu"), &self.executable(), options)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn module_example_executes_with_records_and_private_helpers_at_all_levels() {
    let project = Project::new(&[]);
    for optimization in [
        Optimization::O0,
        Optimization::O1,
        Optimization::O2,
        Optimization::O3,
    ] {
        let ir = project.0.join("program.ll");
        compile_file_with_options(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/modules/main.chuchu"),
            &project.executable(),
            &BuildOptions {
                optimization,
                emit_ir: Some(ir.clone()),
                debug_info: DebugInfo::Full,
                ..BuildOptions::default()
            },
        )
        .unwrap();
        let output = Command::new(project.executable()).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, support::native_output("5\n3\n5\n"));
        let ir = std::fs::read_to_string(ir).unwrap();
        for name in ["main.chuchu", "math.chuchu", "geometry.chuchu"] {
            assert!(ir.contains(&format!("filename: \"{name}\"")), "{ir}");
        }
    }
}

#[test]
fn module_namespaces_do_not_collide_and_dependencies_resolve_from_source_directory() {
    let project = Project::new(&[
        (
            "main.chuchu",
            "import \"lib/a.chuchu\" as a\nimport \"lib/b.chuchu\" as b\nshow a::value()\nshow b::value()",
        ),
        (
            "lib/a.chuchu",
            "module same\nimport \"shared.chuchu\" as shared\nexport function value() be int {\nreturn shared::twice(2)\n}",
        ),
        (
            "lib/b.chuchu",
            "module same\nimport \"./shared.chuchu\" as shared\nexport function value() be int {\nreturn shared::twice(3)\n}",
        ),
        (
            "lib/shared.chuchu",
            "module shared\nexport function twice(n: int) be int {\nreturn n * 2\n}",
        ),
    ]);
    let compile = Command::new(env!("CARGO_BIN_EXE_cheeku"))
        .current_dir(std::env::temp_dir())
        .arg(project.0.join("main.chuchu"))
        .arg("-o")
        .arg(project.executable())
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let output = Command::new(project.executable()).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, support::native_output("4\n6\n"));
}

#[test]
fn dependency_type_errors_report_the_original_file_and_preserve_output() {
    let project = Project::new(&[
        (
            "main.chuchu",
            "import \"lib/a.chuchu\" as a\nshow a::value()",
        ),
        (
            "lib/a.chuchu",
            "module a\nexport function value() be int {\nreturn missing\n}",
        ),
    ]);
    std::fs::write(project.executable(), "previous build").unwrap();
    let error = project.compile(&BuildOptions::default()).unwrap_err();
    let rendered = chuchu::diagnostic::render(&error, &project.0.join("main.chuchu"), "");
    assert!(rendered.contains("a.chuchu:3:8"), "{rendered}");
    assert!(rendered.contains("return missing"), "{rendered}");
    assert!(rendered.contains("imported from"), "{rendered}");
    assert_eq!(
        std::fs::read_to_string(project.executable()).unwrap(),
        "previous build"
    );
}

#[test]
fn every_imported_source_is_protected_from_artifact_overwrites() {
    let library = "module a\nexport function value() be int {\nreturn 1\n}";
    let project = Project::new(&[
        ("main.chuchu", "import \"a.chuchu\" as a\nshow a::value()"),
        ("a.chuchu", library),
    ]);
    let source = project.0.join("a.chuchu");
    for options in [
        BuildOptions {
            emit_ir: Some(source.clone()),
            ..BuildOptions::default()
        },
        BuildOptions {
            emit_assembly: Some(source.clone()),
            ..BuildOptions::default()
        },
    ] {
        assert!(project.compile(&options).is_err());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), library);
    }
    assert!(
        compile_file_with_options(
            &project.0.join("main.chuchu"),
            &source,
            &BuildOptions::default()
        )
        .is_err()
    );
    assert_eq!(std::fs::read_to_string(source).unwrap(), library);
}

#[test]
fn runtime_errors_in_dependencies_name_their_source_file() {
    let project = Project::new(&[
        ("main.chuchu", "import \"a.chuchu\" as a\nshow a::divide(0)"),
        (
            "a.chuchu",
            "module a\nexport function divide(n: int) be int {\nreturn 1 / n\n}",
        ),
    ]);
    project.compile(&BuildOptions::default()).unwrap();
    let output = Command::new(project.executable()).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("a.chuchu:3:"), "{stderr}");
    assert!(stderr.contains("division"), "{stderr}");
}

#[test]
fn named_source_api_resolves_imports_and_unnamed_api_explains_the_requirement() {
    let text = "import \"a.chuchu\" as a\nshow a::value()";
    let project = Project::new(&[
        ("main.chuchu", text),
        (
            "a.chuchu",
            "module a\nexport function value() be int {\nreturn 7\n}",
        ),
    ]);
    let source = chuchu::source::SourceFile::new(project.0.join("main.chuchu"), text.into());
    compile_named_source_with_options(&source, &project.executable(), &BuildOptions::default())
        .unwrap();
    let output = Command::new(project.executable()).output().unwrap();
    assert_eq!(output.stdout, support::native_output("7\n"));
    let error = compile_source(text, &project.executable()).unwrap_err();
    assert!(
        error.to_string().contains("require a source file"),
        "{error}"
    );
}

#[test]
fn imported_record_strings_preserve_value_semantics_across_functions() {
    let project = Project::new(&[
        (
            "main.chuchu",
            "import \"people.chuchu\" as people\nassign original as people::Person {\nname: \"Ada 🌍\"\n}\nassign changed as people::rename(original)\nshow original.name\nshow changed.name",
        ),
        (
            "people.chuchu",
            "module people\nexport record Person {\nname: string\n}\nrecord Private {\nname: string\n}\nexport function rename(person: Person) be Person {\nassign helper as Private {\nname: \"Grace\"\n}\nreassign person.name as helper.name\nreturn person\n}",
        ),
    ]);
    project.compile(&BuildOptions::default()).unwrap();
    let output = Command::new(project.executable()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, support::native_output("Ada 🌍\nGrace\n"));
}

#[test]
fn dependency_parse_errors_point_to_local_eof_with_an_import_trace() {
    let project = Project::new(&[
        ("main.chuchu", "import \"a.chuchu\" as a"),
        ("a.chuchu", "module a\nimport \"b.chuchu\" as b"),
        (
            "b.chuchu",
            "module b\nexport function broken() be int {\nreturn 1",
        ),
    ]);
    let output = Command::new(env!("CARGO_BIN_EXE_cheeku"))
        .arg(project.0.join("main.chuchu"))
        .arg("-o")
        .arg(project.executable())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!project.executable().exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("b.chuchu:3:9"), "{stderr}");
    assert!(stderr.contains("end of file"), "{stderr}");
    assert!(stderr.contains("imported from"), "{stderr}");
    assert!(stderr.contains("a.chuchu"), "{stderr}");
    assert!(stderr.contains("main.chuchu"), "{stderr}");
}
