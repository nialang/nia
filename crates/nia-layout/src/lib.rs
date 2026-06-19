// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_defs::{DefCollection, DefId};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_item_signatures::{
    EnumSignature, FieldSignature, ItemSignatures, ProgramStructSignature, ProgramUnionSignature,
    StructSignature, UnionSignature,
};
use nia_span::Span;
use nia_ty::{ArrayLenTy, LayoutBuiltin, PrimitiveTy, RangeTyKind, TyInterner, TyKind};

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
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GlobalStructLayoutKey {
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldLayout {
    pub def_id: DefId,
    pub offset: u64,
    pub layout: TypeLayout,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layouts {
    pub target: TargetDataLayout,
    pub interner: TyInterner,
    pub types: HashMap<InternedTyId, TypeLayout>,
    pub structs: HashMap<DefId, StructLayout>,
    pub unions: HashMap<DefId, StructLayout>,
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
        if args.is_empty() {
            self.structs
                .get(&def_id.def_id)
                .map(|layout| layout.layout.clone())
                .or_else(|| {
                    self.unions
                        .get(&def_id.def_id)
                        .map(|layout| layout.layout.clone())
                })
        } else {
            let key = StructLayoutKey {
                def_id: def_id.def_id,
                args: args.to_vec(),
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
        self.nominal_struct_layout(def_id, args).and_then(|layout| {
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
        if args.is_empty() {
            self.structs
                .get(&def_id.def_id)
                .or_else(|| self.unions.get(&def_id.def_id))
        } else {
            let key = StructLayoutKey {
                def_id: def_id.def_id,
                args: args.to_vec(),
            };
            self.struct_instances
                .get(&key)
                .or_else(|| self.union_instances.get(&key))
        }
    }
}

pub fn compute_layouts(
    defs: &DefCollection,
    interner: &TyInterner,
    signatures: &ItemSignatures,
    target: TargetDataLayout,
) -> Layouts {
    let normalized = HashMap::new();
    let empty_lengths = NoArrayLengthValues;
    compute_layouts_with_normalized_types(
        defs,
        interner,
        signatures,
        &normalized,
        &empty_lengths,
        target,
    )
}

pub fn compute_layouts_with_normalized_types(
    defs: &DefCollection,
    interner: &TyInterner,
    signatures: &ItemSignatures,
    normalized: &HashMap<InternedTyId, InternedTyId>,
    array_lengths: &dyn ArrayLengthValues,
    target: TargetDataLayout,
) -> Layouts {
    compute_layouts_with_program_context(
        defs,
        interner,
        signatures,
        normalized,
        array_lengths,
        target,
        ProgramLayoutContext::default(),
    )
}

#[derive(Clone, Copy, Default)]
pub struct ProgramLayoutContext<'a> {
    pub layouts: Option<&'a dyn Fn(ModuleId) -> Option<Layouts>>,
    pub array_lengths: Option<&'a dyn Fn(GlobalConstExprId) -> Option<u64>>,
    pub structs: Option<&'a HashMap<GlobalDefId, ProgramStructSignature>>,
    pub unions: Option<&'a HashMap<GlobalDefId, ProgramUnionSignature>>,
}

#[derive(Clone, Copy)]
pub struct InstanceLayoutRequest<'a> {
    pub def_id: GlobalDefId,
    pub args: &'a [InternedTyId],
}

#[derive(Clone, Copy)]
pub struct LayoutComputationInput<'a> {
    pub defs: &'a DefCollection,
    pub interner: &'a TyInterner,
    pub signatures: &'a ItemSignatures,
    pub normalized: &'a HashMap<InternedTyId, InternedTyId>,
    pub array_lengths: &'a dyn ArrayLengthValues,
    pub target: TargetDataLayout,
    pub program: ProgramLayoutContext<'a>,
}

pub fn compute_layouts_with_program_context(
    defs: &DefCollection,
    interner: &TyInterner,
    signatures: &ItemSignatures,
    normalized: &HashMap<InternedTyId, InternedTyId>,
    array_lengths: &dyn ArrayLengthValues,
    target: TargetDataLayout,
    program: ProgramLayoutContext<'_>,
) -> Layouts {
    LayoutComputer::new(
        defs,
        interner,
        signatures,
        normalized,
        array_lengths,
        target,
        program,
    )
    .compute()
}

pub fn compute_struct_instance_layout_with_program_context(
    input: LayoutComputationInput<'_>,
    request: InstanceLayoutRequest<'_>,
) -> Option<StructLayout> {
    let mut computer = LayoutComputer::new(
        input.defs,
        input.interner,
        input.signatures,
        input.normalized,
        input.array_lengths,
        input.target,
        input.program,
    );
    computer.nominal_layout(Span::default(), request.def_id, request.args)?;
    if request.def_id.module_id != input.defs.module_id {
        return computer
            .external_struct_instances
            .get(&GlobalStructLayoutKey {
                def_id: request.def_id,
                args: request.args.to_vec(),
            })
            .cloned();
    }
    computer
        .struct_instances
        .get(&StructLayoutKey {
            def_id: request.def_id.def_id,
            args: request.args.to_vec(),
        })
        .cloned()
}

pub fn compute_union_instance_layout_with_program_context(
    input: LayoutComputationInput<'_>,
    request: InstanceLayoutRequest<'_>,
) -> Option<StructLayout> {
    let mut computer = LayoutComputer::new(
        input.defs,
        input.interner,
        input.signatures,
        input.normalized,
        input.array_lengths,
        input.target,
        input.program,
    );
    computer.nominal_layout(Span::default(), request.def_id, request.args)?;
    if request.def_id.module_id != input.defs.module_id {
        return computer
            .external_union_instances
            .get(&GlobalStructLayoutKey {
                def_id: request.def_id,
                args: request.args.to_vec(),
            })
            .cloned();
    }
    computer
        .union_instances
        .get(&StructLayoutKey {
            def_id: request.def_id.def_id,
            args: request.args.to_vec(),
        })
        .cloned()
}

struct LayoutComputer<'a> {
    module_id: nia_ids::ModuleId,
    interner: TyInterner,
    signatures: &'a ItemSignatures,
    normalized: &'a HashMap<InternedTyId, InternedTyId>,
    array_lengths: &'a dyn ArrayLengthValues,
    target: TargetDataLayout,
    types: HashMap<InternedTyId, TypeLayout>,
    structs: HashMap<DefId, StructLayout>,
    unions: HashMap<DefId, StructLayout>,
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
    fn new(
        defs: &DefCollection,
        interner: &TyInterner,
        signatures: &'a ItemSignatures,
        normalized: &'a HashMap<InternedTyId, InternedTyId>,
        array_lengths: &'a dyn ArrayLengthValues,
        target: TargetDataLayout,
        program: ProgramLayoutContext<'a>,
    ) -> Self {
        Self {
            module_id: defs.module_id,
            interner: interner.clone(),
            signatures,
            normalized,
            array_lengths,
            target,
            types: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            struct_instances: HashMap::new(),
            union_instances: HashMap::new(),
            external_struct_instances: HashMap::new(),
            external_union_instances: HashMap::new(),
            diagnostics: Vec::new(),
            visiting: HashSet::new(),
            visiting_structs: HashSet::new(),
            visiting_unions: HashSet::new(),
            program,
        }
    }

    fn compute(mut self) -> Layouts {
        let mut next = 0usize;
        while next < self.interner.len() {
            let ty_ids = self
                .interner
                .iter()
                .map(|(ty_id, _)| ty_id)
                .collect::<Vec<_>>();
            let Some(ty_id) = ty_ids.get(next).copied() else {
                break;
            };
            next += 1;
            if self.is_inferred_array_type(ty_id) {
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
            self.struct_layout(signature.span, def_id, &signature, &[]);
        }
        let union_signatures: Vec<(DefId, UnionSignature)> = self
            .signatures
            .unions
            .iter()
            .filter(|(_, signature)| signature.generics.is_empty())
            .map(|(def_id, signature)| (*def_id, signature.clone()))
            .collect();
        for (def_id, signature) in union_signatures {
            self.union_layout(signature.span, def_id, &signature, &[]);
        }
        Layouts {
            target: self.target,
            interner: self.interner,
            types: self.types,
            structs: self.structs,
            unions: self.unions,
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
                "E0501",
                span,
                "recursive type layout is not supported",
            ));
            return None;
        }
        let layout = match self.interner.get(ty_id).cloned() {
            Some(TyKind::Primitive(primitive)) => self.primitive_layout(primitive),
            Some(TyKind::Vector { elem, lanes }) => self.vector_layout(span, elem, lanes),
            Some(
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::FunctionPointer { .. },
            ) => Some(TypeLayout {
                size: self.target.pointer_size,
                align: self.target.pointer_align,
            }),
            Some(TyKind::Slice { .. } | TyKind::TraitObject { .. }) => Some(TypeLayout {
                size: self.target.pointer_size * 2,
                align: self.target.pointer_align,
            }),
            Some(TyKind::SlicePointee { .. } | TyKind::TraitObjectPointee { .. }) => None,
            Some(TyKind::Range { kind, bound }) => self.range_layout(span, kind, bound),
            Some(TyKind::Array { len, elem }) => self.array_layout(span, len, elem),
            Some(TyKind::Optional { elem }) => self.optional_layout(span, elem),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.error_union_layout(span, error, value)
            }
            Some(TyKind::Nominal { def_id, args }) => self.nominal_layout(span, def_id, &args),
            Some(
                TyKind::Error
                | TyKind::ComptimeOnly
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
            self.interner.get(ty_id),
            Some(TyKind::Array {
                len: ArrayLenTy::Infer,
                ..
            })
        )
    }

    fn primitive_layout(&self, primitive: PrimitiveTy) -> Option<TypeLayout> {
        let (size, align) = match primitive {
            PrimitiveTy::I8 | PrimitiveTy::U8 | PrimitiveTy::Bool => (1, 1),
            PrimitiveTy::I16 | PrimitiveTy::U16 => (2, 2),
            PrimitiveTy::I32 | PrimitiveTy::U32 | PrimitiveTy::F32 | PrimitiveTy::Char => (4, 4),
            PrimitiveTy::I64 | PrimitiveTy::U64 | PrimitiveTy::F64 => (8, 8),
            PrimitiveTy::I128 | PrimitiveTy::U128 => (16, 16),
            PrimitiveTy::Isize | PrimitiveTy::Usize => {
                (self.target.pointer_size, self.target.pointer_align)
            }
            PrimitiveTy::Void | PrimitiveTy::Never => (0, 1),
        };
        Some(TypeLayout { size, align })
    }

    fn vector_layout(&mut self, span: Span, elem: PrimitiveTy, lanes: u32) -> Option<TypeLayout> {
        let elem_layout = self.primitive_layout(elem)?;
        let Some(size) = elem_layout.size.checked_mul(lanes as u64) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0501",
                span,
                "SIMD vector layout size overflowed",
            ));
            return None;
        };
        Some(TypeLayout {
            size,
            align: elem_layout.align,
        })
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
                    "E0501",
                    span,
                    "array layout requires a concrete length",
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
                        "E0501",
                        span,
                        "array length was not evaluated by comptime",
                    ));
                    return None;
                };
                value
            }
            ArrayLenTy::Builtin { builtin, ty } => {
                let Some(layout) = self.layout_ty(ty, span) else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        "E0501",
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
        Some(TypeLayout {
            size: elem_layout.size.saturating_mul(len),
            align: elem_layout.align,
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
    ) -> Option<TypeLayout> {
        if def_id.module_id != self.module_id {
            return self.external_nominal_layout(span, def_id, args);
        }
        if let Some(signature) = self.signatures.structs.get(&def_id.def_id).cloned() {
            return self.struct_layout(span, def_id.def_id, &signature, args);
        }
        if let Some(signature) = self.signatures.unions.get(&def_id.def_id).cloned() {
            return self.union_layout(span, def_id.def_id, &signature, args);
        }
        if let Some(signature) = self.signatures.enums.get(&def_id.def_id).cloned() {
            return self.enum_layout(span, &signature);
        }
        None
    }

    fn external_nominal_layout(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<TypeLayout> {
        if let Some(layouts) = self
            .program
            .layouts
            .and_then(|query| query(def_id.module_id))
            && let Some(layout) = layouts.nominal_type_layout(def_id, args)
        {
            return Some(layout);
        }
        if let Some(program_structs) = self.program.structs
            && let Some(signature) = program_structs.get(&def_id).cloned()
        {
            let signature = import_struct_signature(&mut self.interner, &signature);
            return self.external_struct_layout(span, def_id, &signature, args);
        }
        if let Some(program_unions) = self.program.unions
            && let Some(signature) = program_unions.get(&def_id).cloned()
        {
            let signature = import_union_signature(&mut self.interner, &signature);
            return self.external_union_layout(span, def_id, &signature, args);
        }
        None
    }

    fn external_struct_layout(
        &mut self,
        _span: Span,
        def_id: GlobalDefId,
        signature: &StructSignature,
        args: &[InternedTyId],
    ) -> Option<TypeLayout> {
        let key = GlobalStructLayoutKey {
            def_id,
            args: args.to_vec(),
        };
        if let Some(existing) = self.external_struct_instances.get(&key) {
            return Some(existing.layout.clone());
        }
        if signature.generics.len() != args.len() {
            return None;
        }
        let substitutions: HashMap<String, InternedTyId> = signature
            .generics
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect();
        let local_key = StructLayoutKey {
            def_id: def_id.def_id,
            args: args.to_vec(),
        };
        let struct_layout = if signature.is_extern {
            self.c_struct_layout(&local_key, signature, &substitutions)?
        } else {
            self.nia_struct_layout(&local_key, signature, &substitutions)?
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
    ) -> Option<TypeLayout> {
        let key = GlobalStructLayoutKey {
            def_id,
            args: args.to_vec(),
        };
        if let Some(existing) = self.external_union_instances.get(&key) {
            return Some(existing.layout.clone());
        }
        if signature.generics.len() != args.len() {
            return None;
        }
        if signature.fields.is_empty() {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0501",
                span,
                "union requires at least one field",
            ));
            return None;
        }
        let substitutions: HashMap<String, InternedTyId> = signature
            .generics
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect();
        let local_key = StructLayoutKey {
            def_id: def_id.def_id,
            args: args.to_vec(),
        };
        let union_layout = self.union_field_layout(&local_key, signature, &substitutions)?;
        let layout = union_layout.layout.clone();
        self.external_union_instances.insert(key, union_layout);
        Some(layout)
    }

    fn enum_layout(&mut self, span: Span, signature: &EnumSignature) -> Option<TypeLayout> {
        self.layout_ty(signature.backing_type, span)
    }

    fn struct_layout(
        &mut self,
        span: Span,
        def_id: DefId,
        signature: &StructSignature,
        args: &[InternedTyId],
    ) -> Option<TypeLayout> {
        let key = StructLayoutKey {
            def_id,
            args: args.to_vec(),
        };
        if let Some(existing) = self.struct_instances.get(&key) {
            return Some(existing.layout.clone());
        }
        if signature.generics.len() != args.len() {
            return None;
        }
        if !self.visiting_structs.insert(key.clone()) {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0501",
                span,
                "recursive struct layout is not supported",
            ));
            return None;
        }
        let substitutions: HashMap<String, InternedTyId> = signature
            .generics
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect();
        let struct_layout = if signature.is_extern {
            self.c_struct_layout(&key, signature, &substitutions)?
        } else {
            self.nia_struct_layout(&key, signature, &substitutions)?
        };
        let layout = struct_layout.layout.clone();
        if key.args.is_empty() {
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
    ) -> Option<TypeLayout> {
        let key = StructLayoutKey {
            def_id,
            args: args.to_vec(),
        };
        if let Some(existing) = self.union_instances.get(&key) {
            return Some(existing.layout.clone());
        }
        if signature.generics.len() != args.len() {
            return None;
        }
        if signature.fields.is_empty() {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0501",
                span,
                "union requires at least one field",
            ));
            return None;
        }
        if !self.visiting_unions.insert(key.clone()) {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0501",
                span,
                "recursive union layout is not supported",
            ));
            return None;
        }
        let substitutions: HashMap<String, InternedTyId> = signature
            .generics
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect();
        let union_layout = self.union_field_layout(&key, signature, &substitutions)?;
        let layout = union_layout.layout.clone();
        if key.args.is_empty() {
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
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<StructLayout> {
        self.sorted_field_layout(key, signature, substitutions)
    }

    fn c_struct_layout(
        &mut self,
        key: &StructLayoutKey,
        signature: &StructSignature,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<StructLayout> {
        self.field_order_layout(key, signature, substitutions)
    }

    fn field_order_layout(
        &mut self,
        key: &StructLayoutKey,
        signature: &StructSignature,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<StructLayout> {
        let fields = self.layout_fields(key, &signature.fields, substitutions)?;
        Some(place_struct_fields(fields))
    }

    fn sorted_field_layout(
        &mut self,
        key: &StructLayoutKey,
        signature: &StructSignature,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<StructLayout> {
        let mut fields = self.layout_fields(key, &signature.fields, substitutions)?;
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
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<Vec<PendingFieldLayout>> {
        let mut layouts = Vec::new();
        for (source_index, field) in fields.iter().enumerate() {
            let field_ty = self.normalize_ty(field.ty);
            let field_ty = substitute_generics(&mut self.interner, field_ty, substitutions);
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
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Option<StructLayout> {
        let mut fields = Vec::new();
        let mut max_size = 0u64;
        let mut max_align = 1u64;
        for field in &signature.fields {
            let field_ty = self.normalize_ty(field.ty);
            let field_ty = substitute_generics(&mut self.interner, field_ty, substitutions);
            let Some(field_layout) = self.layout_ty(field_ty, field.span) else {
                self.visiting_unions.remove(key);
                return None;
            };
            max_size = max_size.max(field_layout.size);
            max_align = max_align.max(field_layout.align);
            fields.push(FieldLayout {
                def_id: field.def_id,
                offset: 0,
                layout: field_layout,
            });
        }
        let layout = TypeLayout {
            size: align_to(max_size, max_align),
            align: max_align,
        };
        Some(StructLayout { layout, fields })
    }
}

fn import_struct_signature(
    target: &mut TyInterner,
    source: &ProgramStructSignature,
) -> StructSignature {
    StructSignature {
        generics: source.signature.generics.clone(),
        where_predicates: source.signature.where_predicates.clone(),
        fields: import_fields(target, &source.interner, &source.signature.fields),
        is_extern: source.signature.is_extern,
        span: source.signature.span,
    }
}

fn import_union_signature(
    target: &mut TyInterner,
    source: &ProgramUnionSignature,
) -> UnionSignature {
    UnionSignature {
        generics: source.signature.generics.clone(),
        where_predicates: source.signature.where_predicates.clone(),
        fields: import_fields(target, &source.interner, &source.signature.fields),
        is_extern: source.signature.is_extern,
        span: source.signature.span,
    }
}

fn import_fields(
    target: &mut TyInterner,
    source: &TyInterner,
    fields: &[FieldSignature],
) -> Vec<FieldSignature> {
    fields
        .iter()
        .map(|field| FieldSignature {
            def_id: field.def_id,
            name: field.name.clone(),
            ty: nia_ty::import_type_into(target, source, field.ty),
            span: field.span,
        })
        .collect()
}

#[derive(Debug, Clone)]
struct PendingFieldLayout {
    def_id: DefId,
    source_index: usize,
    layout: TypeLayout,
}

fn place_struct_fields(fields: Vec<PendingFieldLayout>) -> StructLayout {
    let mut placed = Vec::new();
    let mut offset = 0u64;
    let mut max_align = 1u64;
    for field in fields {
        offset = align_to(offset, field.layout.align);
        placed.push(FieldLayout {
            def_id: field.def_id,
            offset,
            layout: field.layout.clone(),
        });
        offset = offset.saturating_add(field.layout.size);
        max_align = max_align.max(field.layout.align);
    }
    let layout = TypeLayout {
        size: align_to(offset, max_align),
        align: max_align,
    };
    StructLayout {
        layout,
        fields: placed,
    }
}

impl LayoutComputer<'_> {
    fn normalize_ty(&self, ty_id: InternedTyId) -> InternedTyId {
        self.normalized.get(&ty_id).copied().unwrap_or(ty_id)
    }
}

fn substitute_generics(
    interner: &mut TyInterner,
    ty: InternedTyId,
    substitutions: &HashMap<String, InternedTyId>,
) -> InternedTyId {
    match interner.get(ty).cloned() {
        Some(TyKind::GenericParam(name)) => substitutions.get(&name).copied().unwrap_or(ty),
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let elem = substitute_generics(interner, elem, substitutions);
            interner.intern(TyKind::Pointer { is_readonly, elem })
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let elem = substitute_generics(interner, elem, substitutions);
            interner.intern(TyKind::Slice { is_readonly, elem })
        }
        Some(TyKind::Array { len, elem }) => {
            let elem = substitute_generics(interner, elem, substitutions);
            let len = substitute_array_len_generics(interner, len, substitutions);
            interner.intern(TyKind::Array { len, elem })
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let params = params
                .into_iter()
                .map(|param| substitute_generics(interner, param, substitutions))
                .collect();
            let return_type = substitute_generics(interner, return_type, substitutions);
            interner.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            })
        }
        Some(TyKind::Optional { elem }) => {
            let elem = substitute_generics(interner, elem, substitutions);
            interner.intern(TyKind::Optional { elem })
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            let error = substitute_generics(interner, error, substitutions);
            let value = substitute_generics(interner, value, substitutions);
            interner.intern(TyKind::ErrorUnion { error, value })
        }
        Some(TyKind::Nominal { def_id, args }) => {
            let args = args
                .into_iter()
                .map(|arg| substitute_generics(interner, arg, substitutions))
                .collect();
            interner.intern(TyKind::Nominal { def_id, args })
        }
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let args = args
                .into_iter()
                .map(|arg| substitute_generics(interner, arg, substitutions))
                .collect();
            interner.intern(TyKind::BuiltinTrait { trait_id, args })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            name,
        }) => {
            let self_ty = substitute_generics(interner, self_ty, substitutions);
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_generics(interner, arg, substitutions))
                .collect();
            interner.intern(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            })
        }
        _ => ty,
    }
}

fn substitute_array_len_generics(
    interner: &mut TyInterner,
    len: ArrayLenTy,
    substitutions: &HashMap<String, InternedTyId>,
) -> ArrayLenTy {
    match len {
        ArrayLenTy::Builtin { builtin, ty } => ArrayLenTy::Builtin {
            builtin,
            ty: substitute_generics(interner, ty, substitutions),
        },
        ArrayLenTy::Infer | ArrayLenTy::ConstValue(_) | ArrayLenTy::ConstExpr(_) => len,
    }
}

fn align_to(value: u64, align: u64) -> u64 {
    if align <= 1 {
        value
    } else {
        value.div_ceil(align) * align
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

#[cfg(test)]
mod tests {
    use super::*;
    use nia_comptime_check::{
        ComptimeCheck, ComptimeInput, ComptimeModuleInput, ComptimeProgramContext,
        check_module_comptime, lower_module_comptime,
    };
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_item_signatures::collect_item_signatures;
    use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
    use nia_local_resolve::resolve_module_locals;
    use nia_parser::parse_module;
    use nia_sema_ir::SemanticUseTable;
    use nia_ty::{PrimitiveTy, TyKind};
    use nia_type_lower::lower_module_types_with_id;
    use nia_type_resolve::resolve_module_types;
    use nia_value_resolve::resolve_module_values;

    fn compute_test_comptime(
        module: &nia_ast::Module,
        defs: &nia_defs::DefCollection,
        signatures: &ItemSignatures,
        lowered: &nia_type_lower::TypeLowering,
    ) -> ComptimeCheck {
        let values = resolve_module_values(module, defs);
        let locals = resolve_module_locals(module, defs, &values);
        let semantic_uses = semantic_use_table(ModuleId(0), &values, &locals, lowered);
        let target = nia_target_config::TargetConfig::host();
        let item_tree = ModuleItemTree::from_module(module);
        let active_item_tree = ActiveModuleItemTree::new(
            item_tree.active_items_without_comptime(),
            Default::default(),
        );
        let comptime_module = lower_module_comptime(ComptimeModuleInput {
            active_item_tree: &active_item_tree,
            defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            const_exprs: &lowered.const_exprs,
        });
        assert!(
            comptime_module.diagnostics.is_empty(),
            "{:?}",
            comptime_module.diagnostics
        );
        check_module_comptime(ComptimeInput {
            module: &comptime_module.module,
            defs,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            signatures,
            interner: &lowered.interner,
            normalized: &HashMap::new(),
            target: &target,
            program: ComptimeProgramContext::empty(),
        })
    }

    fn semantic_use_table(
        module_id: ModuleId,
        values: &nia_value_resolve::ValueResolution,
        locals: &nia_local_resolve::LocalResolution,
        lowered: &nia_type_lower::TypeLowering,
    ) -> SemanticUseTable {
        let mut builder = SemanticUseTable::builder();
        for (key, local_use) in &locals.node_uses {
            if let nia_local_resolve::LocalUse::Local(local_id) = local_use {
                builder.insert_node_local_value_use(key.clone(), *local_id);
            }
        }
        builder.extend_node_global_value_uses(
            values
                .node_qualified_values
                .iter()
                .map(|(key, global_id)| (key.clone(), *global_id)),
        );
        for (key, resolution) in &values.node_names {
            match resolution {
                nia_value_resolve::ValueNameResolution::Def(def_id) => {
                    builder.insert_node_global_value_use(
                        key.clone(),
                        nia_ids::GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                    );
                }
                nia_value_resolve::ValueNameResolution::External(global_id) => {
                    builder.insert_node_global_value_use(key.clone(), *global_id);
                }
                nia_value_resolve::ValueNameResolution::Module
                | nia_value_resolve::ValueNameResolution::LocalDeferred
                | nia_value_resolve::ValueNameResolution::Error => {}
            }
        }
        builder.extend_node_local_defs(
            locals
                .node_local_defs
                .iter()
                .map(|(key, local_id)| (key.clone(), *local_id)),
        );
        builder.extend_node_type_uses(
            lowered
                .node_type_uses
                .iter()
                .map(|(key, ty)| (key.clone(), *ty)),
        );
        builder.finish()
    }

    #[test]
    fn computes_primitive_pointer_array_and_struct_layouts() {
        let (module, errors) = parse_module(
            r#"
struct Pair {
    a: u8,
    b: i32,
}

fn main(p: &Pair, xs: [3]u16) {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let comptime = compute_test_comptime(&module, &defs, &signatures, &lowered);
        let layouts = compute_layouts_with_normalized_types(
            &defs,
            &lowered.interner,
            &signatures,
            &HashMap::new(),
            &|id| comptime.array_lengths.get(&id).copied(),
            TargetDataLayout::LP64,
        );
        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        assert_eq!(
            layouts
                .types
                .get(&lowered.interner.primitive(PrimitiveTy::U8))
                .expect("u8 layout"),
            &TypeLayout { size: 1, align: 1 }
        );
        assert!(lowered.interner.iter().any(|(ty_id, ty)| {
            matches!(ty, TyKind::Pointer { .. })
                && layouts.types.get(&ty_id) == Some(&TypeLayout { size: 8, align: 8 })
        }));
        assert!(lowered.interner.iter().any(|(ty_id, ty)| {
            matches!(ty, TyKind::Array { .. })
                && layouts.types.get(&ty_id) == Some(&TypeLayout { size: 6, align: 2 })
        }));
        let pair_id = defs.module_scope.types.get("Pair").expect("Pair def");
        let pair = layouts.structs.get(&pair_id).expect("Pair layout");
        assert_eq!(pair.layout, TypeLayout { size: 8, align: 4 });
        assert_eq!(pair.fields[0].offset, 0);
        assert_eq!(pair.fields[1].offset, 4);
    }

    #[test]
    fn computes_layout_builtin_array_lengths() {
        let (module, errors) = parse_module(
            r#"
struct Pair {
    a: u8,
    b: i32,
}

fn main(xs: [@size[Pair]()]u8, ys: [@align[Pair]()]u8) {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let comptime = compute_test_comptime(&module, &defs, &signatures, &lowered);
        let layouts = compute_layouts_with_normalized_types(
            &defs,
            &lowered.interner,
            &signatures,
            &HashMap::new(),
            &|id| comptime.array_lengths.get(&id).copied(),
            TargetDataLayout::LP64,
        );
        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        assert!(lowered.interner.iter().any(|(ty_id, ty)| {
            matches!(
                ty,
                TyKind::Array {
                    len: ArrayLenTy::Builtin { builtin, .. },
                    ..
                } if *builtin == LayoutBuiltin::Size
            ) && layouts.types.get(&ty_id) == Some(&TypeLayout { size: 8, align: 1 })
        }));
        assert!(lowered.interner.iter().any(|(ty_id, ty)| {
            matches!(
                ty,
                TyKind::Array {
                    len: ArrayLenTy::Builtin { builtin, .. },
                    ..
                } if *builtin == LayoutBuiltin::Align
            ) && layouts.types.get(&ty_id) == Some(&TypeLayout { size: 4, align: 1 })
        }));
    }

    #[test]
    fn computes_empty_struct_layout() {
        let (module, errors) = parse_module(
            r#"
struct Empty {}

fn main(value: Empty) {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let comptime = compute_test_comptime(&module, &defs, &signatures, &lowered);
        let layouts = compute_layouts_with_normalized_types(
            &defs,
            &lowered.interner,
            &signatures,
            &HashMap::new(),
            &|id| comptime.array_lengths.get(&id).copied(),
            TargetDataLayout::LP64,
        );
        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        let empty_id = defs.module_scope.types.get("Empty").expect("Empty def");
        let empty = layouts.structs.get(&empty_id).expect("Empty layout");
        assert_eq!(empty.layout, TypeLayout { size: 0, align: 1 });
        assert!(empty.fields.is_empty());
    }

    #[test]
    fn computes_nia_struct_layout_in_physical_field_order() {
        let (module, errors) = parse_module(
            r#"
struct Mixed {
    a: u8,
    b: i64,
    c: u8,
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let mixed_id = defs.module_scope.types.get("Mixed").expect("Mixed def");
        let signature = signatures.structs.get(&mixed_id).expect("Mixed signature");
        let a_id = signature.fields[0].def_id;
        let b_id = signature.fields[1].def_id;
        let c_id = signature.fields[2].def_id;
        let layouts = compute_layouts(
            &defs,
            &lowered.interner,
            &signatures,
            TargetDataLayout::LP64,
        );
        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        let mixed = layouts.structs.get(&mixed_id).expect("Mixed layout");
        assert_eq!(mixed.layout, TypeLayout { size: 16, align: 8 });
        assert_eq!(
            mixed
                .fields
                .iter()
                .map(|field| (field.def_id, field.offset))
                .collect::<Vec<_>>(),
            vec![(b_id, 0), (a_id, 8), (c_id, 9)]
        );
    }

    #[test]
    fn ignores_inferred_array_placeholders_during_global_layout_scan() {
        let (module, errors) = parse_module(
            r#"
fn main() {
    var xs: [_]u8 = [1, 2];
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        assert!(lowered.interner.iter().any(|(_, ty)| matches!(
            ty,
            TyKind::Array {
                len: ArrayLenTy::Infer,
                ..
            }
        )));
        let layouts = compute_layouts(
            &defs,
            &lowered.interner,
            &signatures,
            TargetDataLayout::LP64,
        );
        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
    }

    #[test]
    fn computes_extern_struct_c_field_layout() {
        let (module, errors) = parse_module(
            r#"
extern struct CPair {
    tag: u8,
    value: i32,
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let cpair_id = defs.module_scope.types.get("CPair").expect("CPair def");
        assert!(
            signatures
                .structs
                .get(&cpair_id)
                .expect("CPair signature")
                .is_extern
        );
        let layouts = compute_layouts(
            &defs,
            &lowered.interner,
            &signatures,
            TargetDataLayout::LP64,
        );
        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        let cpair = layouts.structs.get(&cpair_id).expect("CPair layout");
        assert_eq!(cpair.layout, TypeLayout { size: 8, align: 4 });
        assert_eq!(cpair.fields[0].offset, 0);
        assert_eq!(cpair.fields[1].offset, 4);
    }

    #[test]
    fn computes_separate_generic_struct_instance_layouts() {
        let (module, errors) = parse_module(
            r#"
struct ArrayBox[T] {
    values: [3]T,
}

fn main(a: ArrayBox[u8], b: ArrayBox[i32]) {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let comptime = compute_test_comptime(&module, &defs, &signatures, &lowered);
        let layouts = compute_layouts_with_normalized_types(
            &defs,
            &lowered.interner,
            &signatures,
            &HashMap::new(),
            &|id| comptime.array_lengths.get(&id).copied(),
            TargetDataLayout::LP64,
        );
        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        let array_box_id = defs
            .module_scope
            .types
            .get("ArrayBox")
            .expect("ArrayBox def");
        let u8_layout = layouts
            .struct_instances
            .get(&StructLayoutKey {
                def_id: array_box_id,
                args: vec![lowered.interner.primitive(PrimitiveTy::U8)],
            })
            .expect("ArrayBox[u8] layout");
        let i32_layout = layouts
            .struct_instances
            .get(&StructLayoutKey {
                def_id: array_box_id,
                args: vec![lowered.interner.primitive(PrimitiveTy::I32)],
            })
            .expect("ArrayBox[i32] layout");
        assert_eq!(u8_layout.layout, TypeLayout { size: 3, align: 1 });
        assert_eq!(i32_layout.layout, TypeLayout { size: 12, align: 4 });
    }

    #[test]
    fn computes_union_layouts() {
        let (module, errors) = parse_module(
            r#"
union Bits[T] {
    byte: u8,
    value: T,
}

fn main(a: Bits[i32]) {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let layouts = compute_layouts(
            &defs,
            &lowered.interner,
            &signatures,
            TargetDataLayout::LP64,
        );
        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        let bits_id = defs.module_scope.types.get("Bits").expect("Bits def");
        let bits_i32 = layouts
            .union_instances
            .get(&StructLayoutKey {
                def_id: bits_id,
                args: vec![lowered.interner.primitive(PrimitiveTy::I32)],
            })
            .expect("Bits[i32] layout");
        assert_eq!(bits_i32.layout, TypeLayout { size: 4, align: 4 });
        assert!(bits_i32.fields.iter().all(|field| field.offset == 0));
    }
}
