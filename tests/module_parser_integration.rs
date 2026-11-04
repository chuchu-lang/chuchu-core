use chuchu::ast::{ExprKind, FieldType, StmtKind};
use chuchu::{lexer::tokenize, parser::Parser};

#[test]
fn parses_exports_imports_and_qualified_types_and_calls() {
    let source = "module app\nuse module math\nexport function point(p: math.Point) be math.Point {\nreturn p\n}\nshow math.add(2, 3)\nassign p as math.Point {\nx: 0\ny: 0\n}\n";
    let module = Parser::new(tokenize(source).unwrap())
        .parse_module()
        .unwrap();
    assert_eq!(module.name.unwrap().0, "app");
    assert_eq!(module.imports[0].alias, "math");
    assert_eq!(module.imports[0].path, "math.chuchu");
    assert_eq!(module.exports[0].0, "point");
    let StmtKind::Function {
        parameters,
        return_type,
        ..
    } = &module.statements[0].kind
    else {
        panic!()
    };
    assert_eq!(parameters[0].ty, FieldType::Named("math.Point".into()));
    assert_eq!(*return_type, FieldType::Named("math.Point".into()));
    let StmtKind::Show(expr) = &module.statements[1].kind else {
        panic!()
    };
    assert!(matches!(&expr.kind, ExprKind::Call { name, .. } if name == "math.add"));
    let StmtKind::Set { value, .. } = &module.statements[2].kind else {
        panic!()
    };
    assert!(matches!(&value.kind, ExprKind::RecordInit { name, .. } if name == "math.Point"));
}

#[test]
fn rejects_misplaced_directives_and_invalid_qualification() {
    for source in [
        "show 1\nuse module x",
        "module a\nmodule b",
        "export assign x as 1",
        "use math",
        "use module",
        "use module \"math.chuchu\"",
        "use module .math",
        "use module math.",
        "use module math..extra",
        "use module math as other",
        "import \"math.chuchu\" as math",
        "show x.",
        "show x.y.z()",
        "show math::add(2, 3)",
        "function old(p: math::Point) be int { return 1 }",
        "function f() be int {\nuse module x\nreturn 1\n}",
    ] {
        assert!(
            Parser::new(tokenize(source).unwrap())
                .parse_module()
                .is_err(),
            "{source}"
        );
    }
}

#[test]
fn dotted_module_names_resolve_to_subdirectories_with_a_short_namespace() {
    let module = Parser::new(tokenize("use module geometry.math\nshow math.add(2, 3)").unwrap())
        .parse_module()
        .unwrap();
    assert_eq!(module.imports[0].path, "geometry/math.chuchu");
    assert_eq!(module.imports[0].alias, "math");
}

#[test]
fn module_calls_and_record_fields_keep_distinct_ast_shapes() {
    let source = "use module math\nshow math.add(point.x, point.nested.y)";
    let module = Parser::new(tokenize(source).unwrap())
        .parse_module()
        .unwrap();
    let StmtKind::Show(expr) = &module.statements[0].kind else {
        panic!()
    };
    let ExprKind::Call { name, arguments } = &expr.kind else {
        panic!()
    };
    assert_eq!(name, "math.add");
    assert!(
        matches!(&arguments[0].kind, ExprKind::FieldAccess { object, field }
        if field == "x" && matches!(&object.kind, ExprKind::Identifier(name) if name == "point"))
    );
    assert!(
        matches!(&arguments[1].kind, ExprKind::FieldAccess { object, field }
        if field == "y" && matches!(&object.kind, ExprKind::FieldAccess { field, .. } if field == "nested"))
    );
}
