// SPDX-License-Identifier: GPL-3.0-or-later
//! Generic/const scopes, declaration validation, and type-definition lookup caches.

use super::*;

impl TypeLowerer<'_, '_> {
    pub(crate) fn lower_where_clause(&mut self, clause: &WhereClause) {
        for predicate in &clause.predicates {
            self.lower_type_in_context(&predicate.ty, TypeContext::Value);
            for bound in &predicate.bounds {
                self.lower_trait_bound(bound);
            }
        }
    }

    pub(crate) fn lower_trait_bound(&mut self, bound: &nia_ast::TypeRef) {
        self.lower_type_in_context(bound, TypeContext::TraitBound);
    }

    pub(crate) fn lower_array_len(&mut self, len: &ArrayLen) -> ArrayLenTy {
        match len {
            ArrayLen::Infer => ArrayLenTy::Infer,
            ArrayLen::Expr(expr) => self.lower_array_len_expr(expr),
        }
    }

    pub(crate) fn lower_array_len_expr(&mut self, expr: &Expr) -> ArrayLenTy {
        if let ExprKind::Ident(name) = &expr.kind
            && self.is_const_generic_param(name)
        {
            return ArrayLenTy::GenericParam(*name);
        }
        if let Some((builtin, type_arg)) = layout_builtin_array_len(expr) {
            ArrayLenTy::Builtin {
                builtin,
                ty: self.lower_type_in_context(type_arg, TypeContext::SizeQuery),
            }
        } else {
            self.register_const_array_len(expr)
        }
    }

    pub(crate) fn register_const_array_len(&mut self, expr: &Expr) -> ArrayLenTy {
        let id = self.register_const_expr_value(expr);
        ArrayLenTy::ConstExpr(id)
    }

    pub(crate) fn register_const_expr_value(&mut self, expr: &Expr) -> GlobalConstExprId {
        // Const expressions receive module-scoped monotonic identities. Visit first so nested
        // type uses and diagnostics are recorded before the expression becomes observable.
        self.visit_expr(expr);
        let id = GlobalConstExprId {
            module_id: self.module_id,
            const_expr_id: ConstExprId(self.next_const_expr_id),
        };
        self.next_const_expr_id += 1;
        self.const_exprs.insert(id, expr.clone());
        self.const_expr_summaries
            .insert(id, const_expr_summary(expr));
        id
    }

    pub(crate) fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.type_store.get(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
            ))
        )
    }

    pub(crate) fn can_be_integer(&self, ty: InternedTyId) -> bool {
        self.is_integer(ty)
            || matches!(
                self.type_store.get(self.normalize_if_known(ty)),
                Some(TyKind::GenericParam(_))
            )
    }

    pub(crate) fn types_equivalent(&self, left: InternedTyId, right: InternedTyId) -> bool {
        left == right || self.type_store.get(left) == self.type_store.get(right)
    }

    pub(crate) fn invalid_value_type_message(&mut self, ty: InternedTyId) -> Option<&'static str> {
        match self.type_store.get(ty).cloned() {
            Some(TyKind::Primitive(PrimitiveTy::Never)) => {
                Some("`never` is not valid as a value, field, parameter, or array element type")
            }
            Some(TyKind::SlicePointee { .. }) => Some(
                "slice pointee types are unsized and not valid as values, fields, parameters, or array elements; use `&[T]` or `&mut [T]` for a slice value",
            ),
            Some(TyKind::TraitObjectPointee { .. }) => Some(
                "trait object pointee types are unsized and not valid as values, fields, parameters, or array elements; use `&Trait[...]` or `&mut Trait[...]` for a trait object",
            ),
            Some(TyKind::CallablePointee { .. }) => Some(
                "callable interface types are unsized and not valid as values, fields, parameters, or array elements; use `&Fn(...)` or `&mut Fn(...)` for a callable view",
            ),
            Some(TyKind::BuiltinTrait { .. }) => Some(
                "trait types are not valid as values, fields, parameters, or array elements; use `&Trait[...]` or `&mut Trait[...]` for a trait object",
            ),
            Some(TyKind::Nominal { def_id, .. }) if self.is_trait_def(def_id) => Some(
                "trait types are not valid as values, fields, parameters, or array elements; use `&Trait[...]` or `&mut Trait[...]` for a trait object",
            ),
            _ => None,
        }
    }

    pub(crate) fn lower_callable_signature(
        &mut self,
        params: &[TypeRef],
        return_type: Option<&TypeRef>,
    ) -> (Vec<InternedTyId>, InternedTyId) {
        let params = params
            .iter()
            .map(|param| self.lower_type_in_context(param, TypeContext::Value))
            .collect();
        let return_type = match return_type {
            Some(return_type) => self.lower_type_in_context(return_type, TypeContext::Return),
            None => self.append.intern(TyKind::Tuple(Vec::new())),
        };
        (params, return_type)
    }

    pub(crate) fn defs_for_module(&mut self, module_id: ModuleId) -> Option<&DefCollection> {
        if !self.defs_cache.contains_key(&module_id) {
            self.defs_cache
                .insert(module_id, (self.program_defs.defs?)(module_id));
        }
        self.defs_cache
            .get(&module_id)
            .and_then(|defs| defs.as_deref())
    }
}
