use chuchu::modules::ModuleGraph;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Project(PathBuf);
impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "chuchu-module-graph-{}-{}",
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
    fn load(&self) -> Result<ModuleGraph, chuchu::compiler::CompileError> {
        ModuleGraph::load(&self.0.join("main.chuchu"))
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn relative_imports_and_diamonds_have_stable_unique_identity() {
    let project = Project::new(&[
        (
            "main.chuchu",
            "import \"./a.chuchu\" as a\nimport \"lib/b.chuchu\" as b",
        ),
        (
            "a.chuchu",
            "module a\nimport \"lib/shared.chuchu\" as shared",
        ),
        (
            "lib/b.chuchu",
            "module b\nimport \"../lib/shared.chuchu\" as shared",
        ),
        ("lib/shared.chuchu", "module shared"),
    ]);
    let graph = project.load().unwrap();
    assert_eq!(graph.files.len(), 4);
    assert_eq!(graph.files[0].dependencies, [1, 3]);
    assert_eq!(graph.files[1].dependencies, [2]);
    assert_eq!(graph.files[3].dependencies, [2]);
    let sources = graph.sources();
    for file in &graph.files {
        assert_eq!(
            sources.origin(file.offset).unwrap().0.path(),
            file.source.path()
        );
    }
}

#[test]
fn invalid_module_graphs_have_source_diagnostics() {
    for (files, expected) in [
        (
            vec![("main.chuchu", "import \"missing.chuchu\" as missing")],
            "Cannot load imported module",
        ),
        (
            vec![("main.chuchu", "import \"main.chuchu\" as main")],
            "Circular import",
        ),
        (
            vec![
                ("main.chuchu", "import \"a.chuchu\" as a"),
                ("a.chuchu", "module a\nimport \"b.chuchu\" as b"),
                ("b.chuchu", "module b\nimport \"a.chuchu\" as a"),
            ],
            "Circular import",
        ),
        (
            vec![
                ("main.chuchu", "import \"a.chuchu\" as a"),
                ("a.chuchu", "show 1"),
            ],
            "must start",
        ),
        (
            vec![
                ("main.chuchu", "import \"a.chuchu\" as a"),
                ("a.chuchu", "module a\nshow 1"),
            ],
            "executable statements",
        ),
        (
            vec![
                ("main.chuchu", "import \"a.chuchu\" as a"),
                ("a.chuchu", "module a\nshow @"),
            ],
            "Invalid source character",
        ),
        (vec![("main.chuchu", "import \"\" as a")], "relative paths"),
        (
            vec![("main.chuchu", "import \"/a.chuchu\" as a")],
            "relative paths",
        ),
        (
            vec![("main.chuchu", "import \"a.txt\" as a")],
            ".chuchu file",
        ),
    ] {
        let project = Project::new(&files);
        let error = project.load().unwrap_err();
        let rendered =
            chuchu::diagnostic::render(&error, &project.0.join("main.chuchu"), files[0].1);
        assert!(rendered.contains(expected), "{rendered}");
        assert!(rendered.contains(" --> "), "{rendered}");
    }
}

#[test]
fn resolves_private_helpers_and_separate_namespaces() {
    let project = Project::new(&[
        (
            "main.chuchu",
            "import \"a.chuchu\" as a\nimport \"b.chuchu\" as b\nshow a::value() + b::value()",
        ),
        (
            "a.chuchu",
            "module shared\nfunction helper() be int {\nreturn 1\n}\nexport function value() be int {\nreturn helper()\n}",
        ),
        (
            "b.chuchu",
            "module shared\nexport function value() be int {\nreturn 2\n}",
        ),
    ]);
    let statements = project.load().unwrap().resolve().unwrap();
    let mut checker =
        chuchu::type_checker::TypeChecker::new(chuchu::symbol_table::SymbolTable::new());
    checker.check_program(&statements).unwrap();
    let names = statements
        .iter()
        .filter_map(|statement| match &statement.kind {
            chuchu::ast::StmtKind::Function { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(names.len(), 3);
}

#[test]
fn rejects_visibility_and_binding_errors() {
    for (main, library, expected) in [
        (
            "show a::hidden()",
            "function hidden() be int {\nreturn 1\n}",
            "is private",
        ),
        ("show a::missing()", "", "Unknown function"),
        ("show b::missing()", "", "Unknown module alias"),
        (
            "show a::Point()",
            "export record Point {\nx: int\n}",
            "not a function",
        ),
        ("assign a as 1", "", "Duplicate name"),
        ("function a() be int {\nreturn 1\n}", "", "Duplicate name"),
        ("import \"a.chuchu\" as a", "", "Duplicate module alias"),
        (
            "",
            "function f() be int {\nreturn 1\n}\nfunction f() be int {\nreturn 2\n}",
            "Duplicate name",
        ),
        (
            "",
            "record Hidden {\nx: int\n}\nexport function leak(p: Hidden) be int {\nreturn p.x\n}",
            "exposes private record",
        ),
        (
            "",
            "record Hidden {\nx: int\n}\nexport record Public {\np: Hidden\n}",
            "exposes private record",
        ),
        (
            "",
            "function show_private() be int {\nreturn missing::f()\n}",
            "Unknown module alias",
        ),
    ] {
        let main = format!("import \"a.chuchu\" as a\n{main}");
        let library = format!("module a\n{library}");
        let project = Project::new(&[("main.chuchu", &main), ("a.chuchu", &library)]);
        let error = project.load().unwrap().resolve().unwrap_err();
        let rendered = chuchu::diagnostic::render(&error, &project.0.join("main.chuchu"), &main);
        assert!(rendered.contains(expected), "{rendered}");
    }
}
