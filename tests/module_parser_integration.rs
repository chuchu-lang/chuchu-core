use chuchu::ast::{ExprKind, FieldType, StmtKind};
use chuchu::{lexer::tokenize, parser::Parser};

#[test]
fn parses_exports_imports_and_qualified_types_and_calls() {
    let source = "module app\nimport \"./math.chuchu\" as math\nexport function point(p: math::Point) be math::Point {\nreturn p\n}\nshow math::add(2, 3)\nassign p as math::Point {\nx: 0\ny: 0\n}\n";
    let module = Parser::new(tokenize(source).unwrap())
        .parse_module()
        .unwrap();
    assert_eq!(module.name.unwrap().0, "app");
    assert_eq!(module.imports[0].alias, "math");
    assert_eq!(module.exports[0].0, "point");
    let StmtKind::Function {
        parameters,
        return_type,
        ..
    } = &module.statements[0].kind
    else {
        panic!()
    };
    assert_eq!(parameters[0].ty, FieldType::Named("math::Point".into()));
    assert_eq!(*return_type, FieldType::Named("math::Point".into()));
    let StmtKind::Show(expr) = &module.statements[1].kind else {
        panic!()
    };
    assert!(matches!(&expr.kind, ExprKind::Call { name, .. } if name == "math::add"));
    let StmtKind::Set { value, .. } = &module.statements[2].kind else {
        panic!()
    };
    assert!(matches!(&value.kind, ExprKind::RecordInit { name, .. } if name == "math::Point"));
}

#[test]
fn rejects_misplaced_directives_and_invalid_qualification() {
    for source in [
        "show 1\nimport \"x.chuchu\" as x",
        "module a\nmodule b",
        "export assign x as 1",
        "import x as x",
        "import \"x.chuchu\"",
        "show x::",
        "show x::y::z()",
        "function f() be int {\nimport \"x.chuchu\" as x\nreturn 1\n}",
    ] {
        assert!(
            Parser::new(tokenize(source).unwrap())
                .parse_module()
                .is_err(),
            "{source}"
        );
    }
}
