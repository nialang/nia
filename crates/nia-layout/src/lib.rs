// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use nia_defs::{DefCollection, DefId};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_item_signatures::{
    EnumSignature, EnumVariantPayloadSignature, GenericParamSignature, GenericParamSignatureKind,
    ItemSignatures, ProgramEnumSignature, ProgramStructSignature, ProgramTypeAliasSignature,
    ProgramUnionSignature, StructSignature, TypeAliasSignature, UnionSignature,
};
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap, SymbolText, symbol_text_from_optional_resolver};
use nia_ty::{
    ArrayLenTy, ConstGenericArg, ConstGenericValue, LayoutBuiltin, PrimitiveTy, RangeTyKind, TyKind,
};

mod abi;
mod aggregate;

use abi::{align_to, tagged_union_layout, tagged_union_layout_with_tag};
pub use abi::{array_layout, primitive_layout, union_layout_from_fields, vector_layout};
use aggregate::{
    PendingEnumFieldLayout, PendingFieldLayout, place_enum_fields, place_struct_fields,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetDataLayout {
    pub pointer_size: u64,
    pub pointer_align: u64,
}

impl TargetDataLayout {
    pub const LP64: Self = Self {
        pointer_size: 8,
        pointer_align: 8,
    };

    pub fn from_pointer_width(pointer_width: u32) -> Option<Self> {
        if !pointer_width.is_multiple_of(8) {
            return None;
        }
        let pointer_size = u64::from(pointer_width.checked_div(8)?);
        matches!(pointer_size, 1 | 2 | 4 | 8 | 16).then_some(Self {
            pointer_size,
            pointer_align: pointer_size,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeLayout {
    pub size: u64,
    pub align: u64,
}

impl TypeLayout {
    pub fn builtin_value(&self, builtin: LayoutBuiltin) -> u64 {
        match builtin {
            LayoutBuiltin::Size => self.size,
            LayoutBuiltin::Align => self.align,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructLayout {
    pub layout: TypeLayout,
    pub fields: Vec<FieldLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructLayoutKey {
    pub def_id: DefId,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GlobalStructLayoutKey {
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
    const_args: Vec<ConstGenericArg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldLayout {
    pub def_id: DefId,
    pub offset: u64,
    pub layout: TypeLayout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumLayout {
    pub layout: TypeLayout,
    pub tag: TypeLayout,
    pub payload_offset: Option<u64>,
    pub variants: Vec<EnumVariantLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumVariantLayout {
    pub def_id: DefId,
    pub payload: TypeLayout,
    pub fields: Vec<EnumFieldLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumFieldLayout {
    pub def_id: Option<DefId>,
    pub offset: u64,
    pub layout: TypeLayout,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layouts {
    pub target: TargetDataLayout,
    pub types: HashMap<InternedTyId, TypeLayout>,
    pub structs: HashMap<DefId, StructLayout>,
    pub unions: HashMap<DefId, StructLayout>,
    pub enums: HashMap<DefId, EnumLayout>,
    pub struct_instances: HashMap<StructLayoutKey, StructLayout>,
    pub union_instances: HashMap<StructLayoutKey, StructLayout>,
    pub diagnostics: Vec<Diagnostic>,
}

pub trait ArrayLengthValues {
    fn array_len(&self, id: GlobalConstExprId) -> Option<u64>;
}

#[derive(Clone, Copy, Default)]
pub struct NoArrayLengthValues;

impl ArrayLengthValues for NoArrayLengthValues {
    fn array_len(&self, _id: GlobalConstExprId) -> Option<u64> {
        None
    }
}

impl<F> ArrayLengthValues for F
where
    F: Fn(GlobalConstExprId) -> Option<u64>,
{
    fn array_len(&self, id: GlobalConstExprId) -> Option<u64> {
        self(id)
    }
}

impl Layouts {
    pub fn nominal_type_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<TypeLayout> {
        self.nominal_type_layout_with_const_args(def_id, args, &[])
    }

    pub fn nominal_type_layout_with_const_args(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<TypeLayout> {
        if args.is_empty() && const_args.is_empty() {
            self.structs
                .get(&def_id.def_id)
                .map(|layout| layout.layout.clone())
                .or_else(|| {
                    self.unions
                        .get(&def_id.def_id)
                        .map(|layout| layout.layout.clone())
                })
                .or_else(|| {
                    self.enums
                        .get(&def_id.def_id)
                        .map(|layout| layout.layout.clone())
                })
        } else {
            let key = StructLayoutKey {
                def_id: def_id.def_id,
                args: args.to_vec(),
                const_args: const_args.to_vec(),
            };
            self.struct_instances
                .get(&key)
                .map(|layout| layout.layout.clone())
                .or_else(|| {
                    self.union_instances
                        .get(&key)
                        .map(|layout| layout.layout.clone())
                })
        }
    }

    pub fn field_offset(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        field: GlobalDefId,
    ) -> Option<u64> {
        self.field_offset_with_const_args(def_id, args, &[], field)
    }

    pub fn field_offset_with_const_args(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
        field: GlobalDefId,
    ) -> Option<u64> {
        self.nominal_struct_layout_with_const_args(def_id, args, const_args)
            .and_then(|layout| {
                layout
                    .fields
                    .iter()
                    .find(|candidate| candidate.def_id == field.def_id)
                    .map(|field| field.offset)
            })
    }

    pub fn nominal_struct_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<&StructLayout> {
        self.nominal_struct_layout_with_const_args(def_id, args, &[])
    }

    pub fn nominal_struct_layout_with_const_args(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&StructLayout> {
        if args.is_empty() && const_args.is_empty() {
            self.structs
                .get(&def_id.def_id)
                .or_else(|| self.unions.get(&def_id.def_id))
        } else {
            let key = StructLayoutKey {
                def_id: def_id.def_id,
                args: args.to_vec(),
                const_args: const_args.to_vec(),
            };
            self.struct_instances
                .get(&key)
                .or_else(|| self.union_instances.get(&key))
        }
    }

    pub fn nominal_enum_layout(&self, def_id: GlobalDefId) -> Option<&EnumLayout> {
        self.enums.get(&def_id.def_id)
    }
}

pub fn compute_layouts(
    type_store: &nia_ty::TypeStore,
    defs: &DefCollection,
    signatures: &ItemSignatures,
    target: TargetDataLayout,
) -> Layouts {
    let normalized = HashMap::new();
    let empty_lengths = NoArrayLengthValues;
    let root_types = signatures.type_roots();
    compute_layouts_with_program_context(LayoutComputationInput {
        type_store,
        defs,
        signatures,
        root_types: &root_types,
        normalized: &normalized,
        array_lengths: &empty_lengths,
        target,
        program: ProgramLayoutContext::default(),
    })
}

#[derive(Clone, Copy, Default)]
pub struct ProgramLayoutContext<'a> {
    pub symbols: Option<&'a dyn SymbolText>,
    pub layouts: Option<&'a dyn Fn(ModuleId) -> Option<Arc<Layouts>>>,
    pub array_lengths: Option<&'a dyn Fn(GlobalConstExprId) -> Option<u64>>,
    pub structs: Option<&'a HashMap<GlobalDefId, ProgramStructSignature>>,
    pub unions: Option<&'a HashMap<GlobalDefId, ProgramUnionSignature>>,
    pub enums: Option<&'a HashMap<GlobalDefId, ProgramEnumSignature>>,
    pub type_aliases: Option<&'a HashMap<GlobalDefId, ProgramTypeAliasSignature>>,
    pub struct_: Option<&'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>>,
    pub union: Option<&'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>>,
    pub enum_: Option<&'a dyn Fn(GlobalDefId) -> Option<ProgramEnumSignature>>,
    pub type_alias: Option<&'a dyn Fn(GlobalDefId) -> Option<ProgramTypeAliasSignature>>,
}

#[derive(Clone, Copy)]
pub struct InstanceLayoutRequest<'a> {
    pub def_id: GlobalDefId,
    pub args: &'a [InternedTyId],
    pub const_args: &'a [ConstGenericArg],
}

pub struct LayoutComputationInput<'a> {
    pub type_store: &'a nia_ty::TypeStore,
    pub defs: &'a DefCollection,
    pub signatures: &'a ItemSignatures,
    pub root_types: &'a [InternedTyId],
    pub normalized: &'a HashMap<InternedTyId, InternedTyId>,
    pub array_lengths: &'a dyn ArrayLengthValues,
    pub target: TargetDataLayout,
    pub program: ProgramLayoutContext<'a>,
}

impl LayoutComputationInput<'_> {
    fn reborrow(&self) -> LayoutComputationInput<'_> {
        LayoutComputationInput {
            type_store: self.type_store,
            defs: self.defs,
            signatures: self.signatures,
            root_types: self.root_types,
            normalized: self.normalized,
            array_lengths: self.array_lengths,
            target: self.target,
            program: self.program,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct LayoutRoots<'a> {
    pub types: &'a [InternedTyId],
    pub structs: &'a [DefId],
    pub unions: &'a [DefId],
}

pub fn compute_layouts_with_program_context(input: LayoutComputationInput<'_>) -> Layouts {
    LayoutComputer::new(input).compute()
}

pub fn compute_layouts_for_roots_with_program_context(
    input: LayoutComputationInput<'_>,
    roots: LayoutRoots<'_>,
) -> Layouts {
    LayoutComputer::new(input).compute_roots(roots)
}

pub fn compute_struct_instance_layout_with_program_context(
    input: &LayoutComputationInput<'_>,
    request: InstanceLayoutRequest<'_>,
) -> Option<StructLayout> {
    let local_module_id = input.defs.module_id;
    let mut computer = LayoutComputer::new(input.reborrow());
    computer.detailed_struct_layout(
        Span::default(),
        request.def_id,
        request.args,
        request.const_args,
    )?;
    if request.def_id.module_id != local_module_id {
        return computer
            .external_struct_instances
            .get(&GlobalStructLayoutKey {
                def_id: request.def_id,
                args: request.args.to_vec(),
                const_args: request.const_args.to_vec(),
            })
            .cloned();
    }
    computer
        .struct_instances
        .get(&StructLayoutKey {
            def_id: request.def_id.def_id,
            args: request.args.to_vec(),
            const_args: request.const_args.to_vec(),
        })
        .cloned()
}

pub fn compute_union_instance_layout_with_program_context(
    input: &LayoutComputationInput<'_>,
    request: InstanceLayoutRequest<'_>,
) -> Option<StructLayout> {
    let local_module_id = input.defs.module_id;
    let mut computer = LayoutComputer::new(input.reborrow());
    computer.detailed_union_layout(
        Span::default(),
        request.def_id,
        request.args,
        request.const_args,
    )?;
    if request.def_id.module_id != local_module_id {
        return computer
            .external_union_instances
            .get(&GlobalStructLayoutKey {
                def_id: request.def_id,
                args: request.args.to_vec(),
                const_args: request.const_args.to_vec(),
            })
            .cloned();
    }
    computer
        .union_instances
        .get(&StructLayoutKey {
            def_id: request.def_id.def_id,
            args: request.args.to_vec(),
            const_args: request.const_args.to_vec(),
        })
        .cloned()
}

struct LayoutTypeCx<'a> {
    store: &'a nia_ty::TypeStore,
    append: nia_ty::TypeStoreAppend,
}

impl LayoutTypeCx<'_> {
    fn get(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.store.get(ty)
    }
}

struct LayoutComputer<'a> {
    module_id: nia_ids::ModuleId,
    type_context: LayoutTypeCx<'a>,
    signatures: &'a ItemSignatures,
    root_types: &'a [InternedTyId],
    normalized: &'a HashMap<InternedTyId, InternedTyId>,
    array_lengths: &'a dyn ArrayLengthValues,
    target: TargetDataLayout,
    types: HashMap<InternedTyId, TypeLayout>,
    structs: HashMap<DefId, StructLayout>,
    unions: HashMap<DefId, StructLayout>,
    enums: HashMap<DefId, EnumLayout>,
    struct_instances: HashMap<StructLayoutKey, StructLayout>,
    union_instances: HashMap<StructLayoutKey, StructLayout>,
    external_struct_instances: HashMap<GlobalStructLayoutKey, StructLayout>,
    external_union_instances: HashMap<GlobalStructLayoutKey, StructLayout>,
    diagnostics: Vec<Diagnostic>,
    visiting: HashSet<InternedTyId>,
    visiting_structs: HashSet<StructLayoutKey>,
    visiting_unions: HashSet<StructLayoutKey>,
    program: ProgramLayoutContext<'a>,
}

impl<'a> LayoutComputer<'a> {
    fn new(input: LayoutComputationInput<'a>) -> Self {
        Self {
            module_id: input.defs.module_id,
            type_context: LayoutTypeCx {
                store: input.type_store,
                append: input.type_store.append_for_module(input.defs.module_id),
            },
            signatures: input.signatures,
            root_types: input.root_types,
            normalized: input.normalized,
            array_lengths: input.array_lengths,
            target: input.target,
            types: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            enums: HashMap::new(),
            struct_instances: HashMap::new(),
            union_instances: HashMap::new(),
            external_struct_instances: HashMap::new(),
            external_union_instances: HashMap::new(),
            diagnostics: Vec::new(),
            visiting: HashSet::new(),
            visiting_structs: HashSet::new(),
            visiting_unions: HashSet::new(),
            program: input.program,
        }
    }

    fn compute(mut self) -> Layouts {
        for ty_id in self.root_types.iter().copied() {
            if self.is_inferred_array_type(ty_id) || self.is_open_generic_type(ty_id) {
                continue;
            }
            self.layout_ty(ty_id, Span::default());
        }
        let struct_signatures: Vec<(DefId, StructSignature)> = self
            .signatures
            .structs
            .iter()
            .filter(|(_, signature)| signature.generics.is_empty())
            .map(|(def_id, signature)| (*def_id, signature.clone()))
            .collect();
        for (def_id, signature) in struct_signatures {
            self.struct_layout(signature.span, def_id, &signature, &[], &[]);
        }
        let union_signatures: Vec<(DefId, UnionSignature)> = self
            .signatures
            .unions
            .iter()
            .filter(|(_, signature)| signature.generics.is_empty())
            .map(|(def_id, signature)| (*def_id, signature.clone()))
            .collect();
        for (def_id, signature) in union_signatures {
            self.union_layout(signature.span, def_id, &signature, &[], &[]);
        }
        let enum_signatures: Vec<(DefId, EnumSignature)> = self
            .signatures
            .enums
            .iter()
            .map(|(def_id, signature)| (*def_id, signature.clone()))
            .collect();
        for (def_id, signature) in enum_signatures {
            self.enum_layout(signature.span, Some(def_id), &signature);
        }
        self.finish()
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.program.symbols, symbol)
    }

    fn compute_roots(mut self, roots: LayoutRoots<'_>) -> Layouts {
        for ty_id in roots.types {
            if self.is_inferred_array_type(*ty_id) || self.is_open_generic_type(*ty_id) {
                continue;
            }
            self.layout_ty(*ty_id, Span::default());
        }
        for def_id in roots.structs {
            if let Some(signature) = self.signatures.structs.get(def_id).cloned()
                && signature.generics.is_empty()
            {
                self.struct_layout(signature.span, *def_id, &signature, &[], &[]);
            }
        }
        for def_id in roots.unions {
            if let Some(signature) = self.signatures.unions.get(def_id).cloned()
                && signature.generics.is_empty()
            {
                self.union_layout(signature.span, *def_id, &signature, &[], &[]);
            }
        }
        self.finish()
    }

    fn finish(self) -> Layouts {
        Layouts {
            target: self.target,
            types: self.types,
            structs: self.structs,
            unions: self.unions,
            enums: self.enums,
            struct_instances: self.struct_instances,
            union_instances: self.union_instances,
            diagnostics: self.diagnostics,
        }
    }

    fn layout_ty(&mut self, ty_id: InternedTyId, span: Span) -> Option<TypeLayout> {
        let original_ty_id = ty_id;
        if let Some(layout) = self.types.get(&original_ty_id).cloned() {
            return Some(layout);
        }
        let ty_id = self.normalize_ty(original_ty_id);
        if let Some(layout) = self.types.get(&ty_id).cloned() {
            self.types.insert(original_ty_id, layout.clone());
            return Some(layout);
        }
        if !self.visiting.insert(ty_id) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                "recursive type layout is not supported",
            ));
            return None;
        }
        let layout = match self.type_context.get(ty_id).cloned() {
            Some(TyKind::Primitive(primitive)) => self.primitive_layout(primitive),
            Some(TyKind::Tuple(elems)) => self.tuple_layout(span, &elems),
            Some(TyKind::ClosureState { captures, .. }) => self.tuple_layout(span, &captures),
            Some(TyKind::Vector { elem, lanes }) => self.vector_layout(span, elem, lanes),
            Some(
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. },
            ) => Some(TypeLayout {
                size: self.target.pointer_size,
                align: self.target.pointer_align,
            }),
            Some(TyKind::Slice { .. } | TyKind::TraitObject { .. } | TyKind::Callable { .. }) => {
                Some(TypeLayout {
                    size: self.target.pointer_size * 2,
                    align: self.target.pointer_align,
                })
            }
            Some(
                TyKind::Opaque
                | TyKind::SlicePointee { .. }
                | TyKind::TraitObjectPointee { .. }
                | TyKind::CallablePointee { .. },
            ) => None,
            Some(TyKind::Range { kind, bound }) => self.range_layout(span, kind, bound),
            Some(TyKind::Array { len, elem }) => self.array_layout(span, len, elem),
            Some(TyKind::Optional { elem }) => self.optional_layout(span, elem),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.error_union_layout(span, error, value)
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => self.nominal_layout(span, def_id, &args, &const_args),
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::SelfParam
                | TyKind::BuiltinType(_)
                | TyKind::GenericParam(_)
                | TyKind::BuiltinTrait { .. }
                | TyKind::Projection { .. },
            )
            | None => None,
        };
        self.visiting.remove(&ty_id);
        if let Some(layout) = &layout {
            self.types.insert(ty_id, layout.clone());
            self.types.insert(original_ty_id, layout.clone());
        }
        layout
    }

    fn is_inferred_array_type(&self, ty_id: InternedTyId) -> bool {
        matches!(
            self.type_context.get(ty_id),
            Some(TyKind::Array {
                len: ArrayLenTy::Infer,
                ..
            })
        )
    }

    fn is_open_generic_type(&self, ty_id: InternedTyId) -> bool {
        let mut seen = HashSet::new();
        self.is_open_generic_type_inner(ty_id, &mut seen)
    }

    fn is_open_generic_type_inner(
        &self,
        ty_id: InternedTyId,
        seen: &mut HashSet<InternedTyId>,
    ) -> bool {
        if !seen.insert(ty_id) {
            return false;
        }
        match self.type_context.get(ty_id) {
            Some(TyKind::GenericParam(_) | TyKind::SelfParam) => true,
            Some(TyKind::Array { len, elem }) => {
                self.is_open_generic_array_len(len) || self.is_open_generic_type_inner(*elem, seen)
            }
            Some(
                TyKind::Opaque
                | TyKind::Vector { .. }
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_),
            ) => false,
            Some(TyKind::Tuple(elems)) => elems
                .iter()
                .any(|elem| self.is_open_generic_type_inner(*elem, seen)),
            Some(TyKind::ClosureState {
                captures,
                params,
                return_type,
                ..
            }) => {
                captures
                    .iter()
                    .chain(params)
                    .any(|ty| self.is_open_generic_type_inner(*ty, seen))
                    || self.is_open_generic_type_inner(*return_type, seen)
            }
            Some(
                TyKind::Pointer { elem, .. }
                | TyKind::VolatilePointer { elem, .. }
                | TyKind::Slice { elem, .. }
                | TyKind::SlicePointee { elem },
            ) => self.is_open_generic_type_inner(*elem, seen),
            Some(
                TyKind::Optional { elem }
                | TyKind::Range {
                    bound: Some(elem), ..
                },
            ) => self.is_open_generic_type_inner(*elem, seen),
            Some(TyKind::Range { bound: None, .. }) => false,
            Some(TyKind::ErrorUnion { error, value }) => {
                self.is_open_generic_type_inner(*error, seen)
                    || self.is_open_generic_type_inner(*value, seen)
            }
            Some(
                TyKind::FunctionPointer {
                    params,
                    return_type,
                    ..
                }
                | TyKind::Callable {
                    params,
                    return_type,
                    ..
                }
                | TyKind::CallablePointee {
                    params,
                    return_type,
                },
            ) => {
                params
                    .iter()
                    .any(|param| self.is_open_generic_type_inner(*param, seen))
                    || self.is_open_generic_type_inner(*return_type, seen)
            }
            Some(TyKind::Nominal {
                args, const_args, ..
            }) => {
                args.iter()
                    .any(|arg| self.is_open_generic_type_inner(*arg, seen))
                    || const_args
                        .iter()
                        .any(|arg| self.is_open_generic_const_arg(arg, seen))
            }
            Some(TyKind::BuiltinTrait { args, .. }) => args
                .iter()
                .any(|arg| self.is_open_generic_type_inner(*arg, seen)),
            Some(
                TyKind::TraitObject {
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                    ..
                }
                | TyKind::TraitObjectPointee {
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                    ..
                },
            ) => {
                trait_args
                    .iter()
                    .any(|arg| self.is_open_generic_type_inner(*arg, seen))
                    || trait_const_args
                        .iter()
                        .any(|arg| self.is_open_generic_const_arg(arg, seen))
                    || associated_type_bindings.iter().any(|binding| {
                        self.is_open_generic_type_inner(binding.ty, seen)
                            || binding
                                .trait_args
                                .iter()
                                .any(|arg| self.is_open_generic_type_inner(*arg, seen))
                            || binding
                                .trait_const_args
                                .iter()
                                .any(|arg| self.is_open_generic_const_arg(arg, seen))
                    })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            }) => {
                self.is_open_generic_type_inner(*self_ty, seen)
                    || trait_args
                        .iter()
                        .any(|arg| self.is_open_generic_type_inner(*arg, seen))
                    || trait_const_args
                        .iter()
                        .any(|arg| self.is_open_generic_const_arg(arg, seen))
            }
            Some(TyKind::Error | TyKind::ConstOnly) | None => false,
        }
    }

    fn is_open_generic_array_len(&self, len: &ArrayLenTy) -> bool {
        match len {
            ArrayLenTy::GenericParam(_) => true,
            ArrayLenTy::Builtin { ty, .. } => self.is_open_generic_type(*ty),
            ArrayLenTy::Infer | ArrayLenTy::ConstValue(_) | ArrayLenTy::ConstExpr(_) => false,
        }
    }

    fn is_open_generic_const_arg(
        &self,
        arg: &ConstGenericArg,
        seen: &mut HashSet<InternedTyId>,
    ) -> bool {
        matches!(arg.value, ConstGenericValue::GenericParam(_))
            || self.is_open_generic_type_inner(arg.ty, seen)
    }

    fn primitive_layout(&self, primitive: PrimitiveTy) -> Option<TypeLayout> {
        Some(primitive_layout(primitive, self.target))
    }

    fn vector_layout(&mut self, span: Span, elem: PrimitiveTy, lanes: u32) -> Option<TypeLayout> {
        let Some(layout) = vector_layout(elem, lanes, self.target) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                "SIMD vector layout is invalid or overflowed",
            ));
            return None;
        };
        Some(layout)
    }

    fn array_layout(
        &mut self,
        span: Span,
        len: ArrayLenTy,
        elem: InternedTyId,
    ) -> Option<TypeLayout> {
        let elem_layout = self.layout_ty(elem, span)?;
        let len = match len {
            ArrayLenTy::Infer => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::STATIC_CHECK,
                    span,
                    "array layout requires a concrete length",
                ));
                return None;
            }
            ArrayLenTy::GenericParam(name) => {
                let name = self.symbol_name(name);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::STATIC_CHECK,
                    span,
                    format!("array layout requires concrete value for const generic `{name}`"),
                ));
                return None;
            }
            ArrayLenTy::ConstValue(value) => value,
            ArrayLenTy::ConstExpr(id) => {
                let value = if id.module_id == self.module_id {
                    self.array_lengths.array_len(id)
                } else {
                    self.program
                        .array_lengths
                        .and_then(|array_lengths| array_lengths(id))
                };
                let Some(value) = value else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        span,
                        "array length was not evaluated by const",
                    ));
                    return None;
                };
                value
            }
            ArrayLenTy::Builtin { builtin, ty } => {
                let Some(layout) = self.layout_ty(ty, span) else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        span,
                        format!(
                            "cannot compute layout for array length builtin `@{}`",
                            builtin.name()
                        ),
                    ));
                    return None;
                };
                layout.builtin_value(builtin)
            }
        };
        let Some(layout) = crate::array_layout(&elem_layout, len) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                "array layout size overflowed",
            ));
            return None;
        };
        Some(layout)
    }

    fn tuple_layout(&mut self, span: Span, elems: &[InternedTyId]) -> Option<TypeLayout> {
        let mut size = 0u64;
        let mut align = 1u64;
        for elem in elems {
            let elem = self.layout_ty(*elem, span)?;
            size = align_to(size, elem.align).saturating_add(elem.size);
            align = align.max(elem.align);
        }
        Some(TypeLayout {
            size: align_to(size, align),
            align,
        })
    }

    fn range_layout(
        &mut self,
        span: Span,
        kind: RangeTyKind,
        bound: Option<InternedTyId>,
    ) -> Option<TypeLayout> {
        let field_count = match kind {
            RangeTyKind::Exclusive | RangeTyKind::Inclusive => 2,
            RangeTyKind::From | RangeTyKind::To | RangeTyKind::ToInclusive => 1,
            RangeTyKind::Full => 0,
        };
        let Some(bound) = bound else {
            return (field_count == 0).then_some(TypeLayout { size: 0, align: 1 });
        };
        let bound_layout = self.layout_ty(bound, span)?;
        Some(TypeLayout {
            size: align_to(
                bound_layout.size.saturating_mul(field_count),
                bound_layout.align,
            ),
            align: bound_layout.align,
        })
    }

    fn optional_layout(&mut self, span: Span, elem: InternedTyId) -> Option<TypeLayout> {
        let elem_layout = self.layout_ty(elem, span)?;
        Some(tagged_union_layout(&[elem_layout]))
    }

    fn error_union_layout(
        &mut self,
        span: Span,
        error: InternedTyId,
        value: InternedTyId,
    ) -> Option<TypeLayout> {
        let error_layout = self.layout_ty(error, span)?;
        let value_layout = self.layout_ty(value, span)?;
        Some(tagged_union_layout(&[error_layout, value_layout]))
    }

    fn nominal_layout(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<TypeLayout> {
        if def_id.module_id != self.module_id {
            return self.external_nominal_layout(span, def_id, args, const_args);
        }
        if let Some(signature) = self.signatures.structs.get(&def_id.def_id).cloned() {
            return self.struct_layout(span, def_id.def_id, &signature, args, const_args);
        }
        if let Some(signature) = self.signatures.unions.get(&def_id.def_id).cloned() {
            return self.union_layout(span, def_id.def_id, &signature, args, const_args);
        }
        if let Some(signature) = self.signatures.enums.get(&def_id.def_id).cloned() {
            return self.enum_layout(span, Some(def_id.def_id), &signature);
        }
        None
    }

    fn detailed_struct_layout(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<TypeLayout> {
        if def_id.module_id == self.module_id {
            let signature = self.signatures.structs.get(&def_id.def_id)?.clone();
            return self.struct_layout(span, def_id.def_id, &signature, args, const_args);
        }
        let signature = self
            .program
            .structs
            .and_then(|signatures| signatures.get(&def_id).cloned())
            .or_else(|| self.program.struct_.and_then(|query| query(def_id)))?
            .signature;
        self.external_struct_layout(span, def_id, &signature, args, const_args)
    }

    fn detailed_union_layout(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<TypeLayout> {
        if def_id.module_id == self.module_id {
            let signature = self.signatures.unions.get(&def_id.def_id)?.clone();
            return self.union_layout(span, def_id.def_id, &signature, args, const_args);
        }
        let signature = self
            .program
            .unions
            .and_then(|signatures| signatures.get(&def_id).cloned())
            .or_else(|| self.program.union.and_then(|query| query(def_id)))?
            .signature;
        self.external_union_layout(span, def_id, &signature, args, const_args)
    }

    fn external_nominal_layout(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<TypeLayout> {
        if let Some(layouts) = self
            .program
            .layouts
            .and_then(|query| query(def_id.module_id))
            && let Some(layout) =
                layouts.nominal_type_layout_with_const_args(def_id, args, const_args)
        {
            return Some(layout);
        }
        if let Some(program_structs) = self.program.structs
            && let Some(signature) = program_structs.get(&def_id).cloned()
        {
            let signature = signature.signature;
            return self.external_struct_layout(span, def_id, &signature, args, const_args);
        }
        if let Some(program_struct) = self.program.struct_
            && let Some(signature) = program_struct(def_id)
        {
            let signature = signature.signature;
            return self.external_struct_layout(span, def_id, &signature, args, const_args);
        }
        if let Some(program_unions) = self.program.unions
            && let Some(signature) = program_unions.get(&def_id).cloned()
        {
            let signature = signature.signature;
            return self.external_union_layout(span, def_id, &signature, args, const_args);
        }
        if let Some(program_union) = self.program.union
            && let Some(signature) = program_union(def_id)
        {
            let signature = signature.signature;
            return self.external_union_layout(span, def_id, &signature, args, const_args);
        }
        if let Some(program_enums) = self.program.enums
            && let Some(signature) = program_enums.get(&def_id).cloned()
        {
            let signature = signature.signature;
            return self.enum_layout(span, None, &signature);
        }
        if let Some(program_enum) = self.program.enum_
            && let Some(signature) = program_enum(def_id)
        {
            let signature = signature.signature;
            return self.enum_layout(span, None, &signature);
        }
        if let Some(program_type_aliases) = self.program.type_aliases
            && let Some(signature) = program_type_aliases.get(&def_id).cloned()
        {
            let signature = signature.signature;
            return self.type_alias_layout(span, &signature, args);
        }
        if let Some(program_type_alias) = self.program.type_alias
            && let Some(signature) = program_type_alias(def_id)
        {
            let signature = signature.signature;
            return self.type_alias_layout(span, &signature, args);
        }
        None
    }

    fn type_alias_layout(
        &mut self,
        span: Span,
        signature: &TypeAliasSignature,
        args: &[InternedTyId],
    ) -> Option<TypeLayout> {
        if signature.generics.len() != args.len() {
            return None;
        }
        let substitutions: SymbolMap<InternedTyId> = signature
            .generics
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect();
        let target = substitute_layout_ty(
            &self.type_context,
            signature.target,
            &substitutions,
            &SymbolMap::default(),
        );
        self.layout_ty(target, span)
    }

    fn external_struct_layout(
        &mut self,
        _span: Span,
        def_id: GlobalDefId,
        signature: &StructSignature,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<TypeLayout> {
        let key = GlobalStructLayoutKey {
            def_id,
            args: args.to_vec(),
            const_args: const_args.to_vec(),
        };
        if let Some(existing) = self.external_struct_instances.get(&key) {
            return Some(existing.layout.clone());
        }
        let (substitutions, const_substitutions) =
            aggregate_substitutions(&signature.generic_params, args, const_args)?;
        let local_key = StructLayoutKey {
            def_id: def_id.def_id,
            args: args.to_vec(),
            const_args: const_args.to_vec(),
        };
        let struct_layout = if signature.is_extern {
            self.c_struct_layout(&local_key, signature, &substitutions, &const_substitutions)?
        } else {
            self.nia_struct_layout(&local_key, signature, &substitutions, &const_substitutions)?
        };
        let layout = struct_layout.layout.clone();
        self.external_struct_instances.insert(key, struct_layout);
        Some(layout)
    }

    fn external_union_layout(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        signature: &UnionSignature,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<TypeLayout> {
        let key = GlobalStructLayoutKey {
            def_id,
            args: args.to_vec(),
            const_args: const_args.to_vec(),
        };
        if let Some(existing) = self.external_union_instances.get(&key) {
            return Some(existing.layout.clone());
        }
        let (substitutions, const_substitutions) =
            aggregate_substitutions(&signature.generic_params, args, const_args)?;
        if signature.fields.is_empty() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                "union requires at least one field",
            ));
            return None;
        }
        let local_key = StructLayoutKey {
            def_id: def_id.def_id,
            args: args.to_vec(),
            const_args: const_args.to_vec(),
        };
        let union_layout =
            self.union_field_layout(&local_key, signature, &substitutions, &const_substitutions)?;
        let layout = union_layout.layout.clone();
        self.external_union_instances.insert(key, union_layout);
        Some(layout)
    }

    fn enum_layout(
        &mut self,
        span: Span,
        def_id: Option<DefId>,
        signature: &EnumSignature,
    ) -> Option<TypeLayout> {
        if let Some(def_id) = def_id
            && let Some(existing) = self.enums.get(&def_id)
        {
            return Some(existing.layout.clone());
        }
        let tag = self.layout_ty(signature.backing_type, span)?;
        let mut variants = Vec::with_capacity(signature.variants.len());
        for variant in &signature.variants {
            let pending = match &variant.payload {
                EnumVariantPayloadSignature::Unit => Vec::new(),
                EnumVariantPayloadSignature::Tuple(fields) => fields
                    .iter()
                    .map(|ty| {
                        Some(PendingEnumFieldLayout {
                            def_id: None,
                            layout: self.layout_ty(*ty, variant.span)?,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
                EnumVariantPayloadSignature::Named(fields) => fields
                    .iter()
                    .map(|field| {
                        Some(PendingEnumFieldLayout {
                            def_id: Some(field.def_id),
                            layout: self.layout_ty(field.ty, field.span)?,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            };
            let (payload, fields) = place_enum_fields(pending);
            variants.push(EnumVariantLayout {
                def_id: variant.def_id,
                payload,
                fields,
            });
        }
        let has_payload = variants.iter().any(|variant| !variant.fields.is_empty());
        let layout = if has_payload {
            tagged_union_layout_with_tag(
                &tag,
                &variants
                    .iter()
                    .map(|variant| variant.payload.clone())
                    .collect::<Vec<_>>(),
            )
        } else {
            tag.clone()
        };
        let payload_offset = has_payload.then(|| {
            let payload_align = variants
                .iter()
                .map(|variant| variant.payload.align)
                .max()
                .unwrap_or(1);
            align_to(tag.size, payload_align)
        });
        let enum_layout = EnumLayout {
            layout: layout.clone(),
            tag,
            payload_offset,
            variants,
        };
        if let Some(def_id) = def_id {
            self.enums.insert(def_id, enum_layout);
        }
        Some(layout)
    }

    fn struct_layout(
        &mut self,
        span: Span,
        def_id: DefId,
        signature: &StructSignature,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<TypeLayout> {
        let key = StructLayoutKey {
            def_id,
            args: args.to_vec(),
            const_args: const_args.to_vec(),
        };
        if let Some(existing) = self.struct_instances.get(&key) {
            return Some(existing.layout.clone());
        }
        let (substitutions, const_substitutions) =
            aggregate_substitutions(&signature.generic_params, args, const_args)?;
        if !self.visiting_structs.insert(key.clone()) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                "recursive struct layout is not supported",
            ));
            return None;
        }
        let struct_layout = if signature.is_extern {
            self.c_struct_layout(&key, signature, &substitutions, &const_substitutions)?
        } else {
            self.nia_struct_layout(&key, signature, &substitutions, &const_substitutions)?
        };
        let layout = struct_layout.layout.clone();
        if key.args.is_empty() && key.const_args.is_empty() {
            self.structs.insert(def_id, struct_layout.clone());
        }
        self.struct_instances.insert(key.clone(), struct_layout);
        self.visiting_structs.remove(&key);
        Some(layout)
    }

    fn union_layout(
        &mut self,
        span: Span,
        def_id: DefId,
        signature: &UnionSignature,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<TypeLayout> {
        let key = StructLayoutKey {
            def_id,
            args: args.to_vec(),
            const_args: const_args.to_vec(),
        };
        if let Some(existing) = self.union_instances.get(&key) {
            return Some(existing.layout.clone());
        }
        let (substitutions, const_substitutions) =
            aggregate_substitutions(&signature.generic_params, args, const_args)?;
        if signature.fields.is_empty() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                "union requires at least one field",
            ));
            return None;
        }
        if !self.visiting_unions.insert(key.clone()) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                "recursive union layout is not supported",
            ));
            return None;
        }
        let union_layout =
            self.union_field_layout(&key, signature, &substitutions, &const_substitutions)?;
        let layout = union_layout.layout.clone();
        if key.args.is_empty() && key.const_args.is_empty() {
            self.unions.insert(def_id, union_layout.clone());
        }
        self.union_instances.insert(key.clone(), union_layout);
        self.visiting_unions.remove(&key);
        Some(layout)
    }

    fn nia_struct_layout(
        &mut self,
        key: &StructLayoutKey,
        signature: &StructSignature,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Option<StructLayout> {
        self.sorted_field_layout(key, signature, substitutions, const_substitutions)
    }

    fn c_struct_layout(
        &mut self,
        key: &StructLayoutKey,
        signature: &StructSignature,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Option<StructLayout> {
        self.field_order_layout(key, signature, substitutions, const_substitutions)
    }

    fn field_order_layout(
        &mut self,
        key: &StructLayoutKey,
        signature: &StructSignature,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Option<StructLayout> {
        let fields =
            self.layout_fields(key, &signature.fields, substitutions, const_substitutions)?;
        Some(place_struct_fields(fields))
    }

    fn sorted_field_layout(
        &mut self,
        key: &StructLayoutKey,
        signature: &StructSignature,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Option<StructLayout> {
        let mut fields =
            self.layout_fields(key, &signature.fields, substitutions, const_substitutions)?;
        fields.sort_by(|left, right| {
            right
                .layout
                .align
                .cmp(&left.layout.align)
                .then_with(|| right.layout.size.cmp(&left.layout.size))
                .then_with(|| left.source_index.cmp(&right.source_index))
        });
        Some(place_struct_fields(fields))
    }

    fn layout_fields(
        &mut self,
        key: &StructLayoutKey,
        fields: &[nia_item_signatures::FieldSignature],
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Option<Vec<PendingFieldLayout>> {
        let mut layouts = Vec::new();
        for (source_index, field) in fields.iter().enumerate() {
            let field_ty = self.normalize_ty(field.ty);
            let field_ty = substitute_layout_ty(
                &self.type_context,
                field_ty,
                substitutions,
                const_substitutions,
            );
            let Some(field_layout) = self.layout_ty(field_ty, field.span) else {
                self.visiting_structs.remove(key);
                return None;
            };
            layouts.push(PendingFieldLayout {
                def_id: field.def_id,
                source_index,
                layout: field_layout,
            });
        }
        Some(layouts)
    }

    fn union_field_layout(
        &mut self,
        key: &StructLayoutKey,
        signature: &UnionSignature,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Option<StructLayout> {
        let mut fields = Vec::new();
        for field in &signature.fields {
            let field_ty = self.normalize_ty(field.ty);
            let field_ty = substitute_layout_ty(
                &self.type_context,
                field_ty,
                substitutions,
                const_substitutions,
            );
            let Some(field_layout) = self.layout_ty(field_ty, field.span) else {
                self.visiting_unions.remove(key);
                return None;
            };
            fields.push(FieldLayout {
                def_id: field.def_id,
                offset: 0,
                layout: field_layout,
            });
        }
        let layout = union_layout_from_fields(fields.iter().map(|field| &field.layout));
        Some(StructLayout { layout, fields })
    }
}

impl LayoutComputer<'_> {
    fn normalize_ty(&self, ty_id: InternedTyId) -> InternedTyId {
        self.normalized.get(&ty_id).copied().unwrap_or(ty_id)
    }
}

fn substitute_layout_ty(
    types: &LayoutTypeCx<'_>,
    ty: InternedTyId,
    substitutions: &SymbolMap<InternedTyId>,
    const_substitutions: &SymbolMap<ConstGenericArg>,
) -> InternedTyId {
    nia_ty::substitute_ty(
        types.store,
        &types.append,
        ty,
        &|name| substitutions.get(name).copied(),
        &|name| const_substitutions.get(name).cloned(),
        None,
    )
}

fn aggregate_substitutions(
    params: &[GenericParamSignature],
    args: &[InternedTyId],
    const_args: &[ConstGenericArg],
) -> Option<(SymbolMap<InternedTyId>, SymbolMap<ConstGenericArg>)> {
    let mut type_index = 0;
    let mut const_index = 0;
    let mut substitutions = SymbolMap::default();
    let mut const_substitutions = SymbolMap::default();
    for param in params {
        match param.kind {
            GenericParamSignatureKind::Type => {
                substitutions.insert(param.name, *args.get(type_index)?);
                type_index += 1;
            }
            GenericParamSignatureKind::Const { .. } => {
                const_substitutions.insert(param.name, const_args.get(const_index)?.clone());
                const_index += 1;
            }
        }
    }
    (type_index == args.len() && const_index == const_args.len())
        .then_some((substitutions, const_substitutions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_const_check::{
        ConstCheck, ConstInput, ConstModuleInput, ConstProgramContext, check_module_const,
        lower_module_const,
    };
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_ids::ModuleIdAllocator;
    use nia_item_signatures::{ItemSignatureInput, ItemSignatureSource, collect_item_signatures};
    use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
    use nia_local_resolve::resolve_module_locals;
    use nia_parser::parse_module_with_symbols;
    use nia_sema_ir::SemanticUseTable;
    use nia_source::SourcePath;
    use nia_symbol::stable_hash;
    use nia_symbol_table::SymbolTable;
    use nia_ty::{PrimitiveTy, TyKind, TypeStore};
    use nia_type_lower::{TypeLoweringContext, lower_module_types_with_context};
    use nia_type_resolve::resolve_module_types_with_symbols;
    use nia_value_resolve::resolve_module_values;

    #[test]
    fn target_data_layout_rejects_non_byte_pointer_widths() {
        assert_eq!(TargetDataLayout::from_pointer_width(9), None);
        assert_eq!(
            TargetDataLayout::from_pointer_width(64),
            Some(TargetDataLayout::LP64)
        );
    }

    #[test]
    fn array_layout_rejects_size_overflow() {
        let element = TypeLayout {
            size: u64::MAX,
            align: 8,
        };
        assert_eq!(array_layout(&element, 2), None);
        assert_eq!(
            array_layout(&TypeLayout { size: 4, align: 4 }, 3),
            Some(TypeLayout { size: 12, align: 4 })
        );
    }

    #[test]
    fn vector_layout_uses_native_bit_width_and_allocation_alignment() {
        assert_eq!(
            vector_layout(PrimitiveTy::Bool, 16, TargetDataLayout::LP64),
            Some(TypeLayout { size: 2, align: 2 })
        );
        assert_eq!(
            vector_layout(PrimitiveTy::U8, 16, TargetDataLayout::LP64),
            Some(TypeLayout {
                size: 16,
                align: 16,
            })
        );
        assert_eq!(
            vector_layout(PrimitiveTy::F32, 3, TargetDataLayout::LP64),
            Some(TypeLayout {
                size: 16,
                align: 16,
            })
        );
        assert_eq!(
            vector_layout(PrimitiveTy::Usize, 2, TargetDataLayout::LP64),
            Some(TypeLayout {
                size: 16,
                align: 16,
            })
        );
    }

    include!("tests/layout/test_support.rs");

    #[path = "layout/basic_layouts.rs"]
    mod basic_layouts;

    #[path = "layout/array_lengths.rs"]
    mod array_lengths;

    #[path = "layout/field_layouts.rs"]
    mod field_layouts;

    #[path = "layout/aggregate_instances.rs"]
    mod aggregate_instances;

    #[path = "layout/enum_layouts.rs"]
    mod enum_layouts;
}
