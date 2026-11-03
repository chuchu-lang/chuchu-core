//! LLVM representation of records (Issue #11, step 7A): the validated metadata the
//! type checker hands to code generation, the struct type and field indices built
//! from it, and the guarantee that no record program is compiled yet.
use chuchu::ast::Stmt;
use chuchu::codegen::{
    CodegenError, RecordLayouts, generate_program, generate_program_with_options,
    generate_program_with_records,
};
use chuchu::compiler::{CompileError, compile_source};
use chuchu::lexer::tokenize;
use chuchu::optimization::{BuildOptions, target_machine};
use chuchu::parser::Parser;
use chuchu::records::{FieldInfo, RecordInfo, RecordTable};
use chuchu::symbol_table::SymbolTable;
use chuchu::token::Span;
use chuchu::type_checker::{TypeChecker, TypeError};
use chuchu::types::{RecordId, Type};
use inkwell::context::Context;
use inkwell::types::BasicType;

fn parse(source: &str) -> Vec<Stmt> {
    Parser::new(tokenize(source).unwrap())
        .parse_program()
        .unwrap_or_else(|error| panic!("{source:?} should parse: {error:?}"))
}

/// The span of the `occurrence`-th (0-based) match of `text` in `source`.
fn span_of(source: &str, text: &str, occurrence: usize) -> Span {
    let start = source
        .match_indices(text)
        .nth(occurrence)
        .unwrap_or_else(|| panic!("{text:?} #{occurrence} not in {source:?}"))
        .0;
    Span {
        start,
        end: start + text.len(),
    }
}

/// Parses and checks a program whose record declarations are valid, and returns
/// the statements and the metadata. The checker accepts the program, or refuses
/// printing a record; the metadata is available either way, because every
/// declaration validated.
fn checked(source: &str) -> (Vec<Stmt>, RecordTable) {
    let statements = parse(source);
    let mut checker = TypeChecker::new(SymbolTable::new());
    let result = checker.check_program(&statements);
    assert!(
        matches!(result, Ok(()) | Err(TypeError::UnsupportedRecordUse { .. })),
        "{source:?}: {result:?}"
    );
    let table = checker
        .record_table()
        .unwrap_or_else(|| panic!("{source:?} should have validated records"));
    (statements, table)
}

fn table(source: &str) -> RecordTable {
    checked(source).1
}

fn options() -> BuildOptions {
    BuildOptions::default()
}

const SCALARS: &str = "record Sample {\n    count: int\n    ratio: float\n    ready: bool\n}\n";

fn field_names(record: &RecordInfo) -> Vec<&str> {
    record
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect()
}

#[test]
fn the_table_lists_records_in_declaration_order_with_their_ids() {
    let source = "record Second {\n    b: float\n}\nrecord First {\n    a: int\n}\nrecord Third {\n    c: bool\n}";
    let table = table(source);
    assert_eq!(table.len(), 3);
    assert!(!table.is_empty());
    // Ids follow the source, not the names.
    let names: Vec<(usize, &str)> = table
        .iter()
        .map(|(id, record)| (id.0, record.name.as_str()))
        .collect();
    assert_eq!(names, [(0, "Second"), (1, "First"), (2, "Third")]);
    assert_eq!(table.id_of("First"), Some(RecordId(1)));
    assert_eq!(table.id_of("Missing"), None);
    assert_eq!(table.get(RecordId(2)).unwrap().name, "Third");
    assert!(table.get(RecordId(3)).is_none());
    // Spans cover the whole declaration.
    assert_eq!(
        table.get(RecordId(1)).unwrap().span,
        span_of(source, "record First {\n    a: int\n}", 0)
    );
}

#[test]
fn fields_keep_declaration_order_types_and_spans() {
    let source =
        format!("{SCALARS}record Mixed {{\n    zeta: bool\n    alpha: int\n    mid: float\n}}");
    let table = table(&source);
    let sample = table.get(RecordId(0)).unwrap();
    assert_eq!(
        sample.fields,
        vec![
            FieldInfo {
                name: "count".into(),
                ty: Type::Int,
                span: span_of(&source, "count: int", 0),
            },
            FieldInfo {
                name: "ratio".into(),
                ty: Type::Float,
                span: span_of(&source, "ratio: float", 0),
            },
            FieldInfo {
                name: "ready".into(),
                ty: Type::Bool,
                span: span_of(&source, "ready: bool", 0),
            },
        ]
    );
    // Declaration order, not alphabetical.
    assert_eq!(
        field_names(table.get(RecordId(1)).unwrap()),
        ["zeta", "alpha", "mid"]
    );
}

#[test]
fn record_typed_fields_are_resolved_to_ids_even_for_forward_references() {
    let source = "record Course {\n    title: string\n    teacher: Person\n}\nrecord Person {\n    age: int\n}";
    let table = table(source);
    let course = table.get(RecordId(0)).unwrap();
    assert_eq!(course.name, "Course");
    assert_eq!(course.fields[0].ty, Type::String);
    // `Person` is declared later and is `RecordId(1)`.
    assert_eq!(course.fields[1].ty, Type::Record(RecordId(1)));
    assert_eq!(table.id_of("Person"), Some(RecordId(1)));
}

#[test]
fn the_table_is_only_produced_for_validated_records() {
    // Never checked.
    assert_eq!(TypeChecker::new(SymbolTable::new()).record_table(), None);
    // A program without records gives an empty table.
    let mut checker = TypeChecker::new(SymbolTable::new());
    checker
        .check_program(&parse("assign x as 1\nshow x"))
        .unwrap();
    assert_eq!(checker.record_table(), Some(RecordTable::default()));
    // Every kind of record validation failure gives none.
    for source in [
        "record A {\n    x: int\n}\nrecord A {\n    y: int\n}",
        "record A {\n    x: int\n    x: int\n}",
        "record A {\n    b: Missing\n}",
        "record A {\n    a: A\n}",
        "record A {\n    b: B\n}\nrecord B {\n    a: A\n}",
        "record int {\n    x: int\n}",
        "function f() be int {\n    record A {\n        x: int\n    }\n    return 1\n}",
    ] {
        let mut checker = TypeChecker::new(SymbolTable::new());
        assert!(checker.check_program(&parse(source)).is_err(), "{source:?}");
        assert_eq!(checker.record_table(), None, "{source:?}");
    }
    // Records that validated stay available when a later statement is wrong.
    let mut checker = TypeChecker::new(SymbolTable::new());
    let source = format!("{SCALARS}show missing");
    assert!(matches!(
        checker.check_program(&parse(&source)),
        Err(TypeError::UndefinedVariable { .. })
    ));
    assert_eq!(checker.record_table().unwrap().len(), 1);
}

#[test]
fn the_table_is_deterministic() {
    let mut source = String::new();
    for index in 0..12 {
        source += &format!("record R{index} {{\n    c: int\n    a: float\n    b: bool\n}}\n");
    }
    let first = table(&source);
    for _ in 0..30 {
        assert_eq!(table(&source), first);
    }
    let names: Vec<String> = first
        .iter()
        .map(|(_, record)| record.name.clone())
        .collect();
    let expected: Vec<String> = (0..12).map(|index| format!("R{index}")).collect();
    assert_eq!(names, expected);
}

#[test]
fn each_record_becomes_a_named_llvm_struct() {
    let source = "record Student {\n    age: int\n    gpa: float\n    active: bool\n}\nrecord Point {\n    x: float\n    y: float\n}";
    let table = table(source);
    let context = Context::create();
    let layouts = RecordLayouts::new(&context, &table).unwrap();
    assert_eq!(layouts.len(), 2);
    assert!(!layouts.is_empty());

    let student = layouts.get(RecordId(0)).unwrap();
    assert_eq!(student.id(), RecordId(0));
    assert_eq!(student.name(), "Student");
    let struct_type = student.struct_type();
    assert_eq!(
        struct_type.get_name().unwrap().to_str().unwrap(),
        "chuchu.record.Student"
    );
    assert!(!struct_type.is_packed());
    assert!(!struct_type.is_opaque());
    assert_eq!(struct_type.count_fields(), 3);
    assert_eq!(
        struct_type.get_field_types(),
        [
            context.i64_type().as_basic_type_enum(),
            context.f64_type().as_basic_type_enum(),
            context.bool_type().as_basic_type_enum(),
        ]
    );

    // A second record has its own struct, found by id or by name.
    let point = layouts.by_name("Point").unwrap();
    assert_eq!(point.id(), RecordId(1));
    assert_eq!(point.struct_type().count_fields(), 2);
    assert_ne!(point.struct_type(), student.struct_type());
    assert_eq!(layouts.get(RecordId(1)).unwrap().name(), "Point");
    assert!(layouts.by_name("Missing").is_none());
    assert!(layouts.get(RecordId(2)).is_none());
    let order: Vec<&str> = layouts.iter().map(|layout| layout.name()).collect();
    assert_eq!(order, ["Student", "Point"]);
}

#[test]
fn bool_fields_use_the_same_llvm_type_as_bool_locals() {
    // Locals of every supported type use these scalar types, so a later load or
    // store of a field needs no conversion.
    let table = table("record R {\n    i: int\n    f: float\n    b: bool\n}");
    let context = Context::create();
    let layouts = RecordLayouts::new(&context, &table).unwrap();
    let types = layouts
        .get(RecordId(0))
        .unwrap()
        .struct_type()
        .get_field_types();
    assert_eq!(types[0].into_int_type().get_bit_width(), 64);
    assert!(types[1].is_float_type());
    assert_eq!(types[2].into_int_type().get_bit_width(), 1);
}

#[test]
fn field_indices_follow_declaration_order() {
    let table = table(
        "record R {\n    zeta: int\n    alpha: bool\n    mid: float\n    last: int\n}\nrecord S {\n    alpha: float\n    zeta: int\n}",
    );
    let context = Context::create();
    let layouts = RecordLayouts::new(&context, &table).unwrap();
    let record = layouts.by_name("R").unwrap();
    let listed: Vec<(&str, u32, Type)> = record
        .fields()
        .iter()
        .map(|field| (field.name.as_str(), field.index, field.ty))
        .collect();
    assert_eq!(
        listed,
        [
            ("zeta", 0, Type::Int),
            ("alpha", 1, Type::Bool),
            ("mid", 2, Type::Float),
            ("last", 3, Type::Int),
        ]
    );
    for (position, field) in record.fields().iter().enumerate() {
        assert_eq!(field.index as usize, position);
        assert_eq!(record.field_index(&field.name), Some(field.index));
        assert_eq!(record.field(&field.name), Some(field));
    }
    assert_eq!(record.field_index("zeta"), Some(0));
    assert_eq!(record.field_index("last"), Some(3));
    assert_eq!(record.field_index("nope"), None);
    assert_eq!(record.field("nope"), None);
    // Names are case-sensitive, and another record has its own indices.
    assert_eq!(record.field_index("Zeta"), None);
    let other = layouts.by_name("S").unwrap();
    assert_eq!(other.field_index("alpha"), Some(0));
    assert_eq!(other.field_index("zeta"), Some(1));
    // The struct's element types are in the same order as the indices.
    for field in record.fields() {
        let expected = match field.ty {
            Type::Int => context.i64_type().as_basic_type_enum(),
            Type::Float => context.f64_type().as_basic_type_enum(),
            Type::Bool => context.bool_type().as_basic_type_enum(),
            other => panic!("unexpected field type {other:?}"),
        };
        assert_eq!(
            record.struct_type().get_field_type_at_index(field.index),
            Some(expected)
        );
    }
}

#[test]
fn layouts_are_deterministic_across_contexts() {
    let mut source = String::new();
    for index in 0..8 {
        source += &format!("record R{index} {{\n    c: int\n    a: float\n    b: bool\n}}\n");
    }
    let table = table(&source);
    let describe = || {
        let context = Context::create();
        let layouts = RecordLayouts::new(&context, &table).unwrap();
        layouts
            .iter()
            .map(|layout| {
                (
                    layout.id(),
                    layout.name().to_owned(),
                    layout
                        .struct_type()
                        .get_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    layout.fields().to_vec(),
                    layout.struct_type().count_fields(),
                )
            })
            .collect::<Vec<_>>()
    };
    let first = describe();
    assert_eq!(first.len(), 8);
    for _ in 0..10 {
        assert_eq!(describe(), first);
    }
}

#[test]
fn the_layout_has_the_targets_natural_offsets_and_size() {
    let table = table(SCALARS);
    let context = Context::create();
    let layouts = RecordLayouts::new(&context, &table).unwrap();
    let layout = layouts.by_name("Sample").unwrap();
    let module = context.create_module("layout");
    let machine = target_machine(&module, &options()).unwrap();
    let data = machine.get_target_data();
    let struct_type = layout.struct_type();

    let offsets: Vec<u64> = layout
        .fields()
        .iter()
        .map(|field| data.offset_of_element(&struct_type, field.index).unwrap())
        .collect();
    // Fields never overlap and never move: offsets follow the field indices.
    assert!(
        offsets.windows(2).all(|pair| pair[0] < pair[1]),
        "{offsets:?}"
    );
    assert_eq!(offsets[0], 0);
    for (field, offset) in layout.fields().iter().zip(&offsets) {
        let alignment = match field.ty {
            Type::Int => data.get_abi_alignment(&context.i64_type()),
            Type::Float => data.get_abi_alignment(&context.f64_type()),
            _ => data.get_abi_alignment(&context.bool_type()),
        };
        assert_eq!(offset % u64::from(alignment), 0, "{}", field.name);
    }
    let size = data.get_abi_size(&struct_type);
    assert!(size > data.get_abi_size(&context.i64_type()) + data.get_abi_size(&context.f64_type()));
    assert_eq!(size % u64::from(data.get_abi_alignment(&struct_type)), 0);
    // Every supported target is 64-bit: an i64, then a double, then one byte, padded.
    if cfg!(target_pointer_width = "64") {
        assert_eq!(offsets, [0, 8, 16]);
        assert_eq!(size, 24);
    }
}

#[test]
fn field_indices_address_the_right_fields_in_ir() {
    let table = table("record R {\n    a: int\n    b: float\n    c: bool\n}");
    let context = Context::create();
    let layouts = RecordLayouts::new(&context, &table).unwrap();
    let layout = layouts.by_name("R").unwrap();
    let module = context.create_module("gep");
    let builder = context.create_builder();
    let function = module.add_function("probe", context.void_type().fn_type(&[], false), None);
    builder.position_at_end(context.append_basic_block(function, "entry"));
    let slot = builder.build_alloca(layout.struct_type(), "r").unwrap();
    for field in layout.fields() {
        // An index is only valid if LLVM finds that element in the struct.
        let pointer = builder
            .build_struct_gep(layout.struct_type(), slot, field.index, &field.name)
            .unwrap();
        assert!(pointer.is_const() || pointer.get_name().to_str().unwrap() == field.name);
    }
    // One past the last field is rejected by LLVM, which shows the count is exact.
    assert!(
        builder
            .build_struct_gep(layout.struct_type(), slot, 3, "past")
            .is_err()
    );
    builder.build_return(None).unwrap();
    module.verify().unwrap();
}

#[test]
fn string_and_record_fields_have_layouts() {
    let source = "record Address {\n    city: string\n    zip: int\n}\nrecord Person {\n    name: string\n    home: Address\n    age: int\n}";
    let context = Context::create();
    let layouts = RecordLayouts::new(&context, &table(source)).unwrap();
    let address = layouts.by_name("Address").unwrap();
    assert_eq!(
        address.struct_type().get_field_types(),
        [
            context
                .ptr_type(inkwell::AddressSpace::default())
                .as_basic_type_enum(),
            context.i64_type().as_basic_type_enum(),
        ]
    );
    // A nested record is embedded by value, as that record's own struct.
    let person = layouts.by_name("Person").unwrap();
    assert_eq!(
        person.struct_type().get_field_types(),
        [
            context
                .ptr_type(inkwell::AddressSpace::default())
                .as_basic_type_enum(),
            address.struct_type().as_basic_type_enum(),
            context.i64_type().as_basic_type_enum(),
        ]
    );
    assert_eq!(person.field_index("home"), Some(1));
    assert_eq!(person.field("home").unwrap().ty, Type::Record(address.id()));
    assert!(!person.struct_type().is_opaque());
}

#[test]
fn records_may_be_nested_in_any_declaration_order() {
    // The embedding record is declared before, between, and after what it embeds.
    for source in [
        "record A {\n    b: B\n}\nrecord B {\n    c: C\n}\nrecord C {\n    n: int\n}",
        "record C {\n    n: int\n}\nrecord B {\n    c: C\n}\nrecord A {\n    b: B\n}",
        "record B {\n    c: C\n}\nrecord A {\n    b: B\n    c: C\n}\nrecord C {\n    n: int\n}",
    ] {
        let context = Context::create();
        let layouts = RecordLayouts::new(&context, &table(source)).unwrap();
        for layout in layouts.iter() {
            assert!(!layout.struct_type().is_opaque(), "{source:?}");
        }
        // The innermost record has one field; each outer one embeds the next.
        let a = layouts.by_name("A").unwrap().struct_type();
        let b = layouts.by_name("B").unwrap().struct_type();
        let c = layouts.by_name("C").unwrap().struct_type();
        assert_eq!(c.count_fields(), 1);
        assert_eq!(b.get_field_type_at_index(0), Some(c.as_basic_type_enum()));
        assert_eq!(
            a.get_field_type_at_index(0),
            Some(b.as_basic_type_enum()),
            "{source:?}"
        );
        // The module is valid, so every struct is complete and sized.
        let module = context.create_module("nested");
        let builder = context.create_builder();
        let function = module.add_function("probe", context.void_type().fn_type(&[], false), None);
        builder.position_at_end(context.append_basic_block(function, "entry"));
        builder.build_alloca(a, "a").unwrap();
        builder.build_return(None).unwrap();
        module.verify().unwrap();
    }
}

#[test]
fn an_empty_table_has_no_layouts() {
    let context = Context::create();
    let layouts = RecordLayouts::new(&context, &RecordTable::default()).unwrap();
    assert!(layouts.is_empty());
    assert_eq!(layouts.len(), 0);
    assert_eq!(layouts.iter().count(), 0);
}

#[test]
fn declarations_are_not_executable_statements() {
    let plain =
        "assign x as 2\nshow x * 3\nfunction f(n: int) be int {\n    return n + 1\n}\nshow f(x)";
    let with_records = format!(
        "{SCALARS}record Point {{\n    x: float\n    y: float\n}}\n{plain}\nrecord Late {{\n    flag: bool\n}}"
    );
    let (statements, table) = checked(&with_records);
    assert_eq!(table.len(), 3);
    let context = Context::create();
    let module = generate_program_with_records(&context, &statements, &table, None, &options())
        .expect("declarations with validated layouts generate no code");
    // The module is the one the same program without declarations produces. The
    // declarations are blanked rather than removed, because runtime messages carry
    // source offsets that removing them would shift.
    let mut blanked = with_records.clone();
    for (_, record) in table.iter() {
        let declaration = &with_records[record.span.start..record.span.end];
        let spaces: String = declaration
            .chars()
            .map(|c| if c == '\n' { '\n' } else { ' ' })
            .collect();
        blanked = blanked.replacen(declaration, &spaces, 1);
    }
    assert!(!blanked.contains("record"));
    let baseline_context = Context::create();
    let baseline = generate_program(&baseline_context, &parse(&blanked)).unwrap();
    assert_eq!(
        module.print_to_string().to_string(),
        baseline.print_to_string().to_string()
    );
    assert!(module.get_function("main").is_some());
    assert!(module.get_function("chuchu.f").is_some());
    // Declarations alone, and a program that is nothing but declarations.
    let (statements, table) = checked(SCALARS);
    let context = Context::create();
    let module =
        generate_program_with_records(&context, &statements, &table, None, &options()).unwrap();
    assert!(module.get_function("main").is_some());
    assert_eq!(module.get_functions().count(), 1);
}

#[test]
fn declarations_need_validated_metadata() {
    let source = format!("{SCALARS}show 1");
    let (statements, table) = checked(&source);
    let declaration = span_of(&source, SCALARS.trim_end(), 0);
    // The entry points that take no table have no layout for the declaration.
    let context = Context::create();
    for result in [
        generate_program(&context, &statements),
        generate_program_with_options(&context, &statements, None, &options()),
        generate_program_with_records(
            &context,
            &statements,
            &RecordTable::default(),
            None,
            &options(),
        ),
    ] {
        assert!(
            matches!(&result, Err(CodegenError::InvalidType { message, span })
                if message.contains("no validated layout") && *span == declaration),
            "{:?}",
            result.map(|_| ())
        );
    }
    // A table for some other program does not cover this declaration either.
    let (_, other) = checked("record Other {\n    n: int\n}");
    let result = generate_program_with_records(&context, &statements, &other, None, &options());
    assert!(matches!(
        result,
        Err(CodegenError::InvalidType { message, .. }) if message.contains("no validated layout")
    ));
    // The right table is accepted.
    assert!(generate_program_with_records(&context, &statements, &table, None, &options()).is_ok());
}

#[test]
fn records_with_strings_generate_valid_modules() {
    let source = "record Named {\n    name: string\n    n: int\n}\nassign a as Named {\n    name: \"x\"\n    n: 1\n}\nshow a.name";
    let (statements, table) = checked(source);
    let context = Context::create();
    let module = generate_program_with_records(&context, &statements, &table, None, &options())
        .expect("string fields are supported");
    module.verify().unwrap();
}

#[test]
fn code_generation_refuses_what_it_cannot_lower() {
    // The checker already rejects printing a record; lowering refuses it too, so a
    // program that skips the checker cannot produce a wrong executable.
    let initialized = format!(
        "{SCALARS}assign s as Sample {{\n    count: 1\n    ratio: 2.5\n    ready: true\n}}\n"
    );
    let (statements, table) = checked(&format!("{initialized}show s\n"));
    let context = Context::create();
    let result = generate_program_with_records(&context, &statements, &table, None, &options());
    assert!(
        matches!(&result, Err(CodegenError::InvalidType { message, .. })
            if *message == "show requires a scalar"),
        "{:?}",
        result.map(|_| ())
    );
    // A field write needs a variable to write to; without one lowering stops.
    let statements = parse("reassign s.count as 2");
    let context = Context::create();
    let result = generate_program_with_records(
        &context,
        &statements,
        &RecordTable::default(),
        None,
        &options(),
    );
    assert!(
        matches!(&result, Err(CodegenError::UndefinedVariable { name, .. }) if name == "s"),
        "{:?}",
        result.map(|_| ())
    );
    // Copying, reassigning, writing a field, and reading one all lower.
    for body in [
        "assign t as s\n",
        "reassign s as Sample {\n    count: 2\n    ratio: 1.0\n    ready: false\n}\n",
        "reassign s.count as 5\n",
        "show s.count\n",
    ] {
        let (statements, table) = checked(&format!("{initialized}{body}"));
        let context = Context::create();
        let module = generate_program_with_records(&context, &statements, &table, None, &options())
            .unwrap_or_else(|error| panic!("{body:?}: {error:?}"));
        module.verify().unwrap();
    }
}

#[test]
fn programs_without_records_generate_the_same_module_as_before() {
    let source = "assign total as 0\nassign i as 0\nwhile i < 5 {\n    reassign total as total + i * 2\n    reassign i as i + 1\n}\nfunction square(n: int) be int {\n    return n * n\n}\nshow square(total)\nassign f as 1.5\nshow f * 2.0\nassign xs as [1, 2, 3]\nshow xs[1]";
    let statements = parse(source);
    let mut checker = TypeChecker::new(SymbolTable::new());
    checker.check_program(&statements).unwrap();
    let table = checker.record_table().unwrap();
    assert!(table.is_empty());
    let first = Context::create();
    let second = Context::create();
    let third = Context::create();
    let old = generate_program(&first, &statements).unwrap();
    let options_entry =
        generate_program_with_options(&second, &statements, None, &options()).unwrap();
    let with_table =
        generate_program_with_records(&third, &statements, &table, None, &options()).unwrap();
    let old = old.print_to_string().to_string();
    assert_eq!(options_entry.print_to_string().to_string(), old);
    assert_eq!(with_table.print_to_string().to_string(), old);
}

#[test]
fn the_pipeline_rejects_printing_a_record_before_code_generation() {
    let directory =
        std::env::temp_dir().join(format!("cheeku record layout {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join("program");
    let initializer = "{\n    count: 1\n    ratio: 2.5\n    ready: true\n}";
    for source in [
        format!("{SCALARS}assign s as Sample {initializer}\nshow s"),
        format!("{SCALARS}show Sample {initializer}"),
    ] {
        let error = compile_source(&source, &output).unwrap_err();
        assert!(
            matches!(
                &error,
                CompileError::Type(TypeError::UnsupportedRecordUse { .. })
            ),
            "{source:?}: {error:?}"
        );
        assert!(!output.exists(), "{source:?}");
    }
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn ordinary_programs_still_compile_and_run() {
    let directory =
        std::env::temp_dir().join(format!("cheeku record plain {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let output = directory.join(if cfg!(windows) {
        "program.exe"
    } else {
        "program"
    });
    compile_source(
        "assign x as 10\nassign y as 20\nshow x + y\nreassign x as 30\nshow x + y",
        &output,
    )
    .unwrap();
    let run = std::process::Command::new(&output).output().unwrap();
    assert!(run.status.success());
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).replace("\r\n", "\n"),
        "30\n50\n"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}
