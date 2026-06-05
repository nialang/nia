// SPDX-License-Identifier: GPL-3.0-or-later
mod declarations;
mod static_init;
mod trait_objects;
mod types;

pub(crate) use types::{AbiParam, AbiReturn};

use std::{cell::RefCell, collections::HashMap};

use crate::function_codegen::{FunctionCodegen, FunctionCodegenInput};
use crate::program_index::ProgramIndex;
use nia_backend_ir::{BackendFunction, BackendLayouts, BackendModule};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_layout::TypeLayout;
use nia_llvm::{
    Context, LlvmError,
    target::TargetMachine,
    types::StructType,
    values::{FunctionValue, GlobalValue},
};
use nia_mangle::{mangle_base_symbol, mangle_type_with};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};

type InstanceKey = (GlobalDefId, Vec<InternedTyId>);
type InstanceTypeLookup<'ctx> = RefCell<HashMap<InstanceKey, Option<StructType<'ctx>>>>;
type FunctionInstanceLookup<'ctx> = RefCell<HashMap<InstanceKey, Option<FunctionValue<'ctx>>>>;
type AggregateLayoutLookup = RefCell<HashMap<InstanceKey, Option<Vec<InternedTyId>>>>;

struct FunctionSignature<'a, P> {
    param_tys: P,
    return_type: InternedTyId,
    is_extern: bool,
    is_variadic: bool,
    span: Span,
    interner: &'a TyInterner,
    layouts: &'a BackendLayouts,
}

pub(super) struct ModuleCodegen<'ctx, 'a> {
    pub(super) context: &'ctx Context,
    pub(super) source: &'a BackendModule,
    pub(super) program: &'a ProgramIndex<'a>,
    pub(super) module: nia_llvm::module::Module<'ctx>,
    pub(super) structs: HashMap<GlobalDefId, StructType<'ctx>>,
    pub(super) unions: HashMap<GlobalDefId, StructType<'ctx>>,
    pub(super) struct_instances: HashMap<GlobalDefId, HashMap<Vec<InternedTyId>, StructType<'ctx>>>,
    pub(super) struct_instances_by_def:
        HashMap<GlobalDefId, Vec<(Vec<InternedTyId>, StructType<'ctx>)>>,
    pub(super) union_instances: HashMap<GlobalDefId, HashMap<Vec<InternedTyId>, StructType<'ctx>>>,
    pub(super) union_instances_by_def:
        HashMap<GlobalDefId, Vec<(Vec<InternedTyId>, StructType<'ctx>)>>,
    struct_instance_type_lookups: InstanceTypeLookup<'ctx>,
    union_instance_type_lookups: InstanceTypeLookup<'ctx>,
    pub(super) functions: HashMap<GlobalDefId, FunctionValue<'ctx>>,
    pub(super) function_instances:
        HashMap<(GlobalDefId, ModuleId), HashMap<Vec<InternedTyId>, FunctionValue<'ctx>>>,
    pub(super) function_instances_by_def:
        HashMap<GlobalDefId, Vec<(Vec<InternedTyId>, FunctionValue<'ctx>)>>,
    function_instance_value_lookups: FunctionInstanceLookup<'ctx>,
    pub(super) globals: HashMap<GlobalDefId, GlobalValue<'ctx>>,
    layouts: RefCell<HashMap<InternedTyId, Option<TypeLayout>>>,
    same_type_cache: RefCell<HashMap<(InternedTyId, InternedTyId), bool>>,
    mangled_types: RefCell<HashMap<InternedTyId, String>>,
    struct_layout_lookups: AggregateLayoutLookup,
    union_layout_lookups: AggregateLayoutLookup,
    pub(super) trait_object_vtables: HashMap<(InternedTyId, InternedTyId), GlobalValue<'ctx>>,
    trait_object_vtable_lookups:
        RefCell<HashMap<(InternedTyId, InternedTyId), Option<GlobalValue<'ctx>>>>,
}

impl<'ctx, 'a> ModuleCodegen<'ctx, 'a> {
    pub(super) fn new(
        context: &'ctx Context,
        source: &'a BackendModule,
        program: &'a ProgramIndex<'a>,
        _options: crate::output::LlvmCodegenOptions,
    ) -> Result<Self, Diagnostic> {
        Ok(Self {
            context,
            source,
            program,
            module: context
                .create_module(&source.name)
                .map_err(Self::diagnostic_from_llvm_error)?,
            structs: HashMap::new(),
            unions: HashMap::new(),
            struct_instances: HashMap::new(),
            struct_instances_by_def: HashMap::new(),
            union_instances: HashMap::new(),
            union_instances_by_def: HashMap::new(),
            struct_instance_type_lookups: RefCell::new(HashMap::new()),
            union_instance_type_lookups: RefCell::new(HashMap::new()),
            functions: HashMap::new(),
            function_instances: HashMap::new(),
            function_instances_by_def: HashMap::new(),
            function_instance_value_lookups: RefCell::new(HashMap::new()),
            globals: HashMap::new(),
            layouts: RefCell::new(HashMap::new()),
            same_type_cache: RefCell::new(HashMap::new()),
            mangled_types: RefCell::new(HashMap::new()),
            struct_layout_lookups: RefCell::new(HashMap::new()),
            union_layout_lookups: RefCell::new(HashMap::new()),
            trait_object_vtables: HashMap::new(),
            trait_object_vtable_lookups: RefCell::new(HashMap::new()),
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
        self.declare_structs()?;
        self.define_struct_bodies()?;
        self.declare_functions()?;
        self.declare_globals()?;
        self.declare_trait_object_vtables()?;
        self.emit_function_bodies()?;
        self.module
            .verify()
            .map_err(Self::diagnostic_from_llvm_error)
    }

    pub(super) fn layout_of(&self, ty: InternedTyId) -> Option<TypeLayout> {
        if let Some(layout) = self.layouts.borrow().get(&ty) {
            return layout.clone();
        }
        let layout = self.compute_layout_of(ty);
        self.layouts.borrow_mut().insert(ty, layout.clone());
        layout
    }

    fn compute_layout_of(&self, ty: InternedTyId) -> Option<TypeLayout> {
        let owner = self.program.module(ty.interner_id)?;
        if let Some(layout) = self.program.type_layout(ty) {
            return Some(layout.clone());
        }
        let Some(TyKind::Nominal { def_id, args }) = owner.interner.get(ty) else {
            return None;
        };
        if args.is_empty() {
            self.program
                .struct_layout(*def_id)
                .or_else(|| self.program.union_layout(*def_id))
                .map(|layout| layout.layout.clone())
                .or_else(|| {
                    let enum_item = self.program.enums.get(def_id).copied()?;
                    self.layout_of(enum_item.backing_type)
                })
        } else {
            self.program
                .struct_instance_layout(*def_id, args)
                .map(|layout| layout.layout.clone())
                .or_else(|| {
                    self.program
                        .struct_instance_layouts(*def_id)
                        .find_map(|item| {
                            self.same_type_args(&item.key.args, args)
                                .then_some(item.layout.layout.clone())
                        })
                })
                .or_else(|| {
                    self.program
                        .union_instance_layout(*def_id, args)
                        .map(|layout| layout.layout.clone())
                })
                .or_else(|| {
                    self.program
                        .union_instance_layouts(*def_id)
                        .find_map(|item| {
                            self.same_type_args(&item.key.args, args)
                                .then_some(item.layout.layout.clone())
                        })
                })
        }
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
            let Some(function_body) = &function.function_body else {
                if !function.is_extern {
                    return Err(self.error(function.span, "missing function body"));
                }
                continue;
            };
            let Some(llvm_function) = self.functions.get(&function.def_id).copied() else {
                return Err(self.error(
                    function.span,
                    format!("missing function `{}`", function.name),
                ));
            };
            let mut codegen = FunctionCodegen::new(self, function, llvm_function);
            codegen.emit_function_body(function_body)?;
        }
        for instance in &self.source.function_instances {
            let Some(function_body) = &instance.function_body else {
                if !instance.is_extern {
                    return Err(self.error(instance.span, "missing function instance body"));
                }
                continue;
            };
            let Some(llvm_function) = self.function_instance_value(instance.def_id, &instance.args)
            else {
                return Err(self.error(instance.span, "missing function instance"));
            };
            let mut codegen = FunctionCodegen::new(
                self,
                FunctionCodegenInput {
                    params: &instance.params,
                    return_type: instance.return_type,
                    span: instance.span,
                },
                llvm_function,
            );
            codegen.emit_function_body(function_body)?;
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
        } else {
            self.symbol_name(function.def_id, &function.name)
        }
    }

    fn global_symbol_name(&self, global: &nia_backend_ir::BackendGlobal) -> String {
        if global.is_extern {
            global.name.clone()
        } else {
            self.symbol_name(global.def_id, &global.name)
        }
    }

    pub(super) fn trait_object_vtable_symbol(
        &self,
        self_ty: InternedTyId,
        object_ty: InternedTyId,
    ) -> String {
        let self_part = self.mangle_ty(self_ty);
        let object_part = self.mangle_ty(object_ty);
        format!("nia__vtable__{self_part}__as__{object_part}")
    }

    fn mangle_ty(&self, ty: InternedTyId) -> String {
        if let Some(mangled) = self.mangled_types.borrow().get(&ty) {
            return mangled.clone();
        }
        let Some(owner) = self.program.module(ty.interner_id) else {
            return "ty_error".to_string();
        };
        let mangled = mangle_type_with(
            &owner.interner,
            ty,
            |def_id| {
                self.program
                    .structs
                    .get(&def_id)
                    .map(|item| item.name.clone())
                    .or_else(|| {
                        self.program
                            .unions
                            .get(&def_id)
                            .map(|item| item.name.clone())
                    })
                    .or_else(|| {
                        self.program
                            .enums
                            .get(&def_id)
                            .map(|item| item.name.clone())
                    })
                    .or_else(|| {
                        self.program
                            .functions
                            .get(&def_id)
                            .map(|item| item.name.clone())
                    })
                    .unwrap_or_else(|| format!("def{}", def_id.def_id.0))
            },
            |_| Some(0),
        );
        self.mangled_types.borrow_mut().insert(ty, mangled.clone());
        mangled
    }

    pub(super) fn error(&self, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(span, message)
    }

    pub(super) fn diagnostic_from_llvm_error(error: LlvmError) -> Diagnostic {
        error.diagnostic()
    }
}
