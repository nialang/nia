// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_diagnostic::Diagnostic;
use nia_ids::{DefId, InternedTyId, ModuleId};
use nia_item_signatures::{ItemSignatures, TypeAliasSignature};
use nia_span::Span;
use nia_ty::{ArrayLenTy, TyInterner, TyKind};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeNormalization {
    pub interner: TyInterner,
    pub normalized: HashMap<InternedTyId, InternedTyId>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn normalize_module_types(
    module_id: ModuleId,
    interner: &TyInterner,
    signatures: &ItemSignatures,
) -> TypeNormalization {
    let mut normalizer = TypeNormalizer {
        module_id,
        interner: interner.clone(),
        aliases: &signatures.type_aliases,
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
    };
    let ty_ids: Vec<InternedTyId> = normalizer.interner.iter().map(|(ty_id, _)| ty_id).collect();
    for ty_id in ty_ids {
        normalizer.normalize_ty(ty_id, &mut Vec::new());
    }
    TypeNormalization {
        interner: normalizer.interner,
        normalized: normalizer.normalized,
        diagnostics: normalizer.diagnostics,
    }
}

struct TypeNormalizer<'a> {
    module_id: ModuleId,
    interner: TyInterner,
    aliases: &'a HashMap<DefId, TypeAliasSignature>,
    normalized: HashMap<InternedTyId, InternedTyId>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> TypeNormalizer<'a> {
    fn normalize_ty(&mut self, ty_id: InternedTyId, stack: &mut Vec<DefId>) -> InternedTyId {
        if let Some(normalized) = self.normalized.get(&ty_id).copied() {
            return normalized;
        }
        let normalized = match self.interner.get(ty_id).cloned() {
            Some(TyKind::Pointer { is_const, elem }) => {
                let elem = self.normalize_ty(elem, stack);
                self.interner.intern(TyKind::Pointer { is_const, elem })
            }
            Some(TyKind::Slice { is_const, elem }) => {
                let elem = self.normalize_ty(elem, stack);
                self.interner.intern(TyKind::Slice { is_const, elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.normalize_ty(elem, stack);
                let len = normalize_array_len(len);
                self.interner.intern(TyKind::Array { len, elem })
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
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_ty(arg, stack))
                    .collect::<Vec<_>>();
                if def_id.module_id == self.module_id {
                    if let Some(alias) = self.aliases.get(&def_id.def_id).cloned() {
                        self.normalize_alias(def_id.def_id, &alias, &args, stack)
                    } else {
                        self.interner.intern(TyKind::Nominal { def_id, args })
                    }
                } else {
                    self.interner.intern(TyKind::Nominal { def_id, args })
                }
            }
            Some(TyKind::Error | TyKind::Primitive(_) | TyKind::GenericParam(_)) | None => ty_id,
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
            return self.interner.error();
        }
        if alias.generics.len() != args.len() {
            self.diagnostics.push(Diagnostic::error(
                alias.span,
                format!(
                    "type alias argument count mismatch: expected {}, got {}",
                    alias.generics.len(),
                    args.len()
                ),
            ));
            return self.interner.error();
        }
        let substitutions: HashMap<String, InternedTyId> = alias
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
        substitutions: &HashMap<String, InternedTyId>,
        stack: &mut Vec<DefId>,
    ) -> InternedTyId {
        match self.interner.get(ty_id).cloned() {
            Some(TyKind::GenericParam(name)) => substitutions
                .get(&name)
                .copied()
                .unwrap_or_else(|| self.normalize_ty(ty_id, stack)),
            Some(TyKind::Pointer { is_const, elem }) => {
                let elem = self.normalize_ty_with_substitutions(elem, substitutions, stack);
                self.interner.intern(TyKind::Pointer { is_const, elem })
            }
            Some(TyKind::Slice { is_const, elem }) => {
                let elem = self.normalize_ty_with_substitutions(elem, substitutions, stack);
                self.interner.intern(TyKind::Slice { is_const, elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.normalize_ty_with_substitutions(elem, substitutions, stack);
                let len = normalize_array_len(len);
                self.interner.intern(TyKind::Array { len, elem })
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
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.normalize_ty_with_substitutions(arg, substitutions, stack))
                    .collect::<Vec<_>>();
                if def_id.module_id == self.module_id {
                    if let Some(alias) = self.aliases.get(&def_id.def_id).cloned() {
                        self.normalize_alias(def_id.def_id, &alias, &args, stack)
                    } else {
                        self.interner.intern(TyKind::Nominal { def_id, args })
                    }
                } else {
                    self.interner.intern(TyKind::Nominal { def_id, args })
                }
            }
            Some(TyKind::Error | TyKind::Primitive(_)) | None => self.normalize_ty(ty_id, stack),
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
        self.diagnostics.push(Diagnostic::error(
            span,
            format!("recursive type alias detected: {}", cycle.join(" -> ")),
        ));
    }
}

fn normalize_array_len(len: ArrayLenTy) -> ArrayLenTy {
    match len {
        ArrayLenTy::Infer
        | ArrayLenTy::ConstValue(_)
        | ArrayLenTy::ConstExpr(_)
        | ArrayLenTy::Builtin { .. } => len,
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
    use nia_item_signatures::collect_item_signatures;
    use nia_parser::parse_module;
    use nia_ty::{PrimitiveTy, TyKind};
    use nia_type_lower::lower_module_types_with_id;
    use nia_type_resolve::resolve_module_types;

    #[test]
    fn expands_simple_type_aliases() {
        let (module, errors) = parse_module(
            r#"
type Byte = u8;
fn id(x: Byte) u8 { x }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let normalization = normalize_module_types(ModuleId(0), &lowered.interner, &signatures);
        assert!(
            normalization.diagnostics.is_empty(),
            "{:?}",
            normalization.diagnostics
        );
        assert!(lowered.interner.iter().any(|(ty_id, ty)| {
            matches!(ty, TyKind::Nominal { .. })
                && matches!(
                    normalization.interner.get(normalization.normalize(ty_id)),
                    Some(TyKind::Primitive(PrimitiveTy::U8))
                )
        }));
    }

    #[test]
    fn expands_generic_type_aliases() {
        let (module, errors) = parse_module(
            r#"
type Ptr[T] = &T;
fn id(p: Ptr[u8]) &u8 { p }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let normalization = normalize_module_types(ModuleId(0), &lowered.interner, &signatures);
        assert!(
            normalization.diagnostics.is_empty(),
            "{:?}",
            normalization.diagnostics
        );
        assert!(lowered.interner.iter().any(|(ty_id, ty)| {
            matches!(ty, TyKind::Nominal { .. })
                && matches!(
                    normalization.interner.get(normalization.normalize(ty_id)),
                    Some(TyKind::Pointer { elem, .. })
                        if matches!(
                            normalization.interner.get(*elem),
                            Some(TyKind::Primitive(PrimitiveTy::U8))
                        )
                )
        }));
    }

    #[test]
    fn reports_recursive_type_aliases() {
        let (module, errors) = parse_module(
            r#"
type A = B;
type B = A;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let normalization = normalize_module_types(ModuleId(0), &lowered.interner, &signatures);
        assert!(
            normalization
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("recursive type alias"))
        );
    }

    #[test]
    fn preserves_array_length_const_expr_identity() {
        let (module, errors) = parse_module(
            r#"
fn take(xs: [2 + 3]u8) void {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let normalization = normalize_module_types(ModuleId(0), &lowered.interner, &signatures);
        assert!(normalization.interner.iter().any(|(_, ty)| matches!(
            ty,
            TyKind::Array {
                len: ArrayLenTy::ConstExpr(_),
                elem: _,
            }
        )));
    }
}
