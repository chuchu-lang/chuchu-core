use crate::ast::{
    BinaryOperator, Expr, ExprKind, FieldInit, FieldTarget, FieldType, RecordField, Stmt, StmtKind,
};
use crate::records::{FieldInfo, RecordInfo, RecordTable};
use crate::symbol_table::{SymbolError, SymbolTable};
use crate::token::Span;
use crate::types::{MAX_ARRAY_LEN, NumericType, RecordId, ScalarType, Type};
use std::collections::{HashMap, HashSet};

#[derive(Debug, PartialEq, Eq)]
pub enum TypeError {
    UndefinedVariable {
        name: String,
        span: Span,
    },
    DuplicateDeclaration {
        name: String,
        span: Span,
    },
    InvalidOperands {
        left: Type,
        right: Type,
        span: Span,
    },
    Mismatch {
        expected: Type,
        actual: Type,
        span: Span,
    },
    InvalidCondition {
        span: Span,
    },
    InvalidFunction {
        name: String,
        message: &'static str,
        span: Span,
    },
    InvalidArray {
        message: &'static str,
        span: Span,
    },
    InvalidLiteral {
        literal: String,
        span: Span,
    },
    InvalidReturn {
        span: Span,
    },
    MissingReturn {
        name: String,
        span: Span,
    },
    /// An operation on a record value that cannot be compiled yet.
    UnsupportedRecordUse {
        feature: &'static str,
        span: Span,
    },
    DuplicateRecord {
        name: String,
        span: Span,
    },
    DuplicateField {
        record: String,
        field: String,
        span: Span,
    },

    RecordNotTopLevel {
        name: String,
        span: Span,
    },
    ReservedRecordName {
        name: String,
        span: Span,
    },
    UndefinedRecordType {
        record: String,
        type_name: String,
        /// The type name written after the field's `:`.
        span: Span,
    },
    RecursiveRecord {
        /// The records in the cycle, starting and ending with the same name.
        path: Vec<String>,
        /// The field that closes the cycle.
        span: Span,
    },
    /// An initializer names a record that is not declared.
    UndefinedRecord {
        name: String,
        span: Span,
    },

    /// An initializer sets, or an access reads, a field the record does not have.
    UnknownField {
        record: String,
        field: String,
        /// The field name in the initializer or after the dot.
        span: Span,
    },
    /// A field was read from a value that is not a record.
    FieldAccessOnNonRecord {
        field: String,
        /// The type of the value the field was read from.
        actual: String,
        /// The value before the dot.
        span: Span,
    },
    /// A field was assigned on a value that is not a record.
    FieldAssignOnNonRecord {
        field: String,
        /// The type of the value the field was assigned on.
        actual: String,
        /// The target before the dot.
        span: Span,
    },
    /// An initializer sets the same field twice.
    DuplicateInitializer {
        record: String,
        field: String,
        /// The second occurrence of the field name.
        span: Span,
    },
    /// An initializer leaves out fields, listed in declaration order.
    MissingFields {
        record: String,
        fields: Vec<String>,
        /// The whole initializer.
        span: Span,
    },
    /// A field's value does not have the field's declared type.
    FieldMismatch {
        detail: Box<FieldMismatch>,
        /// The field's value.
        span: Span,
    },
    /// A value of a record type was used where another type was required, or the
    /// reverse. Types are named because a record's id has no name on its own.
    RecordMismatch {
        expected: String,
        actual: String,
        span: Span,
    },
}

/// The names behind a [`TypeError::FieldMismatch`], boxed there so the error stays
/// small enough to return by value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldMismatch {
    pub record: String,
    pub field: String,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub parameters: Vec<Type>,
    pub result: Type,
}

pub struct TypeChecker {
    symbols: SymbolTable,
    functions: HashMap<String, FunctionSignature>,
    /// Declared top-level records and their fields, in any source order.
    records: HashMap<String, Vec<RecordField>>,
    /// The id of each declared record: its position in `record_names`.
    record_ids: HashMap<String, RecordId>,
    /// Record names in declaration order, to name a `Type::Record`.
    record_names: Vec<String>,
    /// The declaration span of each record, parallel to `record_names`.
    record_spans: Vec<Span>,
    /// Whether the last `check_program` got through record validation.
    records_validated: bool,
    /// The first record feature that cannot be compiled yet, reported once the whole
    /// program has checked, so an actual error elsewhere takes precedence.
    unsupported_record: Option<TypeError>,
    /// Whether `check_program` is running, which is when unsupported record
    /// features may be deferred instead of returned.
    in_program: bool,
    result: Option<Type>,
    depth: usize,
}

/// Names of the built-in types, which a record may not take.
const RESERVED_TYPE_NAMES: [&str; 5] = ["int", "float", "bool", "string", "result"];

/// A field's span runs from its name to the end of its type, so the type name is
/// the last `type_name.len()` bytes.
fn type_name_span(field: &RecordField, type_name: &str) -> Span {
    // Resolved module identities are longer than their source spelling. In that
    // case highlight the field rather than subtracting an internal name length.
    if type_name.contains('[') {
        return field.span;
    }
    Span {
        start: field
            .span
            .end
            .saturating_sub(type_name.len())
            .max(field.span.start),
        end: field.span.end,
    }
}

pub fn reserved_function(name: &str) -> bool {
    matches!(
        name,
        "input"
            | "try_input"
            | "read_file"
            | "try_read_file"
            | "write_file"
            | "try_write_file"
            | "parse_int"
            | "parse_float"
            | "to_string"
            | "is_ok"
            | "is_eof"
            | "error_code"
            | "error_message"
            | "unwrap"
            | "ok"
            | "err"
            | "abs"
            | "sqrt"
            | "min"
            | "max"
            | "sum"
            | "length"
            | "main"
            | "printf"
            | "write"
            | "exit"
            | "fmin"
            | "fmax"
            | "fabs"
            | "malloc"
            | "free"
            | "strlen"
            | "memcpy"
            | "strcmp"
    ) || crate::runtime::FUNCTIONS.contains(&name)
}

/// Types a call to one of the file built-ins, or returns `None` for any other name.
///
/// | Call | Result |
/// | --- | --- |
/// | `read_file(path)` | `string` |
/// | `try_read_file(path)` | `result<string>` |
/// | `write_file(path, text)` | `int`, the number of bytes written |
/// | `try_write_file(path, text)` | `result<int>`, the number of bytes written |
///
/// Every argument must be exactly `string`; there is no implicit conversion. The
/// messages name the argument at fault because a path and the text to write are
/// both strings and easy to swap or mistype.
fn file_call(name: &str, arguments: &[Type]) -> Option<Result<Type, &'static str>> {
    let (result, writes) = match name {
        "read_file" => (Type::String, false),
        "try_read_file" => (Type::Result(ScalarType::String), false),
        "write_file" => (Type::Int, true),
        "try_write_file" => (Type::Result(ScalarType::Int), true),
        _ => return None,
    };
    if arguments.len() != if writes { 2 } else { 1 } {
        return Some(Err(if writes {
            "expected 2 arguments: the file path and the text to write"
        } else {
            "expected 1 argument: the file path"
        }));
    }
    if arguments[0] != Type::String {
        return Some(Err("the file path must be a string"));
    }
    if writes && arguments[1] != Type::String {
        return Some(Err(
            "the text to write must be a string; convert values with to_string(...) or extract a result with unwrap(...)",
        ));
    }
    Some(Ok(result))
}

impl TypeChecker {
    pub fn new(symbols: SymbolTable) -> Self {
        Self {
            symbols,
            functions: HashMap::new(),
            records: HashMap::new(),
            record_ids: HashMap::new(),
            record_names: Vec::new(),
            record_spans: Vec::new(),
            records_validated: false,
            unsupported_record: None,
            in_program: false,
            result: None,
            depth: 0,
        }
    }
    /// Checks statements in source order; successful earlier declarations are retained on error.
    pub fn check_program(&mut self, statements: &[Stmt]) -> Result<(), TypeError> {
        self.in_program = true;
        let result = self.check_whole_program(statements);
        self.in_program = false;
        result
    }
    /// Reports a record feature that cannot be compiled yet. Inside a program it is
    /// remembered, first one wins, and returned after every statement has checked.
    fn defer_unsupported(&mut self, error: TypeError) -> Result<(), TypeError> {
        if self.in_program {
            self.unsupported_record.get_or_insert(error);
            Ok(())
        } else {
            Err(error)
        }
    }
    fn check_whole_program(&mut self, statements: &[Stmt]) -> Result<(), TypeError> {
        let mut declared = HashSet::new();
        for stmt in statements {
            if let StmtKind::Function { name, .. } = &stmt.kind
                && (reserved_function(name)
                    || self.functions.contains_key(name)
                    || !declared.insert(name.as_str()))
            {
                return Err(TypeError::InvalidFunction {
                    name: name.clone(),
                    message: "duplicate or reserved function name",
                    span: stmt.span,
                });
            }
        }
        // Record declarations are validated before any statement is checked, so a
        // malformed declaration is reported first, and so signatures can name them.
        self.unsupported_record = None;
        self.records_validated = false;
        self.validate_records(statements, true)?;
        self.resolve_record_types(statements)?;
        self.records_validated = true;

        for stmt in statements {
            if let StmtKind::Function {
                name,
                parameters,
                return_type,
                return_span,
                ..
            } = &stmt.kind
            {
                let parameters = parameters
                    .iter()
                    .map(|parameter| self.annotation_type(&parameter.ty, parameter.ty_span))
                    .collect::<Result<Vec<_>, _>>()?;
                let result = self.annotation_type(return_type, *return_span)?;
                self.functions
                    .insert(name.clone(), FunctionSignature { parameters, result });
            }
        }
        for stmt in statements {
            // A declaration generates no code of its own.
            if matches!(stmt.kind, StmtKind::Record { .. }) {
                continue;
            }
            self.check_statement(stmt)?;
        }
        self.unsupported_record.take().map_or(Ok(()), Err)
    }
    /// The validated records of the last `check_program`, resolved and in
    /// declaration order, for code generation. `None` until record validation has
    /// succeeded, so a half-validated registry is never handed on. A program
    /// without records gives an empty table.
    pub fn record_table(&self) -> Option<RecordTable> {
        if !self.records_validated {
            return None;
        }
        let mut records = Vec::with_capacity(self.record_names.len());
        for (name, &span) in self.record_names.iter().zip(&self.record_spans) {
            let fields = self
                .records
                .get(name)?
                .iter()
                .map(|field| {
                    Ok(FieldInfo {
                        name: field.name.clone(),
                        ty: self.field_type(name, field)?,
                        span: field.span,
                    })
                })
                .collect::<Result<Vec<_>, TypeError>>()
                .ok()?;
            records.push(RecordInfo {
                name: name.clone(),
                fields,
                span,
            });
        }
        Some(RecordTable::new(records))
    }
    /// Checks record declarations in source order, including those nested inside
    /// functions and blocks: each must be top-level, uniquely named, and have
    /// uniquely named fields.
    fn validate_records(&mut self, statements: &[Stmt], top_level: bool) -> Result<(), TypeError> {
        for stmt in statements {
            match &stmt.kind {
                StmtKind::Record { name, fields } => {
                    if !top_level {
                        return Err(TypeError::RecordNotTopLevel {
                            name: name.clone(),
                            span: stmt.span,
                        });
                    }
                    if RESERVED_TYPE_NAMES.contains(&name.as_str()) {
                        return Err(TypeError::ReservedRecordName {
                            name: name.clone(),
                            span: stmt.span,
                        });
                    }
                    if self.records.contains_key(name) {
                        return Err(TypeError::DuplicateRecord {
                            name: name.clone(),
                            span: stmt.span,
                        });
                    }
                    self.record_ids
                        .insert(name.clone(), RecordId(self.record_names.len()));
                    self.record_names.push(name.clone());
                    self.record_spans.push(stmt.span);
                    self.records.insert(name.clone(), fields.clone());
                    let mut seen = HashSet::new();
                    for field in fields {
                        if !seen.insert(field.name.as_str()) {
                            return Err(TypeError::DuplicateField {
                                record: name.clone(),
                                field: field.name.clone(),
                                span: field.span,
                            });
                        }
                    }
                }
                StmtKind::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    self.validate_records(then_body, false)?;
                    self.validate_records(else_body, false)?;
                }
                StmtKind::While { body, .. } | StmtKind::Function { body, .. } => {
                    self.validate_records(body, false)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// Resolves record-typed fields against every declared record, so records may
    /// refer to ones declared later, then rejects records that contain themselves,
    /// directly or through other records, because a value cannot have infinite size.
    fn resolve_record_types(&self, statements: &[Stmt]) -> Result<(), TypeError> {
        let declared = || {
            statements.iter().filter_map(|stmt| match &stmt.kind {
                StmtKind::Record { name, .. } => Some(name),
                _ => None,
            })
        };
        for name in declared() {
            for field in &self.records[name] {
                if let FieldType::Named(type_name) = &field.ty
                    && !self.records.contains_key(type_name)
                {
                    return Err(TypeError::UndefinedRecordType {
                        record: name.clone(),
                        type_name: type_name.clone(),
                        span: type_name_span(field, type_name),
                    });
                }
            }
        }
        // Depth-first search with an explicit stack, so a long chain of records
        // cannot overflow the compiler's own stack. Every field type resolves.
        let mut finished: HashSet<&str> = HashSet::new();
        let mut in_progress: HashSet<&str> = HashSet::new();
        for start in declared() {
            if finished.contains(start.as_str()) {
                continue;
            }
            let mut stack: Vec<(&str, usize)> = vec![(start, 0)];
            in_progress.insert(start.as_str());
            while let Some(&mut (node, ref mut next)) = stack.last_mut() {
                let fields = &self.records[node];
                let Some((index, field, target)) =
                    fields
                        .iter()
                        .enumerate()
                        .skip(*next)
                        .find_map(|(i, f)| match &f.ty {
                            FieldType::Named(target) => Some((i, f, target.as_str())),
                            FieldType::Builtin(_) => None,
                        })
                else {
                    in_progress.remove(node);
                    finished.insert(node);
                    stack.pop();
                    continue;
                };
                *next = index + 1;
                if finished.contains(target) {
                    continue;
                }
                if in_progress.contains(target) {
                    let position = stack
                        .iter()
                        .position(|(name, _)| *name == target)
                        .expect("an in-progress record is on the stack");
                    let mut path: Vec<String> = stack[position..]
                        .iter()
                        .map(|(n, _)| (*n).to_owned())
                        .collect();
                    path.push(target.to_owned());
                    return Err(TypeError::RecursiveRecord {
                        path,
                        span: type_name_span(field, target),
                    });
                }
                in_progress.insert(target);
                stack.push((target, 0));
            }
        }
        Ok(())
    }

    fn block(&mut self, statements: &[Stmt]) -> Result<(), TypeError> {
        self.symbols.enter_scope();
        self.depth += 1;
        let result = statements
            .iter()
            .try_for_each(|statement| self.check_statement(statement));
        self.depth -= 1;
        self.symbols.exit_scope();
        result
    }
    fn mismatch(&self, expected: Type, value: &Expr) -> Result<(), TypeError> {
        let actual = self.infer_expression(value)?;
        if !expected.accepts(&actual) {
            return Err(self.mismatch_error(expected, actual, value.span));
        }
        Ok(())
    }
    /// A mismatch error that names record types, which only the checker can do.
    fn mismatch_error(&self, expected: Type, actual: Type, span: Span) -> TypeError {
        if matches!(expected, Type::Record(_)) || matches!(actual, Type::Record(_)) {
            TypeError::RecordMismatch {
                expected: self.type_label(expected),
                actual: self.type_label(actual),
                span,
            }
        } else {
            TypeError::Mismatch {
                expected,
                actual,
                span,
            }
        }
    }
    fn type_label(&self, ty: Type) -> String {
        match ty {
            Type::Int => "int".into(),
            Type::Float => "float".into(),
            Type::Bool => "bool".into(),
            Type::String => "string".into(),
            Type::Result(payload) => format!("result<{}>", self.type_label(payload.ty())),
            Type::Array { element, len } => {
                format!("{}[{len}]", self.type_label(element.scalar()))
            }
            Type::Record(id) => self
                .record_names
                .get(id.0)
                .map_or_else(|| "record".into(), Clone::clone),
        }
    }
    /// The type a parameter or return annotation names; a record name resolves to
    /// that record, which must be declared at the top level.
    fn annotation_type(&self, ty: &FieldType, span: Span) -> Result<Type, TypeError> {
        match ty {
            FieldType::Builtin(ty) => Ok(*ty),
            FieldType::Named(name) => self
                .record_ids
                .get(name)
                .map(|&id| Type::Record(id))
                .ok_or_else(|| TypeError::UndefinedRecord {
                    name: name.clone(),
                    span,
                }),
        }
    }
    /// The type of a declared field; a record-typed field resolves to that record.
    fn field_type(&self, record: &str, field: &RecordField) -> Result<Type, TypeError> {
        match &field.ty {
            FieldType::Builtin(ty) => Ok(*ty),
            FieldType::Named(type_name) => self
                .record_ids
                .get(type_name)
                .map(|&id| Type::Record(id))
                .ok_or_else(|| TypeError::UndefinedRecordType {
                    record: record.to_owned(),
                    type_name: type_name.clone(),
                    span: type_name_span(field, type_name),
                }),
        }
    }
    /// Checks `object.field`: the object is a record that declares the field. The
    /// result is the field's type, so chains such as `a.b.c` resolve one link at a time.
    fn field_access(&self, target: &Expr, field: &str, span: Span) -> Result<Type, TypeError> {
        let ty = self.infer_expression(target)?;
        // The span of an access ends with the field name.
        let field_span = Span {
            start: span.end - field.len(),
            end: span.end,
        };
        self.field_of(ty, field, target.span, field_span, false)
            .map(|(_, field_ty)| field_ty)
    }
    /// Looks up `field` on a value of type `ty`, returning the record that declares
    /// it and the field's type. Reading and writing share this, so they accept
    /// exactly the same fields; `write` only changes how a non-record is described.
    fn field_of(
        &self,
        ty: Type,
        field: &str,
        object_span: Span,
        field_span: Span,
        write: bool,
    ) -> Result<(&str, Type), TypeError> {
        let declared = match ty {
            Type::Record(id) => self
                .record_names
                .get(id.0)
                .and_then(|name| Some((name, self.records.get(name)?))),
            _ => None,
        };
        let Some((record, fields)) = declared else {
            let (field, actual, span) = (field.to_owned(), self.type_label(ty), object_span);
            return Err(if write {
                TypeError::FieldAssignOnNonRecord {
                    field,
                    actual,
                    span,
                }
            } else {
                TypeError::FieldAccessOnNonRecord {
                    field,
                    actual,
                    span,
                }
            });
        };
        let Some(declared_field) = fields.iter().find(|candidate| candidate.name == field) else {
            return Err(TypeError::UnknownField {
                record: record.clone(),
                field: field.to_owned(),
                span: field_span,
            });
        };
        Ok((record, self.field_type(record, declared_field)?))
    }
    /// Checks `reassign variable.field... as value`: the variable exists, each field
    /// is declared by the record before it, and the value fits the last field.
    fn assign_field(&self, target: &FieldTarget, value: &Expr) -> Result<(), TypeError> {
        let mut ty =
            self.symbols
                .lookup(&target.variable)
                .ok_or_else(|| TypeError::UndefinedVariable {
                    name: target.variable.clone(),
                    span: target.variable_span,
                })?;
        let mut owner = "";
        let mut object_span = target.variable_span;
        for field in &target.fields {
            (owner, ty) = self.field_of(ty, &field.name, object_span, field.span, true)?;
            object_span = Span {
                start: target.span.start,
                end: field.span.end,
            };
        }
        let actual = self.infer_expression(value)?;
        if ty.accepts(&actual) {
            return Ok(());
        }
        Err(match target.fields.last() {
            Some(last) => TypeError::FieldMismatch {
                detail: Box::new(FieldMismatch {
                    record: owner.to_owned(),
                    field: last.name.clone(),
                    expected: self.type_label(ty),
                    actual: self.type_label(actual),
                }),
                span: value.span,
            },
            None => self.mismatch_error(ty, actual, value.span),
        })
    }
    /// Checks `Name { field: value ... }`: the record exists, every field is set
    /// exactly once with a value of the declared type, and nothing else is set.
    fn record_init(
        &self,
        name: &str,
        initializers: &[FieldInit],
        span: Span,
    ) -> Result<Type, TypeError> {
        let (Some(declared), Some(&id)) = (self.records.get(name), self.record_ids.get(name))
        else {
            return Err(TypeError::UndefinedRecord {
                name: name.to_owned(),
                span: Span {
                    start: span.start,
                    end: span.start + name.len(),
                },
            });
        };
        let mut seen = HashSet::new();
        for init in initializers {
            let name_span = Span {
                start: init.span.start,
                end: init.span.start + init.name.len(),
            };
            let Some(field) = declared.iter().find(|field| field.name == init.name) else {
                return Err(TypeError::UnknownField {
                    record: name.to_owned(),
                    field: init.name.clone(),
                    span: name_span,
                });
            };
            if !seen.insert(init.name.as_str()) {
                return Err(TypeError::DuplicateInitializer {
                    record: name.to_owned(),
                    field: init.name.clone(),
                    span: name_span,
                });
            }
            let actual = self.infer_expression(&init.value)?;
            let expected = self.field_type(name, field)?;
            if !expected.accepts(&actual) {
                return Err(TypeError::FieldMismatch {
                    detail: Box::new(FieldMismatch {
                        record: name.to_owned(),
                        field: init.name.clone(),
                        expected: self.type_label(expected),
                        actual: self.type_label(actual),
                    }),
                    span: init.value.span,
                });
            }
        }
        let missing: Vec<String> = declared
            .iter()
            .filter(|field| !seen.contains(field.name.as_str()))
            .map(|field| field.name.clone())
            .collect();
        if !missing.is_empty() {
            return Err(TypeError::MissingFields {
                record: name.to_owned(),
                fields: missing,
                span,
            });
        }
        Ok(Type::Record(id))
    }
    pub fn check_statement(&mut self, stmt: &Stmt) -> Result<(), TypeError> {
        match &stmt.kind {
            StmtKind::Set { name, value } => {
                let ty = self.infer_expression(value)?;
                self.symbols
                    .declare(name.clone(), ty)
                    .map_err(|error| match error {
                        SymbolError::DuplicateDeclaration { name } => {
                            TypeError::DuplicateDeclaration {
                                name,
                                span: stmt.span,
                            }
                        }
                    })
            }
            StmtKind::Assign { name, value } => {
                let ty = self
                    .symbols
                    .lookup(name)
                    .ok_or_else(|| TypeError::UndefinedVariable {
                        name: name.clone(),
                        span: stmt.span,
                    })?;
                self.mismatch(ty, value)
            }
            StmtKind::AssignField { target, value } => self.assign_field(target, value),
            StmtKind::Show(expr) => {
                let ty = self.infer_expression(expr)?;
                if matches!(ty, Type::Result(_)) {
                    return Err(TypeError::InvalidFunction {
                        name: "show".into(),
                        message: "unwrap a successful result or inspect error_message before printing",
                        span: expr.span,
                    });
                }
                if matches!(ty, Type::Array { .. }) {
                    return Err(TypeError::InvalidArray {
                        message: "show requires a scalar, not an array",
                        span: expr.span,
                    });
                }
                if matches!(ty, Type::Record(_)) {
                    self.defer_unsupported(TypeError::UnsupportedRecordUse {
                        feature: "printing a record",
                        span: expr.span,
                    })?;
                }
                Ok(())
            }
            StmtKind::If {
                condition,
                then_body,
                else_body,
            } => {
                if self.infer_expression(condition)? != Type::Bool {
                    return Err(TypeError::InvalidCondition {
                        span: condition.span,
                    });
                }
                self.block(then_body)?;
                self.block(else_body)
            }
            StmtKind::While { condition, body } => {
                if self.infer_expression(condition)? != Type::Bool {
                    return Err(TypeError::InvalidCondition {
                        span: condition.span,
                    });
                }
                self.block(body)
            }
            StmtKind::Return(expr) => {
                let ty = self
                    .result
                    .ok_or(TypeError::InvalidReturn { span: stmt.span })?;
                self.mismatch(ty, expr)
            }
            StmtKind::Record { name, .. } if self.depth != 0 || self.result.is_some() => {
                Err(TypeError::RecordNotTopLevel {
                    name: name.clone(),
                    span: stmt.span,
                })
            }
            StmtKind::Record { .. } => Ok(()),
            StmtKind::Function {
                name,
                parameters,
                return_type,
                return_span,
                body,
            } => {
                if self.depth != 0 || self.result.is_some() {
                    return Err(TypeError::InvalidFunction {
                        name: name.clone(),
                        message: "functions must be top-level",
                        span: stmt.span,
                    });
                }
                let return_type = self.annotation_type(return_type, *return_span)?;
                let parameter_types = parameters
                    .iter()
                    .map(|parameter| self.annotation_type(&parameter.ty, parameter.ty_span))
                    .collect::<Result<Vec<_>, _>>()?;
                let previous = std::mem::take(&mut self.symbols);
                self.result = Some(return_type);
                let result = (|| {
                    for (parameter, ty) in parameters.iter().zip(parameter_types) {
                        self.symbols
                            .declare(parameter.name.clone(), ty)
                            .map_err(|_| TypeError::DuplicateDeclaration {
                                name: parameter.name.clone(),
                                span: parameter.span,
                            })?;
                    }
                    for statement in body {
                        self.check_statement(statement)?;
                    }
                    if !returns(body) {
                        return Err(TypeError::MissingReturn {
                            name: name.clone(),
                            span: stmt.span,
                        });
                    }
                    Ok(())
                })();
                self.symbols = previous;
                self.result = None;
                result
            }
        }
    }
    pub fn infer_expression(&self, expr: &Expr) -> Result<Type, TypeError> {
        match &expr.kind {
            ExprKind::RecordInit { name, fields } => self.record_init(name, fields, expr.span),
            ExprKind::FieldAccess { target, field } => self.field_access(target, field, expr.span),
            ExprKind::Integer(_) => Ok(Type::Int),
            ExprKind::Float(literal) => {
                if literal.parse::<f64>().is_ok_and(f64::is_finite) {
                    Ok(Type::Float)
                } else {
                    Err(TypeError::InvalidLiteral {
                        literal: literal.clone(),
                        span: expr.span,
                    })
                }
            }
            ExprKind::Bool(_) => Ok(Type::Bool),
            ExprKind::String(_) => Ok(Type::String),
            ExprKind::Error { payload, code } => {
                self.mismatch(Type::Int, code)?;
                Ok(Type::Result(*payload))
            }
            ExprKind::Identifier(name) => {
                self.symbols
                    .lookup(name)
                    .ok_or_else(|| TypeError::UndefinedVariable {
                        name: name.clone(),
                        span: expr.span,
                    })
            }
            ExprKind::Negate(inner) => {
                let ty = self.infer_expression(inner)?;
                if ty.is_numeric() {
                    Ok(ty)
                } else {
                    Err(TypeError::InvalidOperands {
                        left: ty,
                        right: ty,
                        span: expr.span,
                    })
                }
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.infer_expression(left)?;
                let right = self.infer_expression(right)?;
                if left == Type::String && right == Type::String {
                    match operator {
                        BinaryOperator::Add => return Ok(Type::String),
                        BinaryOperator::Equal | BinaryOperator::NotEqual => return Ok(Type::Bool),
                        _ => {}
                    }
                }
                if left.is_numeric() && right.is_numeric() {
                    if operator.is_comparison() {
                        return Ok(Type::Bool);
                    }
                    return Ok(if left == Type::Float || right == Type::Float {
                        Type::Float
                    } else {
                        Type::Int
                    });
                }
                if left == Type::Bool
                    && right == Type::Bool
                    && matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual)
                {
                    return Ok(Type::Bool);
                }
                if let (Type::Array { .. }, Type::Array { .. }) = (&left, &right)
                    && left == right
                    && matches!(operator, BinaryOperator::Add | BinaryOperator::Multiply)
                {
                    return Ok(left);
                }
                Err(TypeError::InvalidOperands {
                    left,
                    right,
                    span: expr.span,
                })
            }
            ExprKind::Array(elements) => {
                if elements.is_empty() || elements.len() > MAX_ARRAY_LEN {
                    return Err(TypeError::InvalidArray {
                        message: "array literals require 1..=256 elements",
                        span: expr.span,
                    });
                }
                let element = match self.infer_expression(&elements[0])? {
                    Type::Int => NumericType::Int,
                    Type::Float => NumericType::Float,
                    _ => {
                        return Err(TypeError::InvalidArray {
                            message: "arrays require Int or Float elements",
                            span: expr.span,
                        });
                    }
                };
                for value in &elements[1..] {
                    if self.infer_expression(value)? != element.scalar() {
                        return Err(TypeError::InvalidArray {
                            message: "array elements must have identical numeric types",
                            span: value.span,
                        });
                    }
                }
                Ok(Type::Array {
                    element,
                    len: elements.len(),
                })
            }
            ExprKind::Index { array, index } => {
                let ty = self.infer_expression(array)?;
                if self.infer_expression(index)? != Type::Int {
                    return Err(TypeError::InvalidArray {
                        message: "array index must be Int",
                        span: index.span,
                    });
                }
                if let Type::Array { element, .. } = ty {
                    Ok(element.scalar())
                } else {
                    Err(TypeError::InvalidArray {
                        message: "indexing requires an array",
                        span: array.span,
                    })
                }
            }
            ExprKind::Call { name, arguments } => self.call(name, arguments, expr.span),
        }
    }
    fn call(&self, name: &str, arguments: &[Expr], span: Span) -> Result<Type, TypeError> {
        let types = arguments
            .iter()
            .map(|expr| self.infer_expression(expr))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(checked) = file_call(name, &types) {
            return checked.map_err(|message| TypeError::InvalidFunction {
                name: name.to_owned(),
                message,
                span,
            });
        }
        let invalid = || TypeError::InvalidFunction {
            name: name.to_owned(),
            message: "invalid argument count or types",
            span,
        };
        match (name, types.as_slice()) {
            ("input", [] | [Type::String]) => Ok(Type::String),
            ("try_input", [] | [Type::String]) => Ok(Type::Result(ScalarType::String)),
            ("parse_int", [Type::String]) => Ok(Type::Result(ScalarType::Int)),
            ("parse_float", [Type::String]) => Ok(Type::Result(ScalarType::Float)),
            ("to_string", [ty]) if ScalarType::from_type(ty).is_some() => Ok(Type::String),
            ("ok", [ty]) if ScalarType::from_type(ty).is_some() => Ok(Type::Result(
                ScalarType::from_type(ty).expect("checked scalar"),
            )),
            ("is_ok" | "is_eof", [Type::Result(_)]) => Ok(Type::Bool),
            ("error_code", [Type::Result(_)]) => Ok(Type::Int),
            ("error_message", [Type::Result(_)]) => Ok(Type::String),
            ("unwrap", [Type::Result(payload)]) => Ok(payload.ty()),
            (
                "input" | "try_input" | "parse_int" | "parse_float" | "to_string" | "ok" | "is_ok"
                | "is_eof" | "error_code" | "error_message" | "unwrap",
                _,
            ) => Err(invalid()),
            ("length", [Type::Array { .. }]) => Ok(Type::Int),
            ("length", [Type::String]) => Ok(Type::Int),
            ("sum", [Type::Array { element, .. }]) => Ok(element.scalar()),
            ("sqrt", [Type::Float]) => Ok(Type::Float),
            ("abs", [ty]) if ty.is_numeric() => Ok(*ty),
            ("min" | "max", [left, right]) if left.is_numeric() && right.is_numeric() => {
                Ok(if *left == Type::Float || *right == Type::Float {
                    Type::Float
                } else {
                    Type::Int
                })
            }
            ("length" | "sum" | "sqrt" | "abs" | "min" | "max", _) => Err(invalid()),
            _ => {
                let signature =
                    self.functions
                        .get(name)
                        .ok_or_else(|| TypeError::InvalidFunction {
                            name: name.to_owned(),
                            message: "undefined function",
                            span,
                        })?;
                if signature.parameters.len() != types.len()
                    || !signature
                        .parameters
                        .iter()
                        .zip(&types)
                        .all(|(expected, actual)| expected.accepts(actual))
                {
                    return Err(invalid());
                }
                Ok(signature.result)
            }
        }
    }
}

fn returns(statements: &[Stmt]) -> bool {
    statements.iter().any(|statement| match &statement.kind {
        StmtKind::Return(_) => true,
        StmtKind::If {
            then_body,
            else_body,
            ..
        } => returns(then_body) && returns(else_body),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::Parser;

    #[test]
    fn assignment_diagnostic_preserves_rhs_span() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        let statements = parse_program("assign x as 1\nreassign x as true");
        assert_eq!(
            checker.check_program(&statements),
            Err(TypeError::Mismatch {
                expected: Type::Int,
                actual: Type::Bool,
                span: Span { start: 28, end: 32 },
            })
        );
    }

    #[test]
    fn failing_block_restores_symbol_scope() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        let statements =
            parse_program("assign x as 1\nif true {\nassign x as 2.0\nshow missing\n}");
        assert!(checker.check_program(&statements).is_err());
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
    }

    #[test]
    fn function_signature_rejects_wrong_return_type() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        assert!(matches!(
            checker.check_program(&parse_program("function f() be bool { return 1 }")),
            Err(TypeError::Mismatch {
                expected: Type::Bool,
                actual: Type::Int,
                ..
            })
        ));
    }

    fn parse_expression(source: &str) -> Expr {
        let tokens = tokenize(source).expect("Expression should tokenize");
        Parser::new(tokens)
            .parse_expression()
            .expect("Expression should parse")
    }

    fn parse_statement(source: &str) -> Stmt {
        let tokens = tokenize(source).expect("Statement should tokenize");
        Parser::new(tokens)
            .parse_statement()
            .expect("Statement should parse")
    }

    fn parse_program(source: &str) -> Vec<Stmt> {
        let tokens = tokenize(source).expect("Program should tokenize");
        Parser::new(tokens)
            .parse_program()
            .expect("Program should parse")
    }

    #[test]
    fn checks_valid_program() {
        let statements = parse_program("assign x as 10\nassign y as x + 20\nshow y");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(checker.check_program(&statements), Ok(()));
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        assert_eq!(checker.symbols.lookup("y"), Some(Type::Int));
    }

    #[test]
    fn checks_empty_program() {
        let statements = parse_program("");
        let mut symbols = SymbolTable::new();
        symbols
            .declare("existing".to_owned(), Type::Int)
            .expect("Declaration should succeed");
        let mut checker = TypeChecker::new(symbols);

        assert_eq!(checker.check_program(&statements), Ok(()));
        assert_eq!(checker.symbols.lookup("existing"), Some(Type::Int));
        assert_eq!(
            TypeChecker::new(SymbolTable::new()).check_program(&[]),
            Ok(())
        );
    }

    #[test]
    fn checks_program_declaration_using_previous_variable() {
        let statements = parse_program("assign x as 10\nassign y as x\nassign z as y + x");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(checker.check_program(&statements), Ok(()));
        for name in ["x", "y", "z"] {
            assert_eq!(checker.symbols.lookup(name), Some(Type::Int));
        }
    }

    #[test]
    fn rejects_program_with_undefined_variable() {
        let statements = parse_program("assign x as 10\nshow unknown");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_program(&statements),
            Err(TypeError::UndefinedVariable {
                name: "unknown".to_owned(),
                span: Span { start: 20, end: 27 },
            })
        );
    }

    #[test]
    fn rejects_program_with_duplicate_declaration() {
        let statements = parse_program("assign x as 10\nassign x as 20\nassign later as 30");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_program(&statements),
            Err(TypeError::DuplicateDeclaration {
                name: "x".to_owned(),
                span: Span { start: 15, end: 29 },
            })
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        assert_eq!(checker.symbols.lookup("later"), None);
    }

    #[test]
    fn rejects_program_reference_before_declaration() {
        let statements = parse_program("show x\nassign x as 10");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_program(&statements),
            Err(TypeError::UndefinedVariable {
                name: "x".to_owned(),
                span: Span { start: 5, end: 6 },
            })
        );
        assert_eq!(checker.symbols.lookup("x"), None);
    }

    #[test]
    fn program_stops_at_first_error_and_keeps_previous_declarations() {
        let statements =
            parse_program("assign x as 10\nassign y as unknown\nassign later as 20\nshow other");
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_program(&statements),
            Err(TypeError::UndefinedVariable {
                name: "unknown".to_owned(),
                span: Span { start: 27, end: 34 },
            })
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        assert_eq!(checker.symbols.lookup("y"), None);
        assert_eq!(checker.symbols.lookup("later"), None);
    }

    #[test]
    fn checks_declaration_and_subsequent_reference() {
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_statement(&parse_statement("assign x as 10")),
            Ok(())
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        assert_eq!(
            checker.infer_expression(&parse_expression("x")),
            Ok(Type::Int)
        );
    }

    #[test]
    fn checks_declaration_using_previously_declared_variable() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        checker
            .check_statement(&parse_statement("assign x as 10"))
            .expect("First declaration should succeed");

        assert_eq!(
            checker.check_statement(&parse_statement("assign y as x + 20")),
            Ok(())
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        assert_eq!(checker.symbols.lookup("y"), Some(Type::Int));
    }

    #[test]
    fn checks_show_without_changing_symbols() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        checker
            .check_statement(&parse_statement("assign x as 10"))
            .expect("Declaration should succeed");

        for source in ["show 42", "show x", "show x + 20"] {
            assert_eq!(checker.check_statement(&parse_statement(source)), Ok(()));
            assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
        }
        // Output does not create a declaration or replace the existing one.
        assert_eq!(checker.symbols.lookup("unknown"), None);
        assert_eq!(
            checker.check_statement(&parse_statement("assign x as 20")),
            Err(TypeError::DuplicateDeclaration {
                name: "x".to_owned(),
                span: Span { start: 0, end: 14 },
            })
        );
    }

    #[test]
    fn rejects_duplicate_declaration_with_statement_span() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        checker
            .check_statement(&parse_statement("assign x as 10"))
            .expect("First declaration should succeed");

        assert_eq!(
            checker.check_statement(&parse_statement("  assign x as 20")),
            Err(TypeError::DuplicateDeclaration {
                name: "x".to_owned(),
                span: Span { start: 2, end: 16 },
            })
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
    }

    #[test]
    fn rejects_show_with_undefined_variable() {
        let mut checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.check_statement(&parse_statement("show unknown + 10")),
            Err(TypeError::UndefinedVariable {
                name: "unknown".to_owned(),
                span: Span { start: 5, end: 12 },
            })
        );
        assert_eq!(checker.symbols.lookup("unknown"), None);
    }

    #[test]
    fn failed_initializer_does_not_register_variable() {
        let mut checker = TypeChecker::new(SymbolTable::new());

        for (source, name, undefined, span) in [
            (
                "assign x as unknown + 10",
                "x",
                "unknown",
                Span { start: 12, end: 19 },
            ),
            ("assign y as y + 10", "y", "y", Span { start: 12, end: 13 }),
        ] {
            assert_eq!(
                checker.check_statement(&parse_statement(source)),
                Err(TypeError::UndefinedVariable {
                    name: undefined.to_owned(),
                    span,
                })
            );
            assert_eq!(checker.symbols.lookup(name), None);
        }

        assert_eq!(
            checker.check_statement(&parse_statement("assign x as 10")),
            Ok(())
        );
        assert_eq!(checker.symbols.lookup("x"), Some(Type::Int));
    }

    #[test]
    fn infers_integer_literal_type() {
        let checker = TypeChecker::new(SymbolTable::new());
        let expression = parse_expression("42");

        assert_eq!(checker.infer_expression(&expression), Ok(Type::Int));
    }

    #[test]
    fn infers_declared_identifier_type() {
        let mut symbols = SymbolTable::new();
        symbols
            .declare("total".to_owned(), Type::Int)
            .expect("Declaration should succeed");
        let checker = TypeChecker::new(symbols);

        assert_eq!(
            checker.infer_expression(&parse_expression("total")),
            Ok(Type::Int)
        );
    }

    #[test]
    fn reports_undefined_identifier_with_span() {
        let checker = TypeChecker::new(SymbolTable::new());

        assert_eq!(
            checker.infer_expression(&parse_expression("  unknown")),
            Err(TypeError::UndefinedVariable {
                name: "unknown".to_owned(),
                span: Span { start: 2, end: 9 },
            })
        );
    }

    #[test]
    fn infers_integer_addition_type() {
        let checker = TypeChecker::new(SymbolTable::new());

        for source in ["10 + 20", "10 + 20 + 30"] {
            assert_eq!(
                checker.infer_expression(&parse_expression(source)),
                Ok(Type::Int)
            );
        }
    }

    #[test]
    fn infers_addition_with_declared_identifier() {
        let mut symbols = SymbolTable::new();
        symbols
            .declare("x".to_owned(), Type::Int)
            .expect("Declaration should succeed");
        let checker = TypeChecker::new(symbols);

        for source in ["x + 20", "20 + x", "x + x + 20"] {
            assert_eq!(
                checker.infer_expression(&parse_expression(source)),
                Ok(Type::Int)
            );
        }
    }

    #[test]
    fn propagates_undefined_addition_operand_errors() {
        let checker = TypeChecker::new(SymbolTable::new());

        for (source, span) in [
            ("unknown + 10", Span { start: 0, end: 7 }),
            ("10 + unknown", Span { start: 5, end: 12 }),
            ("10 + 20 + unknown", Span { start: 10, end: 17 }),
            ("10 + unknown + 20", Span { start: 5, end: 12 }),
        ] {
            assert_eq!(
                checker.infer_expression(&parse_expression(source)),
                Err(TypeError::UndefinedVariable {
                    name: "unknown".to_owned(),
                    span,
                })
            );
        }
    }
    #[test]
    fn checks_record_typing_and_errors() {
        let mut checker = TypeChecker::new(SymbolTable::new());
        let program = parse_program(
            "record Point {\n x: int\n y: int\n}\nassign p as Point {\n x: 10\n y: 20\n}\nreassign p.x as 30",
        );
        assert_eq!(checker.check_program(&program), Ok(()));

        // Missing field
        let mut checker2 = TypeChecker::new(SymbolTable::new());
        let bad_init =
            parse_program("record Point {\n x: int\n y: int\n}\nassign p as Point { x: 10 }");
        assert!(matches!(
            checker2.check_program(&bad_init),
            Err(TypeError::MissingFields { .. })
        ));

        // Unknown field access
        let mut checker3 = TypeChecker::new(SymbolTable::new());
        let bad_access = parse_program(
            "record Point {\n x: int\n y: int\n}\nassign p as Point {\n x: 10\n y: 20\n}\nshow p.z",
        );
        assert!(matches!(
            checker3.check_program(&bad_access),
            Err(TypeError::UnknownField { .. })
        ));
    }
}
