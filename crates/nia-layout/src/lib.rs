// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_comptime_check::ComptimeCheck;
use nia_defs::{DefCollection, DefId};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_item_signatures::{EnumSignature, ItemSignatures, StructSignature, UnionSignature};
use nia_span::Span;
use nia_ty::{ArrayLenTy, LayoutBuiltin, PrimitiveTy, TyInterner, TyKind};

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
}

pub fn compute_layouts(
    defs: &DefCollection,
    interner: &TyInterner,
    signatures: &ItemSignatures,
    target: TargetDataLayout,
) -> Layouts {
    let normalized = HashMap::new();
    let empty_comptime = ComptimeCheck::default();
    compute_layouts_with_normalized_types(
        defs,
        interner,
        signatures,
        &normalized,
        &empty_comptime,
        target,
    )
}

pub fn compute_layouts_with_normalized_types(
    defs: &DefCollection,
    interner: &TyInterner,
    signatures: &ItemSignatures,
    normalized: &HashMap<InternedTyId, InternedTyId>,
    comptime: &ComptimeCheck,
    target: TargetDataLayout,
) -> Layouts {
    compute_layouts_with_program_context(
        defs,
        interner,
        signatures,
        normalized,
        comptime,
        target,
        ProgramLayoutContext::default(),
    )
}

#[derive(Clone, Copy, Default)]
pub struct ProgramLayoutContext<'a> {
    pub layouts: Option<&'a dyn Fn(ModuleId) -> Option<Layouts>>,
    pub comptimes: Option<&'a dyn Fn(ModuleId) -> Option<ComptimeCheck>>,
}

pub fn compute_layouts_with_program_context(
    defs: &DefCollection,
    interner: &TyInterner,
    signatures: &ItemSignatures,
    normalized: &HashMap<InternedTyId, InternedTyId>,
    comptime: &ComptimeCheck,
    target: TargetDataLayout,
    program: ProgramLayoutContext<'_>,
) -> Layouts {
    LayoutComputer {
        module_id: defs.module_id,
        interner: interner.clone(),
        signatures,
        normalized,
        comptime,
        target,
        types: HashMap::new(),
        structs: HashMap::new(),
        unions: HashMap::new(),
        struct_instances: HashMap::new(),
        union_instances: HashMap::new(),
        diagnostics: Vec::new(),
        visiting: HashSet::new(),
        visiting_structs: HashSet::new(),
        visiting_unions: HashSet::new(),
        program,
    }
    .compute()
}

struct LayoutComputer<'a> {
    module_id: nia_ids::ModuleId,
    interner: TyInterner,
    signatures: &'a ItemSignatures,
    normalized: &'a HashMap<InternedTyId, InternedTyId>,
    comptime: &'a ComptimeCheck,
    target: TargetDataLayout,
    types: HashMap<InternedTyId, TypeLayout>,
    structs: HashMap<DefId, StructLayout>,
    unions: HashMap<DefId, StructLayout>,
    struct_instances: HashMap<StructLayoutKey, StructLayout>,
    union_instances: HashMap<StructLayoutKey, StructLayout>,
    diagnostics: Vec<Diagnostic>,
    visiting: HashSet<InternedTyId>,
    visiting_structs: HashSet<StructLayoutKey>,
    visiting_unions: HashSet<StructLayoutKey>,
    program: ProgramLayoutContext<'a>,
}

impl<'a> LayoutComputer<'a> {
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
        let ty_id = self.normalize_ty(ty_id);
        if let Some(layout) = self.types.get(&ty_id).cloned() {
            return Some(layout);
        }
        if !self.visiting.insert(ty_id) {
            self.diagnostics.push(Diagnostic::error(
                span,
                "recursive type layout is not supported",
            ));
            return None;
        }
        let layout = match self.interner.get(ty_id).cloned() {
            Some(TyKind::Primitive(primitive)) => self.primitive_layout(primitive),
            Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. }) => Some(TypeLayout {
                size: self.target.pointer_size,
                align: self.target.pointer_align,
            }),
            Some(TyKind::Slice { .. }) => Some(TypeLayout {
                size: self.target.pointer_size * 2,
                align: self.target.pointer_align,
            }),
            Some(TyKind::Array { len, elem }) => self.array_layout(span, len, elem),
            Some(TyKind::Nominal { def_id, args }) => self.nominal_layout(span, def_id, &args),
            Some(
                TyKind::Error
                | TyKind::GenericParam(_)
                | TyKind::BuiltinTrait { .. }
                | TyKind::Projection { .. }
                | TyKind::Range { .. },
            )
            | None => None,
        };
        self.visiting.remove(&ty_id);
        if let Some(layout) = &layout {
            self.types.insert(ty_id, layout.clone());
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

    fn array_layout(
        &mut self,
        span: Span,
        len: ArrayLenTy,
        elem: InternedTyId,
    ) -> Option<TypeLayout> {
        let elem_layout = self.layout_ty(elem, span)?;
        let len = match len {
            ArrayLenTy::Infer => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "array layout requires a concrete length",
                ));
                return None;
            }
            ArrayLenTy::ConstValue(value) => value,
            ArrayLenTy::ConstExpr(id) => {
                let value = if id.module_id == self.module_id {
                    self.comptime.array_lengths.get(&id).copied()
                } else {
                    self.program
                        .comptimes
                        .and_then(|comptimes| comptimes(id.module_id))
                        .and_then(|comptime| comptime.array_lengths.get(&id).copied())
                };
                let Some(value) = value else {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "array length was not evaluated by comptime",
                    ));
                    return None;
                };
                value
            }
            ArrayLenTy::Builtin { builtin, ty } => {
                let Some(layout) = self.layout_ty(ty, span) else {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "cannot compute layout for array length builtin `@{}`",
                            builtin.name()
                        ),
                    ));
                    return None;
                };
                match builtin {
                    LayoutBuiltin::Size => layout.size,
                    LayoutBuiltin::Align => layout.align,
                }
            }
        };
        Some(TypeLayout {
            size: elem_layout.size.saturating_mul(len),
            align: elem_layout.align,
        })
    }

    fn nominal_layout(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<TypeLayout> {
        if def_id.module_id != self.module_id {
            return self.external_nominal_layout(def_id, args);
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
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<TypeLayout> {
        let layouts = self
            .program
            .layouts
            .and_then(|query| query(def_id.module_id))?;
        layouts.nominal_type_layout(def_id, args)
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
            self.diagnostics.push(Diagnostic::error(
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
            self.diagnostics
                .push(Diagnostic::error(span, "union requires at least one field"));
            return None;
        }
        if !self.visiting_unions.insert(key.clone()) {
            self.diagnostics.push(Diagnostic::error(
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
        Some(TyKind::Pointer { is_const, elem }) => {
            let elem = substitute_generics(interner, elem, substitutions);
            interner.intern(TyKind::Pointer { is_const, elem })
        }
        Some(TyKind::Slice { is_const, elem }) => {
            let elem = substitute_generics(interner, elem, substitutions);
            interner.intern(TyKind::Slice { is_const, elem })
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

#[cfg(test)]
mod tests {
    use super::*;
    use nia_comptime_check::{
        ComptimeCheck, ComptimeInput, ComptimeProgramContext, check_module_comptime,
    };
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_item_signatures::collect_item_signatures;
    use nia_local_resolve::resolve_module_locals;
    use nia_parser::parse_module;
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
        check_module_comptime(ComptimeInput {
            module,
            defs,
            values: &values,
            locals: &locals,
            signatures,
            interner: &lowered.interner,
            const_exprs: &lowered.const_exprs,
            program: ComptimeProgramContext::empty(),
        })
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
            &comptime,
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
            &comptime,
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
            &comptime,
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
            &comptime,
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
