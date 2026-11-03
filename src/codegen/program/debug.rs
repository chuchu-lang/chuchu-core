//! LLVM debug metadata for the production native lowering.
use super::*;
use crate::optimization::{BuildOptions, Optimization};
use crate::source::SourceFile;
use inkwell::debug_info::{
    AsDIScope, DICompileUnit, DIFlags, DIFlagsConstants, DILocation, DIScope, DIType,
    DWARFEmissionKind, DWARFSourceLanguage, DebugInfoBuilder, debug_metadata_version,
};
use inkwell::module::FlagBehavior;
use inkwell::targets::TargetData;
use inkwell::values::AsValueRef;

pub(super) struct DebugState<'ctx> {
    pub builder: DebugInfoBuilder<'ctx>,
    context: &'ctx Context,
    unit: DICompileUnit<'ctx>,
    source: SourceFile,
    scopes: Vec<DIScope<'ctx>>,
    types: Vec<(Type, DIType<'ctx>)>,
    data: TargetData,
    optimized: bool,
}

impl<'ctx> DebugState<'ctx> {
    pub fn new(
        context: &'ctx Context,
        module: &Module<'ctx>,
        source: &SourceFile,
        options: &BuildOptions,
    ) -> Result<Self, CodegenError> {
        let machine = crate::optimization::target_machine(module, options)
            .map_err(|error| CodegenError::Verification(error.to_string()))?;
        module.add_basic_value_flag(
            "Debug Info Version",
            FlagBehavior::Warning,
            context
                .i32_type()
                .const_int(u64::from(debug_metadata_version()), false),
        );
        if cfg!(target_os = "windows") {
            module.add_basic_value_flag(
                "CodeView",
                FlagBehavior::Warning,
                context.i32_type().const_int(1, false),
            );
        } else {
            module.add_basic_value_flag(
                "Dwarf Version",
                FlagBehavior::Warning,
                context.i32_type().const_int(4, false),
            );
        }
        let path = source.path();
        let filename = path.file_name().and_then(|s| s.to_str()).ok_or(invalid(
            "Debug source path must be UTF-8",
            Span { start: 0, end: 0 },
        ))?;
        let directory = path.parent().and_then(|s| s.to_str()).ok_or(invalid(
            "Debug source directory must be UTF-8",
            Span { start: 0, end: 0 },
        ))?;
        let optimized = options.optimization != Optimization::O0;
        // C is a debugger compatibility encoding for the current primitive types,
        // not an assertion that LLDB understands Chuchu expressions.
        let (builder, unit) = module.create_debug_info_builder(
            true,
            DWARFSourceLanguage::C,
            filename,
            directory,
            concat!("Cheeku ", env!("CARGO_PKG_VERSION")),
            optimized,
            "",
            0,
            "",
            DWARFEmissionKind::Full,
            0,
            false,
            false,
            "",
            "",
        );
        Ok(Self {
            context,
            builder,
            unit,
            source: source.clone(),
            scopes: Vec::new(),
            types: Vec::new(),
            data: machine.get_target_data(),
            optimized,
        })
    }

    fn position(&self, span: Span) -> Result<crate::source::SourcePosition, CodegenError> {
        self.source
            .position(span.start)
            .filter(|p| p.column <= 65535)
            .ok_or(invalid(
                "Source position exceeds LLVM debug location limits",
                span,
            ))
    }

    fn ty(&mut self, ty: Type, llvm: BasicTypeEnum<'ctx>) -> Result<DIType<'ctx>, CodegenError> {
        if let Some((_, debug)) = self.types.iter().find(|(existing, _)| *existing == ty) {
            return Ok(*debug);
        }
        let bits = self.data.get_abi_size(&llvm) * 8;
        let debug = match ty {
            Type::Result(payload) => {
                let structure = llvm.into_struct_type();
                let fields = structure.get_field_types();
                let value_type = self.ty(payload.ty(), fields[1])?;
                let code_type = self
                    .builder
                    .create_basic_type("ErrorCode", 32, 0x05, DIFlags::ZERO)
                    .map_err(|error| CodegenError::Verification(error.to_string()))?
                    .as_type();
                let scope = self.unit.as_debug_info_scope();
                let mut members = Vec::new();
                for (index, (name, ty)) in [("code", code_type), ("value", value_type)]
                    .into_iter()
                    .enumerate()
                {
                    members.push(
                        self.builder
                            .create_member_type(
                                scope,
                                name,
                                self.unit.get_file(),
                                0,
                                self.data.get_abi_size(&fields[index]) * 8,
                                self.data.get_abi_alignment(&fields[index]) * 8,
                                self.data
                                    .offset_of_element(&structure, index as u32)
                                    .ok_or(invalid(
                                        "Invalid result layout",
                                        Span { start: 0, end: 0 },
                                    ))?
                                    * 8,
                                DIFlags::ZERO,
                                ty,
                            )
                            .as_type(),
                    );
                }
                self.builder
                    .create_struct_type(
                        scope,
                        &format!("result<{:?}>", payload),
                        self.unit.get_file(),
                        0,
                        bits,
                        self.data.get_abi_alignment(&llvm) * 8,
                        DIFlags::ZERO,
                        None,
                        &members,
                        0,
                        None,
                        "",
                    )
                    .as_type()
            }
            Type::String => {
                let character = self
                    .builder
                    .create_basic_type("char", 8, 0x06, DIFlags::ZERO)
                    .map_err(|error| CodegenError::Verification(error.to_string()))?
                    .as_type();
                self.builder
                    .create_pointer_type(
                        "String",
                        character,
                        bits,
                        self.data.get_abi_alignment(&llvm) * 8,
                        AddressSpace::default(),
                    )
                    .as_type()
            }
            Type::Array { element, len } => {
                let BasicTypeEnum::ArrayType(array) = llvm else {
                    unreachable!("array lowering type")
                };
                let inner = self.ty(element.scalar(), array.get_element_type())?;
                let subscript = 0..len as i64;
                self.builder
                    .create_array_type(
                        inner,
                        bits,
                        self.data.get_abi_alignment(&llvm) * 8,
                        std::slice::from_ref(&subscript),
                    )
                    .as_type()
            }
            Type::Record(ref name) => {
                let BasicTypeEnum::StructType(_structure) = llvm else {
                    unreachable!("record lowering type")
                };
                let scope = self.unit.as_debug_info_scope();
                self.builder
                    .create_struct_type(
                        scope,
                        name,
                        self.unit.get_file(),
                        0,
                        bits,
                        self.data.get_abi_alignment(&llvm) * 8,
                        DIFlags::ZERO,
                        None,
                        &[],
                        0,
                        None,
                        "",
                    )
                    .as_type()
            }
            _ => {
                let (name, encoding) = match ty {
                    Type::Int => ("Int", 0x05),     // DW_ATE_signed
                    Type::Float => ("Float", 0x04), // DW_ATE_float
                    Type::Bool => ("Bool", 0x02),   // DW_ATE_boolean; ABI storage is a byte
                    _ => unreachable!(),
                };
                self.builder
                    .create_basic_type(name, bits, encoding, DIFlags::ZERO)
                    .map_err(|error| CodegenError::Verification(error.to_string()))?
                    .as_type()
            }
        };
        self.types.push((ty.clone(), debug));
        Ok(debug)
    }

    pub fn function(
        &mut self,
        function: FunctionValue<'ctx>,
        name: &str,
        span: Span,
        result: Option<(Type, BasicTypeEnum<'ctx>)>,
        parameters: &[(Type, BasicTypeEnum<'ctx>)],
    ) -> Result<(), CodegenError> {
        let pos = self.position(span)?;
        let result = if let Some((ty, llvm)) = result {
            Some(self.ty(ty, llvm)?)
        } else {
            Some(
                self.builder
                    .create_basic_type("main status", 32, 0x05, DIFlags::ZERO)
                    .map_err(|error| CodegenError::Verification(error.to_string()))?
                    .as_type(),
            )
        };
        let params = parameters
            .iter()
            .map(|(ty, llvm)| self.ty(ty.clone(), *llvm))
            .collect::<Result<Vec<_>, _>>()?;
        let signature = self.builder.create_subroutine_type(
            self.unit.get_file(),
            result,
            &params,
            DIFlags::ZERO,
        );
        let linkage = function.get_name().to_string_lossy();
        let program = self.builder.create_function(
            self.unit.as_debug_info_scope(),
            name,
            Some(&linkage),
            self.unit.get_file(),
            pos.line,
            signature,
            false,
            true,
            pos.line,
            DIFlags::ZERO,
            self.optimized,
        );
        function.set_subprogram(program);
        self.scopes.clear();
        self.scopes.push(program.as_debug_info_scope());
        Ok(())
    }

    pub fn location(
        &self,
        context: &'ctx Context,
        span: Span,
    ) -> Result<DILocation<'ctx>, CodegenError> {
        let pos = self.position(span)?;
        let scope = *self
            .scopes
            .last()
            .ok_or(invalid("Missing debug scope", span))?;
        Ok(self
            .builder
            .create_debug_location(context, pos.line, pos.column, scope, None))
    }

    pub fn enter_scope(&mut self, span: Span) -> Result<(), CodegenError> {
        let pos = self.position(span)?;
        let parent = *self
            .scopes
            .last()
            .ok_or(invalid("Missing debug scope", span))?;
        self.scopes.push(
            self.builder
                .create_lexical_block(parent, self.unit.get_file(), pos.line, pos.column)
                .as_debug_info_scope(),
        );
        Ok(())
    }
    pub fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn variable(
        &mut self,
        name: &str,
        binding: Binding<'ctx>,
        llvm: BasicTypeEnum<'ctx>,
        span: Span,
        argument: Option<u32>,
        block: inkwell::basic_block::BasicBlock<'ctx>,
    ) -> Result<(), CodegenError> {
        let pos = self.position(span)?;
        let ty = self.ty(binding.ty.clone(), llvm)?;
        let scope = *self
            .scopes
            .last()
            .ok_or(invalid("Missing debug scope", span))?;
        let var = if let Some(index) = argument {
            self.builder.create_parameter_variable(
                scope,
                name,
                index,
                self.unit.get_file(),
                pos.line,
                ty,
                true,
                DIFlags::ZERO,
            )
        } else {
            self.builder.create_auto_variable(
                scope,
                name,
                self.unit.get_file(),
                pos.line,
                ty,
                true,
                DIFlags::ZERO,
                self.data.get_abi_alignment(&llvm) * 8,
            )
        };
        let location = self.location(self.context, span)?;
        let expression = self.builder.create_expression(vec![]);
        // SAFETY: All handles belong to this LLVM context/module, storage has the
        // declared type and block is the live unterminated insertion block. LLVM
        // 23 returns a DbgRecord here. Inkwell 0.11 incorrectly wraps that record
        // as an InstructionValue, so use the re-exported C API and ignore its
        // return value rather than constructing an invalid instruction wrapper.
        unsafe {
            inkwell::llvm_sys::debuginfo::LLVMDIBuilderInsertDeclareRecordAtEnd(
                self.builder.as_mut_ptr(),
                binding.pointer.as_value_ref(),
                var.as_mut_ptr(),
                expression.as_mut_ptr(),
                location.as_mut_ptr(),
                block.as_mut_ptr(),
            );
        }
        Ok(())
    }
}
