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
    module::Linkage,
    target::TargetMachine,
    types::{BasicTypeEnum, FunctionType, StructType},
    values::{BasicValueEnum, FunctionValue, GlobalValue, PointerValue},
};
use nia_mangle::{mangle_base_symbol, mangle_type_with};
use nia_span::Span;
use nia_ty::{ConstGenericArg, PrimitiveTy, TyInterner, TyKind};

type InstanceKey = (GlobalDefId, Vec<InternedTyId>, Vec<ConstGenericArg>);
type FunctionInstanceKey = (
    GlobalDefId,
    ModuleId,
    Vec<InternedTyId>,
    Vec<ConstGenericArg>,
);
type GlobalInstanceKey = (
    GlobalDefId,
    ModuleId,
    Vec<InternedTyId>,
    Vec<ConstGenericArg>,
);
type InstanceTypeLookup<'ctx> = RefCell<HashMap<InstanceKey, Option<StructType<'ctx>>>>;
type FunctionInstanceLookup<'ctx> =
    RefCell<HashMap<FunctionInstanceKey, Option<FunctionValue<'ctx>>>>;
type AggregateLayoutLookup = RefCell<HashMap<InstanceKey, Option<Vec<InternedTyId>>>>;
type TraitObjectAdapterKey = (
    InternedTyId,
    GlobalDefId,
    ModuleId,
    Vec<InternedTyId>,
    Vec<ConstGenericArg>,
);

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
    pub(super) struct_instances:
        HashMap<GlobalDefId, HashMap<(Vec<InternedTyId>, Vec<ConstGenericArg>), StructType<'ctx>>>,
    pub(super) struct_instances_by_def:
        HashMap<GlobalDefId, Vec<(Vec<InternedTyId>, Vec<ConstGenericArg>, StructType<'ctx>)>>,
    pub(super) union_instances:
        HashMap<GlobalDefId, HashMap<(Vec<InternedTyId>, Vec<ConstGenericArg>), StructType<'ctx>>>,
    pub(super) union_instances_by_def:
        HashMap<GlobalDefId, Vec<(Vec<InternedTyId>, Vec<ConstGenericArg>, StructType<'ctx>)>>,
    struct_instance_type_lookups: InstanceTypeLookup<'ctx>,
    union_instance_type_lookups: InstanceTypeLookup<'ctx>,
    pub(super) functions: HashMap<GlobalDefId, FunctionValue<'ctx>>,
    pub(super) function_instances: HashMap<
        (GlobalDefId, ModuleId),
        HashMap<(Vec<InternedTyId>, Vec<ConstGenericArg>), FunctionValue<'ctx>>,
    >,
    pub(super) function_instances_by_def: HashMap<
        GlobalDefId,
        Vec<(
            ModuleId,
            Vec<InternedTyId>,
            Vec<ConstGenericArg>,
            FunctionValue<'ctx>,
        )>,
    >,
    function_instance_value_lookups: FunctionInstanceLookup<'ctx>,
    pub(super) globals: HashMap<GlobalDefId, GlobalValue<'ctx>>,
    pub(super) global_instances: HashMap<GlobalInstanceKey, GlobalValue<'ctx>>,
    static_array_counter: RefCell<usize>,
    layouts: RefCell<HashMap<InternedTyId, Option<TypeLayout>>>,
    same_type_cache: RefCell<HashMap<(InternedTyId, InternedTyId), bool>>,
    mangled_types: RefCell<HashMap<InternedTyId, String>>,
    struct_layout_lookups: AggregateLayoutLookup,
    union_layout_lookups: AggregateLayoutLookup,
    pub(super) trait_object_vtables: HashMap<(InternedTyId, InternedTyId), GlobalValue<'ctx>>,
    trait_object_vtable_lookups:
        RefCell<HashMap<(InternedTyId, InternedTyId), Option<GlobalValue<'ctx>>>>,
    trait_object_adapters: RefCell<HashMap<TraitObjectAdapterKey, FunctionValue<'ctx>>>,
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
            global_instances: HashMap::new(),
            static_array_counter: RefCell::new(0),
            layouts: RefCell::new(HashMap::new()),
            same_type_cache: RefCell::new(HashMap::new()),
            mangled_types: RefCell::new(HashMap::new()),
            struct_layout_lookups: RefCell::new(HashMap::new()),
            union_layout_lookups: RefCell::new(HashMap::new()),
            trait_object_vtables: HashMap::new(),
            trait_object_vtable_lookups: RefCell::new(HashMap::new()),
            trait_object_adapters: RefCell::new(HashMap::new()),
        })
    }

    pub(super) fn emit_ir(&mut self) -> Result<String, Diagnostic> {
        self.emit_module()?;
        self.module
            .ir_string()
            .map_err(Self::diagnostic_from_llvm_error)
    }

    pub(super) fn next_static_array_name(&self) -> String {
        let mut counter = self.static_array_counter.borrow_mut();
        let id = *counter;
        *counter += 1;
        format!(".nia.static.array.{id}")
    }

    pub(super) fn materialize_static_array_pointer(
        &self,
        array_ty: BasicTypeEnum<'ctx>,
        value: BasicValueEnum<'ctx>,
        span: Span,
    ) -> Result<PointerValue<'ctx>, Diagnostic> {
        if matches!(value, BasicValueEnum::PointerValue(_)) {
            return Err(self.error(
                span,
                "static array pointer source emitted a pointer instead of an array",
            ));
        }
        let value_ty = value.get_type().map_err(|err| {
            self.error(
                span,
                format!("failed to inspect static array value: {err:?}"),
            )
        })?;
        if value_ty != array_ty {
            return Err(self.error(
                span,
                format!(
                    "static array pointer source type does not match array type: expected {array_ty:?}, got {value_ty:?}"
                ),
            ));
        }
        let global = self
            .module
            .add_global(array_ty, None, &self.next_static_array_name())
            .map_err(Self::diagnostic_from_llvm_error)?;
        global.set_linkage(Linkage::Internal);
        global.set_constant(true);
        global.set_initializer(&value);
        Ok(global.as_pointer_value())
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
        let owner = self.program.module(self.type_owner(ty).module_id())?;
        if let Some(layout) = self.program.type_layout(ty) {
            return Some(layout.clone());
        }
        match owner.interner.get(ty) {
            Some(TyKind::Primitive(primitive)) => self.primitive_layout(*primitive),
            Some(TyKind::Vector { elem, lanes }) => self.vector_layout(*elem, *lanes),
            Some(
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. },
            ) => Some(TypeLayout {
                size: owner.layouts.target.pointer_size,
                align: owner.layouts.target.pointer_align,
            }),
            Some(TyKind::Slice { .. } | TyKind::TraitObject { .. }) => Some(TypeLayout {
                size: owner.layouts.target.pointer_size * 2,
                align: owner.layouts.target.pointer_align,
            }),
            Some(TyKind::SlicePointee { .. } | TyKind::TraitObjectPointee { .. }) => None,
            Some(TyKind::Range { bound: None, .. }) => Some(TypeLayout { size: 0, align: 1 }),
            Some(TyKind::Range {
                bound: Some(bound), ..
            }) => {
                let bound = self.layout_of(*bound)?;
                Some(TypeLayout {
                    size: bound.size * 2,
                    align: bound.align,
                })
            }
            Some(TyKind::Array { len, elem }) => {
                let len = self.array_len_in(len, Span::default()).ok()?;
                let elem = self.layout_of(*elem)?;
                Some(TypeLayout {
                    size: elem.size.saturating_mul(len),
                    align: elem.align,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.layout_of(*elem)?;
                Some(tagged_union_layout(&[elem]))
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.layout_of(*error)?;
                let value = self.layout_of(*value)?;
                Some(tagged_union_layout(&[error, value]))
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) if args.is_empty() && const_args.is_empty() => self
                .program
                .struct_layout(*def_id)
                .or_else(|| self.program.union_layout(*def_id))
                .map(|layout| layout.layout.clone())
                .or_else(|| {
                    let enum_item = self.program.enums.get(def_id).copied()?;
                    self.layout_of(enum_item.backing_type)
                }),
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => self
                .program
                .struct_instance_layout(*def_id, args, const_args)
                .map(|layout| layout.layout.clone())
                .or_else(|| {
                    self.program
                        .struct_instance_layouts(*def_id)
                        .find_map(|item| {
                            (self.same_type_args(&item.key.args, args)
                                && item.key.const_args.as_slice() == const_args.as_slice())
                            .then_some(item.layout.layout.clone())
                        })
                })
                .or_else(|| {
                    self.program
                        .union_instance_layout(*def_id, args, const_args)
                        .map(|layout| layout.layout.clone())
                })
                .or_else(|| {
                    self.program
                        .union_instance_layouts(*def_id)
                        .find_map(|item| {
                            (self.same_type_args(&item.key.args, args)
                                && item.key.const_args.as_slice() == const_args.as_slice())
                            .then_some(item.layout.layout.clone())
                        })
                }),
            Some(
                TyKind::GenericParam(_)
                | TyKind::BuiltinTrait { .. }
                | TyKind::Projection { .. }
                | TyKind::ComptimeOnly
                | TyKind::Error,
            )
            | None => None,
        }
    }

    fn primitive_layout(&self, primitive: PrimitiveTy) -> Option<TypeLayout> {
        Some(match primitive {
            PrimitiveTy::Void | PrimitiveTy::Never => TypeLayout { size: 0, align: 1 },
            PrimitiveTy::Bool | PrimitiveTy::I8 | PrimitiveTy::U8 => {
                TypeLayout { size: 1, align: 1 }
            }
            PrimitiveTy::I16 | PrimitiveTy::U16 => TypeLayout { size: 2, align: 2 },
            PrimitiveTy::I32 | PrimitiveTy::U32 | PrimitiveTy::F32 | PrimitiveTy::Char => {
                TypeLayout { size: 4, align: 4 }
            }
            PrimitiveTy::I64 | PrimitiveTy::U64 | PrimitiveTy::F64 => {
                TypeLayout { size: 8, align: 8 }
            }
            PrimitiveTy::I128 | PrimitiveTy::U128 => TypeLayout {
                size: 16,
                align: 16,
            },
            PrimitiveTy::Isize | PrimitiveTy::Usize => TypeLayout {
                size: self.source.layouts.target.pointer_size,
                align: self.source.layouts.target.pointer_align,
            },
        })
    }

    fn vector_layout(&self, elem: PrimitiveTy, lanes: u32) -> Option<TypeLayout> {
        if !elem.is_vector_element() || lanes == 0 {
            return None;
        }
        let elem_layout = self.primitive_layout(elem)?;
        Some(TypeLayout {
            size: elem_layout.size.checked_mul(lanes as u64)?,
            align: elem_layout.align,
        })
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
            if !function.generics.is_empty() {
                continue;
            }
            let Some(function_body) = &function.function_body else {
                if !function.is_extern {
                    return Err(self.error(
                        function.span,
                        format!(
                            "missing function body for `{}` {:?}",
                            function.name, function.def_id
                        ),
                    ));
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
                    return Err(self.error(
                        instance.span,
                        format!(
                            "missing function instance body for `{}` {:?} with args {:?}",
                            instance.name, instance.def_id, instance.args
                        ),
                    ));
                }
                continue;
            };
            let Some(llvm_function) = self.function_instance_value(
                instance.def_id,
                instance.arg_module_id,
                &instance.args,
                &instance.const_args,
            ) else {
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

    pub(super) fn add_internal_helper_function(
        &self,
        name: &str,
        ty: FunctionType<'ctx>,
    ) -> Result<FunctionValue<'ctx>, Diagnostic> {
        self.module
            .add_function(name, ty, Some(Linkage::Internal))
            .map_err(Self::diagnostic_from_llvm_error)
    }

    fn mangle_ty(&self, ty: InternedTyId) -> String {
        if let Some(mangled) = self.mangled_types.borrow().get(&ty) {
            return mangled.clone();
        }
        let Some(owner) = self.program.module(self.type_owner(ty).module_id()) else {
            panic!("Nia ICE: cannot mangle type {ty:?} without owner module");
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
        Diagnostic::user_error_at(nia_diagnostic::codes::LLVM_CODEGEN, span, message)
    }

    pub(super) fn diagnostic_from_llvm_error(error: LlvmError) -> Diagnostic {
        error.diagnostic()
    }
}

fn tagged_union_layout(payloads: &[TypeLayout]) -> TypeLayout {
    let tag = TypeLayout { size: 1, align: 1 };
    let payload_size = payloads.iter().map(|layout| layout.size).max().unwrap_or(0);
    let payload_align = payloads
        .iter()
        .map(|layout| layout.align)
        .max()
        .unwrap_or(1);
    let align = tag.align.max(payload_align);
    let payload_offset = align_to(tag.size, payload_align);
    TypeLayout {
        size: align_to(payload_offset.saturating_add(payload_size), align),
        align,
    }
}

fn align_to(value: u64, align: u64) -> u64 {
    if align <= 1 {
        value
    } else {
        value.div_ceil(align) * align
    }
}
