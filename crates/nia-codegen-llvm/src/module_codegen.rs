// SPDX-License-Identifier: GPL-3.0-or-later
mod declarations;
mod static_init;
mod types;

pub(crate) use types::{AbiParam, AbiReturn};

use std::collections::HashMap;

use crate::function_codegen::FunctionCodegen;
use crate::output::LlvmCodegenOptions;
use crate::program_index::ProgramIndex;
use nia_backend_ir::{BackendFunction, BackendModule};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_layout::TypeLayout;
use nia_llvm::{
    Context, LlvmError,
    target::TargetMachine,
    types::StructType,
    values::{FunctionValue, GlobalValue},
};
use nia_mangle::mangle_base_symbol;
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};

pub(super) struct ModuleCodegen<'ctx, 'a> {
    pub(super) context: &'ctx Context,
    pub(super) source: &'a BackendModule,
    pub(super) program: &'a ProgramIndex<'a>,
    pub(super) module: nia_llvm::module::Module<'ctx>,
    pub(super) options: LlvmCodegenOptions,
    pub(super) structs: HashMap<GlobalDefId, StructType<'ctx>>,
    pub(super) unions: HashMap<GlobalDefId, StructType<'ctx>>,
    pub(super) struct_instances: HashMap<(GlobalDefId, Vec<InternedTyId>), StructType<'ctx>>,
    pub(super) union_instances: HashMap<(GlobalDefId, Vec<InternedTyId>), StructType<'ctx>>,
    pub(super) functions: HashMap<GlobalDefId, FunctionValue<'ctx>>,
    pub(super) function_instances:
        HashMap<(GlobalDefId, ModuleId, Vec<InternedTyId>), FunctionValue<'ctx>>,
    pub(super) globals: HashMap<GlobalDefId, GlobalValue<'ctx>>,
}

impl<'ctx, 'a> ModuleCodegen<'ctx, 'a> {
    pub(super) fn new(
        context: &'ctx Context,
        source: &'a BackendModule,
        program: &'a ProgramIndex<'a>,
        options: LlvmCodegenOptions,
    ) -> Result<Self, Diagnostic> {
        Ok(Self {
            context,
            source,
            program,
            module: context
                .create_module(&source.name)
                .map_err(Self::diagnostic_from_llvm_error)?,
            options,
            structs: HashMap::new(),
            unions: HashMap::new(),
            struct_instances: HashMap::new(),
            union_instances: HashMap::new(),
            functions: HashMap::new(),
            function_instances: HashMap::new(),
            globals: HashMap::new(),
        })
    }

    pub(super) fn emit_ir(&mut self) -> Result<String, Diagnostic> {
        self.emit_module()?;
        self.module
            .ir_string()
            .map_err(Self::diagnostic_from_llvm_error)
    }

    pub(super) fn emit_object(&mut self, target: &TargetMachine) -> Result<Vec<u8>, Diagnostic> {
        self.emit_module()?;
        target
            .emit_object(&self.module)
            .map_err(Self::diagnostic_from_llvm_error)
    }

    fn emit_module(&mut self) -> Result<(), Diagnostic> {
        self.check_hosted_entry_signatures()?;
        self.declare_structs()?;
        self.define_struct_bodies()?;
        self.declare_functions()?;
        self.declare_globals()?;
        self.emit_function_bodies()?;
        self.module
            .verify()
            .map_err(Self::diagnostic_from_llvm_error)
    }

    fn check_hosted_entry_signatures(&self) -> Result<(), Diagnostic> {
        if !self.options.hosted_entry || self.options.root_module != Some(self.source.id) {
            return Ok(());
        }
        for function in &self.source.functions {
            if function.name != "main" || function.is_extern {
                continue;
            }
            if !self.is_valid_hosted_entry_signature(function) {
                return Err(self.error(
                    function.span,
                    "hosted entry `main` must be `fn main() i32` or `fn main(argc: i32, argv: &const &const u8) i32`",
                ));
            }
        }
        Ok(())
    }

    fn is_valid_hosted_entry_signature(&self, function: &BackendFunction) -> bool {
        if !function.generics.is_empty()
            || function.is_variadic
            || function.return_type != self.source.interner.primitive(PrimitiveTy::I32)
        {
            return false;
        }
        match function.params.as_slice() {
            [] => true,
            [argc, argv] => {
                argc.ty == self.source.interner.primitive(PrimitiveTy::I32)
                    && self.is_const_argv_ty(argv.ty)
            }
            _ => false,
        }
    }

    pub(super) fn layout_of(&self, ty: InternedTyId) -> Option<TypeLayout> {
        let owner = self.program.module(ty.interner_id)?;
        if let Some(layout) = owner
            .layouts
            .types
            .iter()
            .find_map(|(candidate, layout)| (*candidate == ty).then_some(layout.clone()))
        {
            return Some(layout);
        }
        let Some(TyKind::Nominal { def_id, args }) = owner.interner.get(ty) else {
            return None;
        };
        let def_owner = self.program.module(def_id.module_id)?;
        if args.is_empty() {
            def_owner
                .layouts
                .structs
                .iter()
                .find_map(|(candidate, layout)| {
                    (*candidate == *def_id).then_some(layout.layout.clone())
                })
                .or_else(|| {
                    def_owner
                        .layouts
                        .unions
                        .iter()
                        .find_map(|(candidate, layout)| {
                            (*candidate == *def_id).then_some(layout.layout.clone())
                        })
                })
        } else {
            def_owner
                .layouts
                .struct_instances
                .iter()
                .find_map(|(key, layout)| {
                    (key.def_id == *def_id && self.same_type_args(&key.args, args))
                        .then_some(layout.layout.clone())
                })
                .or_else(|| {
                    def_owner
                        .layouts
                        .union_instances
                        .iter()
                        .find_map(|(key, layout)| {
                            (key.def_id == *def_id && self.same_type_args(&key.args, args))
                                .then_some(layout.layout.clone())
                        })
                })
        }
    }

    fn is_const_argv_ty(&self, ty: InternedTyId) -> bool {
        let Some(TyKind::Pointer {
            is_const: true,
            elem: argv_elem,
        }) = self.ty_kind(ty)
        else {
            return false;
        };
        let Some(TyKind::Pointer {
            is_const: true,
            elem: byte_elem,
        }) = self.ty_kind(*argv_elem)
        else {
            return false;
        };
        *byte_elem == self.source.interner.primitive(PrimitiveTy::U8)
    }

    pub(super) fn integer_llvm_type(
        &self,
        primitive: PrimitiveTy,
        span: Span,
    ) -> Result<nia_llvm::types::IntType<'ctx>, Diagnostic> {
        match primitive {
            PrimitiveTy::I8 | PrimitiveTy::U8 => Ok(self.context.i8_type()),
            PrimitiveTy::I16 | PrimitiveTy::U16 => Ok(self.context.i16_type()),
            PrimitiveTy::I32 | PrimitiveTy::U32 | PrimitiveTy::Char => Ok(self.context.i32_type()),
            PrimitiveTy::I64 | PrimitiveTy::U64 | PrimitiveTy::Isize | PrimitiveTy::Usize => {
                Ok(self.context.i64_type())
            }
            PrimitiveTy::I128 | PrimitiveTy::U128 => Ok(self.context.i128_type()),
            PrimitiveTy::Bool => Ok(self.context.bool_type()),
            PrimitiveTy::F32 | PrimitiveTy::F64 | PrimitiveTy::Void | PrimitiveTy::Never => {
                Err(self.error(span, "expected integer primitive type"))
            }
        }
    }

    fn emit_function_bodies(&mut self) -> Result<(), Diagnostic> {
        for function in &self.source.functions {
            if function.is_extern && function.body.is_none() {
                continue;
            }
            if let Some(body) = &function.body {
                let Some(llvm_function) = self.functions.get(&function.def_id).copied() else {
                    return Err(self.error(
                        function.span,
                        format!("missing function `{}`", function.name),
                    ));
                };
                let mut codegen = FunctionCodegen::new(self, function, llvm_function);
                if let Some(control_body) = &function.control_body
                    && FunctionCodegen::can_emit_control_body(control_body)
                {
                    codegen.emit_control_body(control_body)?;
                } else {
                    codegen.emit_body(body)?;
                }
            }
        }
        for instance in &self.source.function_instances {
            if instance.is_extern && instance.body.is_none() {
                continue;
            }
            if let Some(body) = &instance.body {
                let Some(llvm_function) =
                    self.function_instance_value(instance.def_id, &instance.args)
                else {
                    return Err(self.error(instance.span, "missing function instance"));
                };
                let function = BackendFunction {
                    def_id: instance.def_id,
                    name: instance.name.clone(),
                    generics: Vec::new(),
                    params: instance.params.clone(),
                    return_type: instance.return_type,
                    is_extern: instance.is_extern,
                    is_variadic: instance.is_variadic,
                    body: instance.body.clone(),
                    control_body: instance.control_body.clone(),
                    span: instance.span,
                };
                let mut codegen = FunctionCodegen::new(self, &function, llvm_function);
                if let Some(control_body) = &function.control_body
                    && FunctionCodegen::can_emit_control_body(control_body)
                {
                    codegen.emit_control_body(control_body)?;
                } else {
                    codegen.emit_body(body)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn interner(&self) -> &TyInterner {
        &self.source.interner
    }

    fn symbol_name(&self, def_id: GlobalDefId, name: &str) -> String {
        mangle_base_symbol(def_id, name)
    }

    fn struct_symbol_name(&self, def_id: GlobalDefId, name: &str) -> String {
        self.symbol_name(def_id, name)
    }

    fn function_symbol_name(&self, function: &BackendFunction) -> String {
        if function.is_extern {
            function.name.clone()
        } else if self.is_hosted_entry(function) {
            "main".to_string()
        } else {
            self.symbol_name(function.def_id, &function.name)
        }
    }

    fn is_hosted_entry(&self, function: &BackendFunction) -> bool {
        self.options.hosted_entry
            && self.options.root_module == Some(function.def_id.module_id)
            && function.name == "main"
            && function.generics.is_empty()
    }

    fn global_symbol_name(&self, global: &nia_backend_ir::BackendGlobal) -> String {
        if global.is_extern {
            global.name.clone()
        } else {
            self.symbol_name(global.def_id, &global.name)
        }
    }

    pub(super) fn error(&self, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(span, message)
    }

    pub(super) fn diagnostic_from_llvm_error(error: LlvmError) -> Diagnostic {
        error.diagnostic()
    }
}
