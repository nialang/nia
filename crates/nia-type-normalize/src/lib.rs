// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{DefId, InternedTyId, ModuleId};
use nia_item_signatures::{ItemSignatures, TypeAliasSignature};
use nia_span::Span;
use nia_symbol::SymbolMap;
use nia_ty::{ArrayLenTy, TyKind, TypeStore, TypeStoreAppend};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeNormalization {
    pub normalized: HashMap<InternedTyId, InternedTyId>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct TypeNormalizationInput<'a> {
    pub module_id: ModuleId,
    pub type_store: &'a TypeStore,
    pub input_ids: &'a [InternedTyId],
    pub signatures: &'a ItemSignatures,
}

pub fn normalize_module_types(input: TypeNormalizationInput<'_>) -> TypeNormalization {
    let mut normalizer = TypeNormalizer {
        module_id: input.module_id,
        type_store: input.type_store,
        interner: input.type_store.append_for_module(input.module_id),
        aliases: &input.signatures.type_aliases,
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    for ty_id in input.input_ids.iter().copied() {
        normalizer.normalize_ty(ty_id, &mut Vec::new());
    }
    TypeNormalization {
        normalized: normalizer.normalized,
        diagnostics: normalizer.diagnostics,
    }
}

struct TypeNormalizer<'a, 'store> {
    module_id: ModuleId,
    type_store: &'store TypeStore,
    interner: TypeStoreAppend,
    aliases: &'a HashMap<DefId, TypeAliasSignature>,
    normalized: HashMap<InternedTyId, InternedTyId>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> TypeNormalizer<'a, '_> {
    fn normalize_ty(&mut self, ty_id: InternedTyId, stack: &mut Vec<DefId>) -> InternedTyId {
        if let Some(normalized) = self.normalized.get(&ty_id).copied() {
            return normalized;
        }
        let normalized = match self.type_store.get(ty_id).cloned() {
            Some(TyKind::Opaque | TyKind::BuiltinType(_) | TyKind::SelfParam) => ty_id,
            Some(TyKind::Tuple(elems)) => {
                let elems = elems
                    .into_iter()
                    .map(|elem| self.normalize_ty(elem, stack))
                    .collect();
                self.interner.intern(TyKind::Tuple(elems))
            }
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.normalize_ty(elem, stack);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem = self.normalize_ty(elem, stack);
                self.interner
                    .intern(TyKind::VolatilePointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.normalize_ty(elem, stack);
                self.interner.intern(TyKind::Slice { is_readonly, elem })
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem = self.normalize_ty(elem, stack);
                self.interner.intern(TyKind::SlicePointee { elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.normalize_ty(elem, stack);
                let len = self.normalize_array_len(len, stack);
                self.interner.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound.map(|bound| self.normalize_ty(bound, stack));
                self.interner.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.normalize_ty(param, stack))
                    .collect();
                let return_type = self.normalize_ty(return_type, stack);
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.normalize_ty(elem, stack);
                self.interner.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.normalize_ty(error, stack);
                let value = self.normalize_ty(value, stack);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_ty(arg, stack))
                    .collect::<Vec<_>>();
                let const_args = const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_ty(arg.ty, stack);
                        arg
                    })
                    .collect::<Vec<_>>();
                if def_id.module_id == self.module_id {
                    if let Some(alias) = self.aliases.get(&def_id.def_id).cloned() {
                        self.normalize_alias(def_id.def_id, &alias, &args, stack)
                    } else {
                        self.interner.intern(TyKind::Nominal {
                            def_id,
                            args,
                            const_args,
                        })
                    }
                } else {
                    self.interner.intern(TyKind::Nominal {
                        def_id,
                        args,
                        const_args,
                    })
                }
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_ty(arg, stack))
                    .collect();
                self.interner
                    .intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_ty(arg, stack))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_ty(arg.ty, stack);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.normalize_ty(arg, stack))
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.normalize_ty(arg.ty, stack);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.normalize_ty(binding.ty, stack),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_ty(arg, stack))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_ty(arg.ty, stack);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.normalize_ty(arg, stack))
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.normalize_ty(arg.ty, stack);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.normalize_ty(binding.ty, stack),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }) => {
                let self_ty = self.normalize_ty(self_ty, stack);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_ty(arg, stack))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_ty(arg.ty, stack);
                        arg
                    })
                    .collect();
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    name,
                })
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::GenericParam(_),
            )
            | None => ty_id,
        };
        self.normalized.insert(ty_id, normalized);
        normalized
    }

    fn normalize_alias(
        &mut self,
        alias_id: DefId,
        alias: &TypeAliasSignature,
        args: &[InternedTyId],
        stack: &mut Vec<DefId>,
    ) -> InternedTyId {
        if stack.contains(&alias_id) {
            self.report_recursive_alias(alias.span, stack, alias_id);
            return self.interner.intern(TyKind::Error);
        }
        if alias.generics.len() != args.len() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                alias.span,
                format!(
                    "type alias argument count mismatch: expected {}, got {}",
                    alias.generics.len(),
                    args.len()
                ),
            ));
            return self.interner.intern(TyKind::Error);
        }
        let substitutions: SymbolMap<InternedTyId> = alias
            .generics
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect();
        stack.push(alias_id);
        let normalized = self.normalize_ty_with_substitutions(alias.target, &substitutions, stack);
        stack.pop();
        normalized
    }

    fn normalize_ty_with_substitutions(
        &mut self,
        ty_id: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
        stack: &mut Vec<DefId>,
    ) -> InternedTyId {
        match self.type_store.get(ty_id).cloned() {
            Some(TyKind::Opaque | TyKind::BuiltinType(_) | TyKind::SelfParam) => ty_id,
            Some(TyKind::Tuple(elems)) => {
                let elems = elems
                    .into_iter()
                    .map(|elem| self.normalize_ty_with_substitutions(elem, substitutions, stack))
                    .collect();
                self.interner.intern(TyKind::Tuple(elems))
            }
            Some(TyKind::GenericParam(name)) => substitutions
                .get(&name)
                .copied()
                .unwrap_or_else(|| self.normalize_ty(ty_id, stack)),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.normalize_ty_with_substitutions(elem, substitutions, stack);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem = self.normalize_ty_with_substitutions(elem, substitutions, stack);
                self.interner
                    .intern(TyKind::VolatilePointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.normalize_ty_with_substitutions(elem, substitutions, stack);
                self.interner.intern(TyKind::Slice { is_readonly, elem })
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem = self.normalize_ty_with_substitutions(elem, substitutions, stack);
                self.interner.intern(TyKind::SlicePointee { elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.normalize_ty_with_substitutions(elem, substitutions, stack);
                let len = self.normalize_array_len_with_substitutions(len, substitutions, stack);
                self.interner.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound
                    .map(|bound| self.normalize_ty_with_substitutions(bound, substitutions, stack));
                self.interner.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.normalize_ty_with_substitutions(param, substitutions, stack))
                    .collect();
                let return_type =
                    self.normalize_ty_with_substitutions(return_type, substitutions, stack);
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.normalize_ty_with_substitutions(elem, substitutions, stack);
                self.interner.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.normalize_ty_with_substitutions(error, substitutions, stack);
                let value = self.normalize_ty_with_substitutions(value, substitutions, stack);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_ty_with_substitutions(arg, substitutions, stack))
                    .collect::<Vec<_>>();
                let const_args = const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_ty_with_substitutions(arg.ty, substitutions, stack);
                        arg
                    })
                    .collect::<Vec<_>>();
                if def_id.module_id == self.module_id {
                    if let Some(alias) = self.aliases.get(&def_id.def_id).cloned() {
                        self.normalize_alias(def_id.def_id, &alias, &args, stack)
                    } else {
                        self.interner.intern(TyKind::Nominal {
                            def_id,
                            args,
                            const_args,
                        })
                    }
                } else {
                    self.interner.intern(TyKind::Nominal {
                        def_id,
                        args,
                        const_args,
                    })
                }
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_ty_with_substitutions(arg, substitutions, stack))
                    .collect();
                self.interner
                    .intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_ty_with_substitutions(arg, substitutions, stack))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_ty_with_substitutions(arg.ty, substitutions, stack);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| {
                                self.normalize_ty_with_substitutions(arg, substitutions, stack)
                            })
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.normalize_ty_with_substitutions(
                                    arg.ty,
                                    substitutions,
                                    stack,
                                );
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.normalize_ty_with_substitutions(binding.ty, substitutions, stack),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_ty_with_substitutions(arg, substitutions, stack))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_ty_with_substitutions(arg.ty, substitutions, stack);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| {
                                self.normalize_ty_with_substitutions(arg, substitutions, stack)
                            })
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.normalize_ty_with_substitutions(
                                    arg.ty,
                                    substitutions,
                                    stack,
                                );
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.normalize_ty_with_substitutions(binding.ty, substitutions, stack),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }) => {
                let self_ty = self.normalize_ty_with_substitutions(self_ty, substitutions, stack);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_ty_with_substitutions(arg, substitutions, stack))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.normalize_ty_with_substitutions(arg.ty, substitutions, stack);
                        arg
                    })
                    .collect();
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    name,
                })
            }
            Some(
                TyKind::Error | TyKind::ConstOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
            )
            | None => self.normalize_ty(ty_id, stack),
        }
    }

    fn report_recursive_alias(&mut self, span: Span, stack: &[DefId], alias_id: DefId) {
        let mut seen = HashSet::new();
        let mut cycle = Vec::new();
        for def_id in stack.iter().copied().chain([alias_id]) {
            if seen.insert(def_id) {
                cycle.push(format!("#{}", def_id.0));
            }
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_NORMALIZATION,
            span,
            format!("recursive type alias detected: {}", cycle.join(" -> ")),
        ));
    }

    fn normalize_array_len(&mut self, len: ArrayLenTy, stack: &mut Vec<DefId>) -> ArrayLenTy {
        match len {
            ArrayLenTy::Builtin { builtin, ty } => ArrayLenTy::Builtin {
                builtin,
                // Layout-builtin array lengths are semantically type-level expressions; keep the
                // operand normalized so alias-expanded types compare the same inside and outside
                // array length positions.
                ty: self.normalize_ty(ty, stack),
            },
            ArrayLenTy::Infer
            | ArrayLenTy::GenericParam(_)
            | ArrayLenTy::ConstValue(_)
            | ArrayLenTy::ConstExpr(_) => len,
        }
    }

    fn normalize_array_len_with_substitutions(
        &mut self,
        len: ArrayLenTy,
        substitutions: &SymbolMap<InternedTyId>,
        stack: &mut Vec<DefId>,
    ) -> ArrayLenTy {
        match len {
            ArrayLenTy::Builtin { builtin, ty } => ArrayLenTy::Builtin {
                builtin,
                ty: self.normalize_ty_with_substitutions(ty, substitutions, stack),
            },
            ArrayLenTy::Infer
            | ArrayLenTy::GenericParam(_)
            | ArrayLenTy::ConstValue(_)
            | ArrayLenTy::ConstExpr(_) => len,
        }
    }
}

impl TypeNormalization {
    pub fn normalize(&self, ty_id: InternedTyId) -> InternedTyId {
        self.normalized.get(&ty_id).copied().unwrap_or(ty_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_ids::ModuleIdAllocator;
    use nia_item_signatures::{ItemSignatureInput, ItemSignatureSource, collect_item_signatures};
    use nia_parser::parse_module;
    use nia_ty::{ArrayLenTy, LayoutBuiltin, PrimitiveTy, TyKind, TypeStore};
    use nia_type_lower::{TypeLowering, TypeLoweringContext, lower_module_types_with_context};
    use nia_type_resolve::resolve_module_types;

    fn normalize_lowered(
        module_id: ModuleId,
        type_store: &TypeStore,
        lowered: &TypeLowering,
        signatures: &ItemSignatures,
    ) -> TypeNormalization {
        let input_ids = lowered.explicit_type_roots();
        normalize_module_types(TypeNormalizationInput {
            module_id,
            type_store,
            input_ids: &input_ids,
            signatures,
        })
    }

    fn lowered_types<'a>(
        lowered: &'a TypeLowering,
        type_store: &'a TypeStore,
    ) -> impl Iterator<Item = (InternedTyId, &'a TyKind)> {
        lowered
            .explicit_type_roots()
            .into_iter()
            .filter_map(|ty| type_store.get(ty).map(|kind| (ty, kind)))
    }

    fn collect_test_signatures(
        source: ItemSignatureSource<'_>,
        defs: &nia_defs::DefCollection,
        lowered: &TypeLowering,
        type_store: &TypeStore,
    ) -> ItemSignatures {
        collect_item_signatures(ItemSignatureInput {
            source,
            defs,
            lowered,
            type_store,
            symbols: None,
        })
    }

    #[test]
    fn expands_simple_type_aliases() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let (module, errors) = parse_module(
            r#"
type Byte = u8;
fn id(x: Byte) u8 { x }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let lowered = lower_module_types_with_context(
            module_id,
            &module,
            &resolved,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_test_signatures(
            ItemSignatureSource::Module(&module),
            &defs,
            &lowered,
            &type_store,
        );
        let normalization = normalize_lowered(module_id, &type_store, &lowered, &signatures);
        assert!(
            normalization.diagnostics.is_empty(),
            "{:?}",
            normalization.diagnostics
        );
        assert!(lowered_types(&lowered, &type_store).any(|(ty_id, ty)| {
            matches!(ty, TyKind::Nominal { .. })
                && matches!(
                    type_store.get(normalization.normalize(ty_id)),
                    Some(TyKind::Primitive(PrimitiveTy::U8))
                )
        }));
    }

    #[test]
    fn expands_generic_type_aliases() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let (module, errors) = parse_module(
            r#"
type RawPtr[T] = &T;
fn id(p: RawPtr[u8]) &u8 { p }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let lowered = lower_module_types_with_context(
            module_id,
            &module,
            &resolved,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_test_signatures(
            ItemSignatureSource::Module(&module),
            &defs,
            &lowered,
            &type_store,
        );
        let normalization = normalize_lowered(module_id, &type_store, &lowered, &signatures);
        assert!(
            normalization.diagnostics.is_empty(),
            "{:?}",
            normalization.diagnostics
        );
        assert!(lowered_types(&lowered, &type_store).any(|(ty_id, ty)| {
            matches!(ty, TyKind::Nominal { .. })
                && matches!(
                    type_store.get(normalization.normalize(ty_id)),
                    Some(TyKind::Pointer { elem, .. })
                        if matches!(
                            type_store.get(*elem),
                            Some(TyKind::Primitive(PrimitiveTy::U8))
                        )
                )
        }));
    }

    #[test]
    fn recursively_substitutes_and_normalizes_tuple_alias_elements() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let (module, errors) = parse_module(
            r#"
type Nested[T] = (T, ((), T));
fn id(value: Nested[u16]) (u16, ((), u16)) { value }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let lowered = lower_module_types_with_context(
            module_id,
            &module,
            &resolved,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_test_signatures(
            ItemSignatureSource::Module(&module),
            &defs,
            &lowered,
            &type_store,
        );
        let normalization = normalize_lowered(module_id, &type_store, &lowered, &signatures);
        assert!(
            normalization.diagnostics.is_empty(),
            "{:?}",
            normalization.diagnostics
        );

        assert!(lowered_types(&lowered, &type_store).any(|(ty_id, ty)| {
            let TyKind::Nominal { .. } = ty else {
                return false;
            };
            let Some(TyKind::Tuple(outer)) = type_store.get(normalization.normalize(ty_id)) else {
                return false;
            };
            matches!(
                outer.as_slice(),
                [first, nested]
                    if type_store.get(*first) == Some(&TyKind::Primitive(PrimitiveTy::U16))
                        && matches!(
                            type_store.get(*nested),
                            Some(TyKind::Tuple(inner))
                                if matches!(
                                    inner.as_slice(),
                                    [unit, second]
                                        if type_store.get(*unit).is_some_and(TyKind::is_unit)
                                            && type_store.get(*second)
                                                == Some(&TyKind::Primitive(PrimitiveTy::U16))
                                )
                        )
            )
        }));
    }

    #[test]
    fn normalizes_layout_builtin_array_length_operand() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let (module, errors) = parse_module(
            r#"
type Byte = u8;
fn id(x: [std::builtin::size[Byte]()]u8) [std::builtin::size[u8]()]u8 { x }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let lowered = lower_module_types_with_context(
            module_id,
            &module,
            &resolved,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_test_signatures(
            ItemSignatureSource::Module(&module),
            &defs,
            &lowered,
            &type_store,
        );
        let normalization = normalize_lowered(module_id, &type_store, &lowered, &signatures);
        assert!(
            normalization.diagnostics.is_empty(),
            "{:?}",
            normalization.diagnostics
        );
        assert!(lowered_types(&lowered, &type_store).any(|(ty_id, ty)| {
            matches!(
                ty,
                TyKind::Array {
                    len: ArrayLenTy::Builtin {
                        builtin: LayoutBuiltin::Size,
                        ..
                    },
                    ..
                }
            ) && matches!(
                type_store.get(normalization.normalize(ty_id)),
                Some(TyKind::Array {
                    len: ArrayLenTy::Builtin {
                        builtin: LayoutBuiltin::Size,
                        ty,
                    },
                    ..
                }) if type_store.get(*ty)
                    == Some(&TyKind::Primitive(PrimitiveTy::U8))
            )
        }));
    }

    #[test]
    fn substitutes_layout_builtin_array_length_operand_in_generic_alias() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let (module, errors) = parse_module(
            r#"
type SizedBytes[T] = [std::builtin::size[T]()]u8;
fn id(x: SizedBytes[u16]) [std::builtin::size[u16]()]u8 { x }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let lowered = lower_module_types_with_context(
            module_id,
            &module,
            &resolved,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_test_signatures(
            ItemSignatureSource::Module(&module),
            &defs,
            &lowered,
            &type_store,
        );
        let normalization = normalize_lowered(module_id, &type_store, &lowered, &signatures);
        assert!(
            normalization.diagnostics.is_empty(),
            "{:?}",
            normalization.diagnostics
        );
        assert!(lowered_types(&lowered, &type_store).any(|(ty_id, ty)| {
            matches!(ty, TyKind::Nominal { .. })
                && matches!(
                    type_store.get(normalization.normalize(ty_id)),
                    Some(TyKind::Array {
                        len: ArrayLenTy::Builtin {
                            builtin: LayoutBuiltin::Size,
                            ty,
                        },
                        ..
                    }) if type_store.get(*ty)
                        == Some(&TyKind::Primitive(PrimitiveTy::U16))
                )
        }));
    }

    #[test]
    fn reports_recursive_type_aliases() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let (module, errors) = parse_module(
            r#"
type A = B;
type B = A;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let lowered = lower_module_types_with_context(
            module_id,
            &module,
            &resolved,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_test_signatures(
            ItemSignatureSource::Module(&module),
            &defs,
            &lowered,
            &type_store,
        );
        let normalization = normalize_lowered(module_id, &type_store, &lowered, &signatures);
        assert!(
            normalization
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("recursive type alias"))
        );
    }

    #[test]
    fn preserves_array_length_const_expr_identity() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let (module, errors) = parse_module(
            r#"
fn take(xs: [2 + 3]u8) () {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let lowered = lower_module_types_with_context(
            module_id,
            &module,
            &resolved,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_test_signatures(
            ItemSignatureSource::Module(&module),
            &defs,
            &lowered,
            &type_store,
        );
        let normalization = normalize_lowered(module_id, &type_store, &lowered, &signatures);
        assert!(normalization.normalized.values().any(|ty| matches!(
            type_store.get(*ty),
            Some(TyKind::Array {
                len: ArrayLenTy::ConstExpr(_),
                elem: _,
            })
        )));
    }

    #[test]
    fn normalizes_only_the_explicit_input_set() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let type_store = TypeStore::new();
        let append = type_store.append_for_module(module_id);
        let elem = append.intern(TyKind::Primitive(PrimitiveTy::U8));
        let pointer = append.intern(TyKind::Pointer {
            is_readonly: true,
            elem,
        });
        let slice = append.intern(TyKind::Slice {
            is_readonly: true,
            elem,
        });

        let signatures = ItemSignatures {
            functions: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            traits: HashMap::new(),
            trait_impls: Vec::new(),
            enums: HashMap::new(),
            type_aliases: HashMap::new(),
            globals: HashMap::new(),
            consts: HashMap::new(),
            diagnostics: Vec::new(),
        };
        let normalization = normalize_module_types(TypeNormalizationInput {
            module_id,
            type_store: &type_store,
            input_ids: &[pointer],
            signatures: &signatures,
        });

        assert!(normalization.normalized.contains_key(&pointer));
        assert!(!normalization.normalized.contains_key(&slice));
        assert!(type_store.get(pointer).is_some());
        assert!(type_store.get(slice).is_some());
    }
}
