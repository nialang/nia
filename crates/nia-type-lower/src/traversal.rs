// SPDX-License-Identifier: GPL-3.0-or-later
//! Full AST traversal that discovers type and const-expression lowering sites.

use super::*;

impl<'ast> Visitor<'ast> for TypeLowerer<'_, '_> {
    fn visit_item(&mut self, item: &'ast Item) {
        match &item.kind {
            ItemKind::Struct(item_struct) => {
                self.with_generics(&item_struct.generics, |lowerer| {
                    lowerer.lower_where_clause(&item_struct.where_clause);
                    for field in &item_struct.fields {
                        lowerer.lower_type_in_context(&field.ty, TypeContext::Value);
                    }
                });
            }
            ItemKind::Union(item_union) => {
                self.with_generics(&item_union.generics, |lowerer| {
                    lowerer.lower_where_clause(&item_union.where_clause);
                    for field in &item_union.fields {
                        lowerer.lower_type_in_context(&field.ty, TypeContext::Value);
                    }
                });
            }
            ItemKind::Trait(item_trait) => {
                self.with_generics(&item_trait.generics, |lowerer| {
                    let self_ty = lowerer.append.intern(TyKind::SelfParam);
                    lowerer.with_self_type(self_ty, |lowerer| {
                        if let Some(trait_id) = lowerer.local_trait_id(&item.node_key) {
                            let trait_args = item_trait
                                .generics
                                .iter()
                                .filter(|generic| matches!(generic.kind, GenericParamKind::Type))
                                .map(|generic| {
                                    lowerer.append.intern(TyKind::GenericParam(generic.name))
                                })
                                .collect::<Vec<_>>();
                            let trait_const_args = item_trait
                                .generics
                                .iter()
                                .filter_map(|generic| match generic.kind {
                                    GenericParamKind::Type => None,
                                    GenericParamKind::Const { ref ty } => {
                                        let ty =
                                            lowerer.lower_type_in_context(ty, TypeContext::Value);
                                        Some(ConstGenericArg {
                                            ty,
                                            value: ConstGenericValue::GenericParam(generic.name),
                                        })
                                    }
                                })
                                .collect::<Vec<_>>();
                            let associated_types = item_trait
                                .associated_types
                                .iter()
                                .map(|associated_type| associated_type.name)
                                .collect::<Vec<_>>();
                            lowerer.with_associated_type_scope(
                                AssociatedTypeScope {
                                    self_ty,
                                    trait_id: TraitId::Source(trait_id),
                                    trait_args,
                                    trait_const_args,
                                    names: associated_types,
                                },
                                |lowerer| {
                                    for supertrait in &item_trait.supertraits {
                                        lowerer.lower_type_in_context(
                                            supertrait,
                                            TypeContext::TraitBound,
                                        );
                                    }
                                    lowerer.lower_where_clause(&item_trait.where_clause);
                                    for associated_value in &item_trait.associated_values {
                                        lowerer.lower_type_in_context(
                                            &associated_value.ty,
                                            TypeContext::Value,
                                        );
                                    }
                                    for method in &item_trait.methods {
                                        lowerer.visit_function(&method.function);
                                    }
                                },
                            );
                        } else {
                            for supertrait in &item_trait.supertraits {
                                lowerer.lower_type_in_context(supertrait, TypeContext::TraitBound);
                            }
                            lowerer.lower_where_clause(&item_trait.where_clause);
                            for associated_value in &item_trait.associated_values {
                                lowerer.lower_type_in_context(
                                    &associated_value.ty,
                                    TypeContext::Value,
                                );
                            }
                            for method in &item_trait.methods {
                                lowerer.visit_function(&method.function);
                            }
                        }
                    });
                });
            }
            ItemKind::Extend(extend) => {
                self.with_generics(&extend.generics, |lowerer| {
                    let self_ty =
                        lowerer.lower_type_in_context(&extend.target, TypeContext::ExtendTarget);
                    let trait_scope = extend.trait_ref.as_ref().and_then(|trait_ref| {
                        let trait_ty =
                            lowerer.lower_type_in_context(trait_ref, TypeContext::TraitBound);
                        lowerer.associated_type_scope_for_trait_impl(self_ty, trait_ty)
                    });
                    lowerer.with_self_type(self_ty, |lowerer| {
                        lowerer.lower_where_clause(&extend.where_clause);
                        for associated_type in &extend.associated_types {
                            lowerer.lower_type_in_context(&associated_type.ty, TypeContext::Value);
                        }
                        for associated_value in &extend.associated_values {
                            if let Some(ty) = &associated_value.binding.ty {
                                lowerer.lower_type_in_context(ty, TypeContext::Value);
                            }
                            if lowerer.mode == TypeLowerMode::All
                                && let Some(value) = &associated_value.binding.value
                            {
                                lowerer.visit_expr(value);
                            }
                        }
                        if let Some(trait_scope) = trait_scope {
                            lowerer.with_associated_type_scope(trait_scope, |lowerer| {
                                for method in &extend.methods {
                                    lowerer.visit_function(&method.function);
                                }
                            });
                        } else {
                            for method in &extend.methods {
                                lowerer.visit_function(&method.function);
                            }
                        }
                    });
                });
            }
            ItemKind::Enum(item_enum) => {
                if let Some(backing_type) = &item_enum.backing_type {
                    let ty = self.lower_type_in_context(backing_type, TypeContext::Value);
                    if !self.is_integer(ty) {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            backing_type.span,
                            "enum backing type must be an integer type",
                        ));
                    }
                }
                for variant in &item_enum.variants {
                    match &variant.payload {
                        nia_ast::EnumVariantPayload::Unit => {}
                        nia_ast::EnumVariantPayload::Tuple(fields) => {
                            for field in fields {
                                self.lower_type_in_context(field, TypeContext::Value);
                            }
                        }
                        nia_ast::EnumVariantPayload::Named(fields) => {
                            for field in fields {
                                self.lower_type_in_context(&field.ty, TypeContext::Value);
                            }
                        }
                    }
                }
            }
            ItemKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |lowerer| {
                    lowerer.lower_where_clause(&alias.where_clause);
                    if let Some(ty) = &alias.ty {
                        lowerer.lower_type_in_context(ty, TypeContext::Alias);
                    }
                });
            }
            ItemKind::Binding(binding) => {
                if let Some(ty) = &binding.ty {
                    self.lower_type_in_context(ty, TypeContext::Value);
                }
                if self.mode == TypeLowerMode::All
                    && let Some(value) = &binding.value
                {
                    nia_ast_walk::walk_expr(self, value);
                }
            }
            ItemKind::Function(function) => self.visit_function(function),
            ItemKind::Module(_) | ItemKind::Using(_) => {}
        }
    }

    fn visit_function(&mut self, function: &'ast FunctionItem) {
        self.with_generics(&function.generics, |lowerer| {
            lowerer.lower_where_clause(&function.where_clause);
            for param in &function.params {
                if let Some(ty) = &param.ty {
                    lowerer.lower_type_in_context(ty, TypeContext::Value);
                }
            }
            if let Some(return_type) = &function.return_type {
                lowerer.lower_type_in_context(return_type, TypeContext::Return);
            }
            if lowerer.mode == TypeLowerMode::All
                && let Some(body) = &function.body
            {
                lowerer.visit_block(body);
            }
        });
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        self.lower_type_in_context(ty, TypeContext::Value);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        match &expr.kind {
            ExprKind::BracketSuffix { callee, args } => {
                self.visit_expr(callee);
                for arg in args {
                    if let Some(ty) = &arg.ty {
                        if !matches!(ty.kind, TypeKind::Infer) {
                            self.lower_type_in_context(ty, TypeContext::Value);
                        }
                    } else if let Some(expr) = &arg.expr {
                        self.visit_expr(expr);
                    }
                }
            }
            ExprKind::TypeTarget { ty } => {
                self.visit_type(ty);
            }
            ExprKind::TraitTarget { ty, trait_ref } => {
                self.lower_type_in_context(ty, TypeContext::Value);
                self.lower_type_in_context(trait_ref, TypeContext::TraitBound);
            }
            ExprKind::TypedStructLiteral { ty, fields } => {
                self.visit_type(ty);
                for field in fields {
                    self.visit_expr(&field.value);
                }
            }
            _ => nia_ast_walk::walk_expr(self, expr),
        }
    }
}

impl TypeLowerer<'_, '_> {
    pub(crate) fn visit_item_tree_node(&mut self, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Struct(item_struct) => {
                self.with_generics(&item_struct.generics, |lowerer| {
                    lowerer.lower_where_clause(&item_struct.where_clause);
                    for field in &item_struct.fields {
                        lowerer.lower_type_in_context(&field.ty, TypeContext::Value);
                    }
                });
            }
            ItemTreeNodeKind::Union(item_union) => {
                self.with_generics(&item_union.generics, |lowerer| {
                    lowerer.lower_where_clause(&item_union.where_clause);
                    for field in &item_union.fields {
                        lowerer.lower_type_in_context(&field.ty, TypeContext::Value);
                    }
                });
            }
            ItemTreeNodeKind::Trait(item_trait) => {
                self.with_generics(&item_trait.generics, |lowerer| {
                    let self_ty = lowerer.append.intern(TyKind::SelfParam);
                    lowerer.with_self_type(self_ty, |lowerer| {
                        if let Some(trait_id) = lowerer.local_trait_id(&item.node_key) {
                            let trait_args = item_trait
                                .generics
                                .iter()
                                .filter(|generic| matches!(generic.kind, GenericParamKind::Type))
                                .map(|generic| {
                                    lowerer.append.intern(TyKind::GenericParam(generic.name))
                                })
                                .collect::<Vec<_>>();
                            let trait_const_args = item_trait
                                .generics
                                .iter()
                                .filter_map(|generic| match generic.kind {
                                    GenericParamKind::Type => None,
                                    GenericParamKind::Const { ref ty } => {
                                        let ty =
                                            lowerer.lower_type_in_context(ty, TypeContext::Value);
                                        Some(ConstGenericArg {
                                            ty,
                                            value: ConstGenericValue::GenericParam(generic.name),
                                        })
                                    }
                                })
                                .collect::<Vec<_>>();
                            let associated_types = item_trait
                                .associated_types
                                .iter()
                                .map(|associated_type| associated_type.name)
                                .collect::<Vec<_>>();
                            lowerer.with_associated_type_scope(
                                AssociatedTypeScope {
                                    self_ty,
                                    trait_id: TraitId::Source(trait_id),
                                    trait_args,
                                    trait_const_args,
                                    names: associated_types,
                                },
                                |lowerer| {
                                    for supertrait in &item_trait.supertraits {
                                        lowerer.lower_type_in_context(
                                            supertrait,
                                            TypeContext::TraitBound,
                                        );
                                    }
                                    lowerer.lower_where_clause(&item_trait.where_clause);
                                    for associated_value in &item_trait.associated_values {
                                        lowerer.lower_type_in_context(
                                            &associated_value.ty,
                                            TypeContext::Value,
                                        );
                                    }
                                    for method in &item_trait.methods {
                                        lowerer.visit_function(&method.function);
                                    }
                                },
                            );
                        } else {
                            for supertrait in &item_trait.supertraits {
                                lowerer.lower_type_in_context(supertrait, TypeContext::TraitBound);
                            }
                            lowerer.lower_where_clause(&item_trait.where_clause);
                            for associated_value in &item_trait.associated_values {
                                lowerer.lower_type_in_context(
                                    &associated_value.ty,
                                    TypeContext::Value,
                                );
                            }
                            for method in &item_trait.methods {
                                lowerer.visit_function(&method.function);
                            }
                        }
                    });
                });
            }
            ItemTreeNodeKind::Extend(extend) => {
                self.with_generics(&extend.generics, |lowerer| {
                    let self_ty =
                        lowerer.lower_type_in_context(&extend.target, TypeContext::ExtendTarget);
                    let trait_scope = extend.trait_ref.as_ref().and_then(|trait_ref| {
                        let trait_ty =
                            lowerer.lower_type_in_context(trait_ref, TypeContext::TraitBound);
                        lowerer.associated_type_scope_for_trait_impl(self_ty, trait_ty)
                    });
                    lowerer.with_self_type(self_ty, |lowerer| {
                        lowerer.lower_where_clause(&extend.where_clause);
                        for associated_type in &extend.associated_types {
                            lowerer.lower_type_in_context(&associated_type.ty, TypeContext::Value);
                        }
                        for associated_value in &extend.associated_values {
                            if let Some(ty) = &associated_value.binding.ty {
                                lowerer.lower_type_in_context(ty, TypeContext::Value);
                            }
                            if lowerer.mode == TypeLowerMode::All
                                && let Some(value) = &associated_value.binding.value
                            {
                                lowerer.visit_expr(value);
                            }
                        }
                        if let Some(trait_scope) = trait_scope {
                            lowerer.with_associated_type_scope(trait_scope, |lowerer| {
                                for method in &extend.methods {
                                    lowerer.visit_function(&method.function);
                                }
                            });
                        } else {
                            for method in &extend.methods {
                                lowerer.visit_function(&method.function);
                            }
                        }
                    });
                });
            }
            ItemTreeNodeKind::Enum(item_enum) => {
                if let Some(backing_type) = &item_enum.backing_type {
                    let ty = self.lower_type_in_context(backing_type, TypeContext::Value);
                    if !self.is_integer(ty) {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_NORMALIZATION,
                            backing_type.span,
                            "enum backing type must be an integer type",
                        ));
                    }
                }
                for variant in &item_enum.variants {
                    match &variant.payload {
                        nia_ast::EnumVariantPayload::Unit => {}
                        nia_ast::EnumVariantPayload::Tuple(fields) => {
                            for field in fields {
                                self.lower_type_in_context(field, TypeContext::Value);
                            }
                        }
                        nia_ast::EnumVariantPayload::Named(fields) => {
                            for field in fields {
                                self.lower_type_in_context(&field.ty, TypeContext::Value);
                            }
                        }
                    }
                }
            }
            ItemTreeNodeKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |lowerer| {
                    lowerer.lower_where_clause(&alias.where_clause);
                    if let Some(ty) = &alias.ty {
                        lowerer.lower_type_in_context(ty, TypeContext::Alias);
                    }
                });
            }
            ItemTreeNodeKind::Binding(binding) => {
                if let Some(ty) = &binding.ty {
                    self.lower_type_in_context(ty, TypeContext::Value);
                }
                if self.mode == TypeLowerMode::All
                    && let Some(value) = &binding.value
                {
                    nia_ast_walk::walk_expr(self, value);
                }
            }
            ItemTreeNodeKind::Function(function) => self.visit_function(function),
            ItemTreeNodeKind::Module(_) | ItemTreeNodeKind::Using(_) => {}
        }
    }
}
