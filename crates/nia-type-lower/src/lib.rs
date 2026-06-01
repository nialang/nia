// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ast::{
    ArrayLen, Expr, ExprKind, FunctionItem, Item, ItemKind, Module, TypeArg, TypeKind,
    TypePathSegment, TypeRef, WhereClause,
};
use nia_ast_walk::{Visitor, walk_module};
use nia_defs::DefCollection;
use nia_diagnostic::Diagnostic;
use nia_ids::{ConstExprId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_span::Span;
use nia_ty::{
    ArrayLenTy, BuiltinTrait, LayoutBuiltin, PrimitiveTy, RangeTyKind, TraitId, TyInterner, TyKind,
};
use nia_type_resolve::{PrimitiveType, TypeNameResolution, TypeResolution};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeLowering {
    pub interner: TyInterner,
    pub type_uses: HashMap<Span, InternedTyId>,
    pub const_exprs: HashMap<GlobalConstExprId, Expr>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramDefsContext<'a> {
    pub defs: Option<&'a HashMap<ModuleId, DefCollection>>,
}

impl<'a> ProgramDefsContext<'a> {
    pub fn empty() -> Self {
        Self { defs: None }
    }
}

pub fn lower_module_types(module: &Module, resolved: &TypeResolution) -> TypeLowering {
    lower_module_types_with_id(ModuleId(0), module, resolved)
}

pub fn lower_module_types_with_id(
    module_id: ModuleId,
    module: &Module,
    resolved: &TypeResolution,
) -> TypeLowering {
    lower_module_types_with_defs(module_id, module, resolved, ProgramDefsContext::empty())
}

pub fn lower_module_types_with_defs(
    module_id: ModuleId,
    module: &Module,
    resolved: &TypeResolution,
    program_defs: ProgramDefsContext<'_>,
) -> TypeLowering {
    let mut lowerer = TypeLowerer {
        module_id,
        resolved,
        program_defs,
        interner: TyInterner::new(module_id),
        type_uses: HashMap::new(),
        const_exprs: HashMap::new(),
        diagnostics: Vec::new(),
        generic_stack: Vec::new(),
        self_type_stack: Vec::new(),
        next_const_expr_id: 0,
    };
    walk_module(&mut lowerer, module);
    TypeLowering {
        interner: lowerer.interner,
        type_uses: lowerer.type_uses,
        const_exprs: lowerer.const_exprs,
        diagnostics: lowerer.diagnostics,
    }
}

struct TypeLowerer<'a> {
    module_id: ModuleId,
    resolved: &'a TypeResolution,
    program_defs: ProgramDefsContext<'a>,
    interner: TyInterner,
    type_uses: HashMap<Span, InternedTyId>,
    const_exprs: HashMap<GlobalConstExprId, Expr>,
    diagnostics: Vec<Diagnostic>,
    generic_stack: Vec<Vec<String>>,
    self_type_stack: Vec<InternedTyId>,
    next_const_expr_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeContext {
    Value,
    Return,
    Alias,
    SizeQuery,
    TraitBound,
}

#[derive(Debug, Clone, Default)]
struct TraitObjectArgs {
    // Trait object syntax accepts both positional trait arguments and
    // `Assoc = Ty` bindings in the same bracket list. Keeping them separated
    // here prevents later phases from depending on parser ordering details.
    trait_args: Vec<InternedTyId>,
    associated_type_bindings: Vec<(String, InternedTyId)>,
}

impl<'ast> Visitor<'ast> for TypeLowerer<'_> {
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
                    let self_ty = lowerer
                        .interner
                        .intern(TyKind::GenericParam("Self".to_string()));
                    lowerer.with_self_type(self_ty, |lowerer| {
                        for supertrait in &item_trait.supertraits {
                            lowerer.lower_type_in_context(supertrait, TypeContext::TraitBound);
                        }
                        lowerer.lower_where_clause(&item_trait.where_clause);
                        for method in &item_trait.methods {
                            lowerer.visit_function(&method.function);
                        }
                    });
                });
            }
            ItemKind::Extend(extend) => {
                self.with_generics(&extend.generics, |lowerer| {
                    let self_ty = lowerer.lower_type_in_context(&extend.target, TypeContext::Value);
                    if let Some(trait_ref) = &extend.trait_ref {
                        lowerer.lower_type_in_context(trait_ref, TypeContext::TraitBound);
                    }
                    lowerer.with_self_type(self_ty, |lowerer| {
                        lowerer.lower_where_clause(&extend.where_clause);
                        for associated_type in &extend.associated_types {
                            lowerer.lower_type_in_context(&associated_type.ty, TypeContext::Value);
                        }
                        for method in &extend.methods {
                            lowerer.visit_function(&method.function);
                        }
                    });
                });
            }
            ItemKind::Enum(item_enum) => {
                if let Some(backing_type) = &item_enum.backing_type {
                    let ty = self.lower_type_in_context(backing_type, TypeContext::Value);
                    if !self.is_integer(ty) {
                        self.diagnostics.push(Diagnostic::error(
                            backing_type.span,
                            "enum backing type must be an integer type",
                        ));
                    }
                }
            }
            ItemKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |lowerer| {
                    lowerer.lower_where_clause(&alias.where_clause);
                    lowerer.lower_type_in_context(&alias.ty, TypeContext::Alias);
                });
            }
            ItemKind::Binding(binding) => {
                if let Some(ty) = &binding.ty {
                    self.lower_type_in_context(ty, TypeContext::Value);
                }
                if let Some(value) = &binding.value {
                    nia_ast_walk::walk_expr(self, value);
                }
            }
            ItemKind::Function(function) => self.visit_function(function),
            ItemKind::Import(_) | ItemKind::Using(_) => {}
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
            if let Some(body) = &function.body {
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
                    if let Some(expr) = &arg.expr {
                        self.visit_expr(expr);
                    }
                    if let Some(ty) = &arg.ty {
                        self.visit_type(ty);
                    }
                }
            }
            ExprKind::TypeTarget { ty } => {
                self.visit_type(ty);
            }
            _ => nia_ast_walk::walk_expr(self, expr),
        }
    }
}

impl<'a> TypeLowerer<'a> {
    fn lower_type_in_context(&mut self, ty: &TypeRef, context: TypeContext) -> InternedTyId {
        let lowered = self.lower_type(ty, context);
        self.type_uses.insert(ty.span, lowered);
        if context == TypeContext::Value
            && let Some(message) = self.invalid_value_type_message(lowered)
        {
            self.diagnostics.push(Diagnostic::error(ty.span, message));
        }
        lowered
    }

    fn lower_type(&mut self, ty: &TypeRef, context: TypeContext) -> InternedTyId {
        match &ty.kind {
            TypeKind::Error => self.interner.error(),
            TypeKind::Infer => {
                self.diagnostics.push(Diagnostic::error(
                    ty.span,
                    "`_` type inference is not valid in this type lowering context",
                ));
                self.interner.error()
            }
            TypeKind::Void => self.interner.primitive(PrimitiveTy::Void),
            TypeKind::Never => self.interner.primitive(PrimitiveTy::Never),
            TypeKind::SelfType => self.self_type_stack.last().copied().unwrap_or_else(|| {
                self.diagnostics.push(Diagnostic::error(
                    ty.span,
                    "`Self` is only valid in traits and extend blocks",
                ));
                self.interner.error()
            }),
            TypeKind::Pointer { is_const, elem } => {
                if let Some(trait_object) = self.lower_trait_object_type(*is_const, elem) {
                    trait_object
                } else {
                    let elem = self.lower_type_in_context(elem, TypeContext::Value);
                    self.interner.intern(TyKind::Pointer {
                        is_const: *is_const,
                        elem,
                    })
                }
            }
            TypeKind::Slice { is_const, elem } => {
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.interner.intern(TyKind::Slice {
                    is_const: *is_const,
                    elem,
                })
            }
            TypeKind::Array { len, elem } => {
                let len = self.lower_array_len(len);
                let elem = self.lower_type_in_context(elem, TypeContext::Value);
                self.interner.intern(TyKind::Array { len, elem })
            }
            TypeKind::Range {
                start,
                end,
                inclusive,
            } => self.lower_range_type(ty.span, start.as_deref(), end.as_deref(), *inclusive),
            TypeKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                let params = params
                    .iter()
                    .map(|param| self.lower_type_in_context(param, TypeContext::Value))
                    .collect();
                let return_type = match return_type {
                    Some(return_type) => {
                        self.lower_type_in_context(return_type, TypeContext::Return)
                    }
                    None => self.interner.primitive(PrimitiveTy::Void),
                };
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic: *is_variadic,
                })
            }
            TypeKind::Path { segments } => {
                let Some(first) = segments.first() else {
                    return self.interner.error();
                };
                let Some(type_segment) = type_name_segment(segments) else {
                    return self.interner.error();
                };
                match self.resolved.type_names.get(&ty.span).copied() {
                    Some(TypeNameResolution::Primitive(primitive)) => {
                        self.interner.primitive(lower_primitive(primitive))
                    }
                    Some(TypeNameResolution::BuiltinTrait(trait_id)) => {
                        self.lower_builtin_trait_type(ty.span, type_segment, trait_id, context)
                    }
                    Some(TypeNameResolution::GenericParam) => self
                        .interner
                        .intern(TyKind::GenericParam(first.name.clone())),
                    Some(TypeNameResolution::Def(def_id)) => {
                        let def_id = self
                            .resolved
                            .qualified_type_names
                            .get(&ty.span)
                            .copied()
                            .unwrap_or(GlobalDefId {
                                module_id: self.module_id,
                                def_id,
                            });
                        self.lower_path_type(ty.span, type_segment, def_id, context)
                    }
                    Some(TypeNameResolution::External(global_id)) => {
                        self.lower_path_type(ty.span, type_segment, global_id, context)
                    }
                    Some(TypeNameResolution::Error) | None => self.interner.error(),
                }
            }
            TypeKind::Projection {
                ty,
                trait_ref,
                name,
            } => {
                let self_ty = self.lower_type_in_context(ty, TypeContext::Value);
                let trait_ty = self.lower_type_in_context(trait_ref, TypeContext::TraitBound);
                let trait_ty = self.normalize_if_known(trait_ty);
                let Some((trait_id, args)) = self.projection_trait_id(trait_ty) else {
                    self.diagnostics.push(Diagnostic::error(
                        trait_ref.span,
                        "projection trait must resolve to a trait",
                    ));
                    return self.interner.error();
                };
                if !self.trait_id_has_associated_type(trait_id, name) {
                    self.diagnostics.push(Diagnostic::error(
                        ty.span,
                        format!("trait does not define associated type `{name}`"),
                    ));
                    return self.interner.error();
                }
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args: args,
                    name: name.clone(),
                })
            }
        }
    }

    fn lower_range_type(
        &mut self,
        span: Span,
        start: Option<&TypeRef>,
        end: Option<&TypeRef>,
        inclusive: bool,
    ) -> InternedTyId {
        let start_ty = start.map(|ty| self.lower_type_in_context(ty, TypeContext::Value));
        let end_ty = end.map(|ty| self.lower_type_in_context(ty, TypeContext::Value));
        let kind = match (start_ty, end_ty, inclusive) {
            (Some(_), Some(_), false) => RangeTyKind::Exclusive,
            (Some(_), Some(_), true) => RangeTyKind::Inclusive,
            (Some(_), None, false) => RangeTyKind::From,
            (None, Some(_), false) => RangeTyKind::To,
            (None, Some(_), true) => RangeTyKind::ToInclusive,
            (None, None, false) => RangeTyKind::Full,
            (Some(_), None, true) | (None, None, true) => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "inclusive range type requires an end bound",
                ));
                return self.interner.error();
            }
        };
        let bound = match (start_ty, end_ty) {
            (Some(start_ty), Some(end_ty)) => {
                if !self.types_equivalent(start_ty, end_ty) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        "range type bounds must have the same type",
                    ));
                    return self.interner.error();
                }
                Some(start_ty)
            }
            (Some(bound), None) | (None, Some(bound)) => Some(bound),
            (None, None) => None,
        };
        if let Some(bound) = bound
            && !self.is_integer(bound)
        {
            self.diagnostics.push(Diagnostic::error(
                span,
                "range bound type must be an integer type",
            ));
            return self.interner.error();
        }
        self.interner.intern(TyKind::Range { kind, bound })
    }

    fn normalize_if_known(&self, ty: InternedTyId) -> InternedTyId {
        ty
    }

    fn lower_path_type(
        &mut self,
        span: Span,
        segment: &TypePathSegment,
        def_id: GlobalDefId,
        context: TypeContext,
    ) -> InternedTyId {
        let mut args = Vec::new();
        let mut seen_assoc_bindings = HashSet::new();
        let mut seen_assoc_binding = false;
        for arg in &segment.args {
            match arg {
                TypeArg::Type(arg_ty) => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::error(
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    args.push(self.lower_type_in_context(arg_ty, TypeContext::Value));
                }
                TypeArg::Const(expr) => {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "const generic type arguments are not supported",
                    ));
                }
                TypeArg::AssocBinding {
                    name,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    if context == TypeContext::TraitBound {
                        self.lower_type_in_context(binding_ty, TypeContext::Value);
                        if !self.is_trait_def(def_id) {
                            self.diagnostics.push(Diagnostic::error(
                                *span,
                                "associated type bindings require a trait bound",
                            ));
                        } else {
                            if !seen_assoc_bindings.insert(name.as_str()) {
                                self.diagnostics.push(Diagnostic::error(
                                    *span,
                                    format!("duplicate associated type binding `{name}`"),
                                ));
                            }
                            if !self.trait_has_associated_type(def_id, name) {
                                self.diagnostics.push(Diagnostic::error(
                                    *span,
                                    format!("trait does not define associated type `{name}`"),
                                ));
                            }
                        }
                    } else {
                        self.diagnostics.push(Diagnostic::error(
                            *span,
                            "associated type bindings are only valid in trait bounds",
                        ));
                    }
                }
            }
        }
        self.check_type_arg_count(span, def_id, args.len());
        self.interner.intern(TyKind::Nominal { def_id, args })
    }

    fn lower_trait_object_type(&mut self, is_const: bool, ty: &TypeRef) -> Option<InternedTyId> {
        let TypeKind::Path { segments } = &ty.kind else {
            return None;
        };
        let type_segment = type_name_segment(segments)?;
        match self.resolved.type_names.get(&ty.span).copied() {
            Some(TypeNameResolution::BuiltinTrait(trait_id)) => {
                Some(self.lower_builtin_trait_object(ty.span, is_const, type_segment, trait_id))
            }
            Some(TypeNameResolution::Def(def_id)) => {
                let def_id = self
                    .resolved
                    .qualified_type_names
                    .get(&ty.span)
                    .copied()
                    .unwrap_or(GlobalDefId {
                        module_id: self.module_id,
                        def_id,
                    });
                self.lower_source_trait_object(ty.span, is_const, type_segment, def_id)
            }
            Some(TypeNameResolution::External(def_id)) => {
                self.lower_source_trait_object(ty.span, is_const, type_segment, def_id)
            }
            _ => None,
        }
    }

    fn lower_source_trait_object(
        &mut self,
        span: Span,
        is_const: bool,
        segment: &TypePathSegment,
        def_id: GlobalDefId,
    ) -> Option<InternedTyId> {
        if !self.is_trait_def(def_id) {
            return None;
        }
        let object_args = self.lower_trait_object_args(span, segment, TraitId::Source(def_id))?;
        self.check_type_arg_count(span, def_id, object_args.trait_args.len());
        Some(self.interner.intern(TyKind::TraitObject {
            is_const,
            trait_id: TraitId::Source(def_id),
            trait_args: object_args.trait_args,
            associated_type_bindings: object_args.associated_type_bindings,
        }))
    }

    fn lower_builtin_trait_object(
        &mut self,
        span: Span,
        is_const: bool,
        segment: &TypePathSegment,
        trait_id: BuiltinTrait,
    ) -> InternedTyId {
        let object_args = self
            .lower_trait_object_args(span, segment, TraitId::Builtin(trait_id))
            .unwrap_or_default();
        self.check_builtin_trait_arg_count(span, trait_id, object_args.trait_args.len());
        self.interner.intern(TyKind::TraitObject {
            is_const,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: object_args.trait_args,
            associated_type_bindings: object_args.associated_type_bindings,
        })
    }

    fn lower_trait_object_args(
        &mut self,
        _span: Span,
        segment: &TypePathSegment,
        trait_id: TraitId,
    ) -> Option<TraitObjectArgs> {
        let mut object_args = TraitObjectArgs::default();
        let mut seen_assoc_bindings = HashSet::new();
        let mut seen_assoc_binding = false;
        for arg in &segment.args {
            match arg {
                TypeArg::Type(arg_ty) => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::error(
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    object_args
                        .trait_args
                        .push(self.lower_type_in_context(arg_ty, TypeContext::Value));
                }
                TypeArg::Const(expr) => {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "const generic type arguments are not supported",
                    ));
                }
                TypeArg::AssocBinding {
                    name,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    let binding_ty = self.lower_type_in_context(binding_ty, TypeContext::Value);
                    if !seen_assoc_bindings.insert(name.as_str()) {
                        self.diagnostics.push(Diagnostic::error(
                            *span,
                            format!("duplicate associated type binding `{name}`"),
                        ));
                    }
                    if !self.trait_id_has_associated_type(trait_id, name) {
                        self.diagnostics.push(Diagnostic::error(
                            *span,
                            format!("trait does not define associated type `{name}`"),
                        ));
                    }
                    object_args
                        .associated_type_bindings
                        .push((name.clone(), binding_ty));
                }
            }
        }
        Some(object_args)
    }

    fn lower_builtin_trait_type(
        &mut self,
        span: Span,
        segment: &TypePathSegment,
        trait_id: BuiltinTrait,
        context: TypeContext,
    ) -> InternedTyId {
        let mut args = Vec::new();
        let mut seen_assoc_bindings = HashSet::new();
        let mut seen_assoc_binding = false;
        for arg in &segment.args {
            match arg {
                TypeArg::Type(arg_ty) => {
                    if seen_assoc_binding {
                        self.diagnostics.push(Diagnostic::error(
                            arg_ty.span,
                            "positional type arguments must precede associated type bindings",
                        ));
                    }
                    args.push(self.lower_type_in_context(arg_ty, TypeContext::Value));
                }
                TypeArg::Const(expr) => {
                    self.diagnostics.push(Diagnostic::error(
                        expr.span,
                        "const generic type arguments are not supported",
                    ));
                }
                TypeArg::AssocBinding {
                    name,
                    span,
                    ty: binding_ty,
                } => {
                    seen_assoc_binding = true;
                    if context == TypeContext::TraitBound {
                        self.lower_type_in_context(binding_ty, TypeContext::Value);
                        if !seen_assoc_bindings.insert(name.as_str()) {
                            self.diagnostics.push(Diagnostic::error(
                                *span,
                                format!("duplicate associated type binding `{name}`"),
                            ));
                        }
                        if !trait_id.has_associated_type(name) {
                            self.diagnostics.push(Diagnostic::error(
                                *span,
                                format!("trait does not define associated type `{name}`"),
                            ));
                        }
                    } else {
                        self.diagnostics.push(Diagnostic::error(
                            *span,
                            "associated type bindings are only valid in trait bounds",
                        ));
                    }
                }
            }
        }
        self.check_builtin_trait_arg_count(span, trait_id, args.len());
        self.interner
            .intern(TyKind::BuiltinTrait { trait_id, args })
    }

    fn projection_trait_id(&self, trait_ty: InternedTyId) -> Option<(TraitId, Vec<InternedTyId>)> {
        match self.interner.get(trait_ty) {
            Some(TyKind::Nominal { def_id, args }) if self.is_trait_def(*def_id) => {
                Some((TraitId::Source(*def_id), args.clone()))
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(*trait_id), args.clone()))
            }
            _ => None,
        }
    }

    fn is_trait_def(&self, def_id: GlobalDefId) -> bool {
        self.defs_for_module(def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id))
            .is_some_and(|def| def.kind == nia_defs::DefKind::Trait)
    }

    fn trait_has_associated_type(&self, trait_id: GlobalDefId, name: &str) -> bool {
        let Some(defs) = self.defs_for_module(trait_id.module_id) else {
            return true;
        };
        let Some(members) = defs.scopes.struct_members.get(&trait_id.def_id) else {
            return true;
        };
        members.fields.get(name).is_some_and(|def_id| {
            defs.defs
                .get(def_id)
                .is_some_and(|def| def.kind == nia_defs::DefKind::TraitAssociatedType)
        })
    }

    fn trait_id_has_associated_type(&self, trait_id: TraitId, name: &str) -> bool {
        match trait_id {
            TraitId::Source(def_id) => self.trait_has_associated_type(def_id, name),
            TraitId::Builtin(trait_id) => trait_id.has_associated_type(name),
        }
    }

    fn check_type_arg_count(&mut self, span: Span, def_id: GlobalDefId, actual: usize) {
        let Some(defs) = self.defs_for_module(def_id.module_id) else {
            return;
        };
        let Some(def) = defs.defs.get(def_id.def_id) else {
            return;
        };
        let expected = def.generics.len();
        if expected != actual {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "generic argument count mismatch for `{}`: expected {expected}, got {actual}",
                    def.name
                ),
            ));
        }
    }

    fn check_builtin_trait_arg_count(&mut self, span: Span, trait_id: BuiltinTrait, actual: usize) {
        let expected = trait_id.generic_count();
        if expected != actual {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "generic argument count mismatch for `{}`: expected {expected}, got {actual}",
                    trait_id.name()
                ),
            ));
        }
    }

    fn with_generics(&mut self, generics: &[String], f: impl FnOnce(&mut Self)) {
        self.generic_stack.push(generics.to_vec());
        f(self);
        self.generic_stack.pop();
    }

    fn with_self_type(&mut self, self_ty: InternedTyId, f: impl FnOnce(&mut Self)) {
        self.self_type_stack.push(self_ty);
        f(self);
        self.self_type_stack.pop();
    }

    fn lower_where_clause(&mut self, clause: &WhereClause) {
        for predicate in &clause.predicates {
            self.lower_type_in_context(&predicate.ty, TypeContext::Value);
            for bound in &predicate.bounds {
                self.lower_trait_bound(bound);
            }
        }
    }

    fn lower_trait_bound(&mut self, bound: &nia_ast::TypeRef) {
        self.lower_type_in_context(bound, TypeContext::TraitBound);
    }

    fn lower_array_len(&mut self, len: &ArrayLen) -> ArrayLenTy {
        match len {
            ArrayLen::Infer => ArrayLenTy::Infer,
            ArrayLen::Expr(expr) => self.lower_array_len_expr(expr),
        }
    }

    fn lower_array_len_expr(&mut self, expr: &Expr) -> ArrayLenTy {
        match &expr.kind {
            ExprKind::Builtin {
                name,
                type_arg: Some(type_arg),
            } if LayoutBuiltin::from_name(name).is_some() => ArrayLenTy::Builtin {
                builtin: LayoutBuiltin::from_name(name)
                    .expect("layout builtin was checked in match guard"),
                ty: self.lower_type_in_context(type_arg, TypeContext::SizeQuery),
            },
            ExprKind::Call { callee, args }
                if args.is_empty()
                    && matches!(
                        &callee.kind,
                        ExprKind::Builtin {
                            name,
                            type_arg: Some(_),
                        } if LayoutBuiltin::from_name(name).is_some()
                    ) =>
            {
                let ExprKind::Builtin {
                    name,
                    type_arg: Some(type_arg),
                } = &callee.kind
                else {
                    return self.register_const_array_len(expr);
                };
                ArrayLenTy::Builtin {
                    builtin: LayoutBuiltin::from_name(name)
                        .expect("layout builtin was checked in match guard"),
                    ty: self.lower_type_in_context(type_arg, TypeContext::SizeQuery),
                }
            }
            _ => self.register_const_array_len(expr),
        }
    }

    fn register_const_array_len(&mut self, expr: &Expr) -> ArrayLenTy {
        let id = GlobalConstExprId {
            module_id: self.module_id,
            const_expr_id: ConstExprId(self.next_const_expr_id),
        };
        self.next_const_expr_id += 1;
        self.const_exprs.insert(id, expr.clone());
        ArrayLenTy::ConstExpr(id)
    }

    fn is_integer(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
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

    fn types_equivalent(&self, left: InternedTyId, right: InternedTyId) -> bool {
        left == right || self.interner.get(left) == self.interner.get(right)
    }

    fn invalid_value_type_message(&self, ty: InternedTyId) -> Option<&'static str> {
        match self.interner.get(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Never)) => {
                Some("`!` is not valid as a value, field, parameter, or array element type")
            }
            Some(TyKind::BuiltinTrait { .. }) => Some(
                "trait types are not valid as values, fields, parameters, or array elements; use `&Trait[...]` or `&const Trait[...]` for a trait object",
            ),
            Some(TyKind::Nominal { def_id, .. }) if self.is_trait_def(*def_id) => Some(
                "trait types are not valid as values, fields, parameters, or array elements; use `&Trait[...]` or `&const Trait[...]` for a trait object",
            ),
            _ => None,
        }
    }

    fn defs_for_module(&self, module_id: ModuleId) -> Option<&DefCollection> {
        self.program_defs.defs?.get(&module_id)
    }
}

fn type_name_segment(segments: &[TypePathSegment]) -> Option<&TypePathSegment> {
    segments.last()
}

fn lower_primitive(primitive: PrimitiveType) -> PrimitiveTy {
    match primitive {
        PrimitiveType::I8 => PrimitiveTy::I8,
        PrimitiveType::I16 => PrimitiveTy::I16,
        PrimitiveType::I32 => PrimitiveTy::I32,
        PrimitiveType::I64 => PrimitiveTy::I64,
        PrimitiveType::I128 => PrimitiveTy::I128,
        PrimitiveType::Isize => PrimitiveTy::Isize,
        PrimitiveType::U8 => PrimitiveTy::U8,
        PrimitiveType::U16 => PrimitiveTy::U16,
        PrimitiveType::U32 => PrimitiveTy::U32,
        PrimitiveType::U64 => PrimitiveTy::U64,
        PrimitiveType::U128 => PrimitiveTy::U128,
        PrimitiveType::Usize => PrimitiveTy::Usize,
        PrimitiveType::F32 => PrimitiveTy::F32,
        PrimitiveType::F64 => PrimitiveTy::F64,
        PrimitiveType::Bool => PrimitiveTy::Bool,
        PrimitiveType::Char => PrimitiveTy::Char,
        PrimitiveType::Void => PrimitiveTy::Void,
        PrimitiveType::Never => PrimitiveTy::Never,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_parser::parse_module;
    use nia_type_resolve::resolve_module_types;
    use std::collections::HashMap;

    #[test]
    fn lowers_primitive_pointer_array_function_and_nominal_types() {
        let (module, errors) = parse_module(
            r#"
struct Box[T] {
    value: T,
}

fn make(ptr: &const u8, cb: &const fn(i32) void) [4]Box[i32] {
    var tmp: [_]i32 = [1, 2, 3];
    [{ value: 0 }; 4]
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let lowered = lower_module_types(&module, &resolved);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        assert!(
            lowered
                .interner
                .get(lowered.interner.error())
                .is_some_and(|ty| matches!(ty, TyKind::Error))
        );
        assert!(
            lowered
                .interner
                .get(lowered.interner.primitive(PrimitiveTy::I8))
                .is_some_and(|_| lowered.interner.len() > 1)
        );
        assert!(
            lowered
                .type_uses
                .values()
                .any(|ty_id| matches!(lowered.interner.get(*ty_id), Some(TyKind::Nominal { .. })))
        );
        assert!(
            lowered
                .type_uses
                .values()
                .any(|ty_id| matches!(lowered.interner.get(*ty_id), Some(TyKind::Array { .. })))
        );
        assert!(
            lowered
                .type_uses
                .values()
                .any(|ty_id| matches!(lowered.interner.get(*ty_id), Some(TyKind::Pointer { .. })))
        );
    }

    #[test]
    fn rejects_const_generic_type_arguments() {
        let (module, errors) = parse_module(
            r#"
struct Box[T] {
    value: T,
}

fn make() Box[4] {
    { value: 0 }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types(&module, &resolved);
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("const generic"))
        );
    }

    #[test]
    fn reports_generic_type_argument_count_mismatches() {
        let (module, errors) = parse_module(
            r#"
struct Point {}
struct Box[T] { value: T }
type Pair[T, U] = T;
fn missing_arg(a: Box) {}
fn extra_arg(a: Box[i32, bool]) {}
fn alias_missing_arg(a: Pair[i32]) {}
fn non_generic_arg(a: Point[i32]) {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let program_defs = HashMap::from([(ModuleId(0), defs.clone())]);
        let lowered = lower_module_types_with_defs(
            ModuleId(0),
            &module,
            &resolved,
            ProgramDefsContext {
                defs: Some(&program_defs),
            },
        );
        let mismatch_count = lowered
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .message
                    .contains("generic argument count mismatch")
            })
            .count();
        assert_eq!(mismatch_count, 4, "{:?}", lowered.diagnostics);
    }

    #[test]
    fn accepts_void_value_types_but_rejects_never_value_types_and_enum_backing_types() {
        let (module, errors) = parse_module(
            r#"
enum Bad: bool {
    A,
}

struct BadFields {
    field: void,
    array: [1]void,
    never_field: !,
}

fn bad_param(x: void) void {}
fn bad_never_param(x: !) void {}
fn good_return() void {}
fn good_never_return() ! {}

var global_void: void;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types(&module, &resolved);
        assert!(
            lowered.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("enum backing type must be an integer type")),
            "{:?}",
            lowered.diagnostics
        );
        assert_eq!(
            lowered
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("`!` is not valid"))
                .count(),
            2,
            "{:?}",
            lowered.diagnostics
        );
    }

    #[test]
    fn lowers_trait_object_pointer_types() {
        let (module, errors) = parse_module(
            r#"
trait Source[T] {
    type Item;
}

fn read(source: &const Source[i32, Item = i32]) void {}
fn write(source: &Source[i32, Item = i32]) void {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let program_defs = HashMap::from([(ModuleId(0), defs.clone())]);
        let lowered = lower_module_types_with_defs(
            ModuleId(0),
            &module,
            &resolved,
            ProgramDefsContext {
                defs: Some(&program_defs),
            },
        );
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let trait_objects = lowered
            .type_uses
            .values()
            .filter_map(|ty| match lowered.interner.get(*ty) {
                Some(TyKind::TraitObject {
                    is_const,
                    trait_args,
                    associated_type_bindings,
                    ..
                }) => Some((*is_const, trait_args.len(), associated_type_bindings.len())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(trait_objects.contains(&(true, 1, 1)), "{trait_objects:?}");
        assert!(trait_objects.contains(&(false, 1, 1)), "{trait_objects:?}");
    }

    #[test]
    fn validates_trait_object_associated_type_bindings() {
        let (module, errors) = parse_module(
            r#"
trait Source {
    type Item;
}

fn unknown(source: &const Source[Missing = i32]) void {}
fn duplicate(source: &const Source[Item = i32, Item = bool]) void {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let program_defs = HashMap::from([(ModuleId(0), defs.clone())]);
        let lowered = lower_module_types_with_defs(
            ModuleId(0),
            &module,
            &resolved,
            ProgramDefsContext {
                defs: Some(&program_defs),
            },
        );
        assert!(
            lowered.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("trait does not define associated type `Missing`")),
            "{:?}",
            lowered.diagnostics
        );
        assert!(
            lowered.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("duplicate associated type binding `Item`")),
            "{:?}",
            lowered.diagnostics
        );
    }

    #[test]
    fn rejects_bare_trait_as_value_type() {
        let (module, errors) = parse_module(
            r#"
trait Show {}

fn bad(value: Show) void {}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let program_defs = HashMap::from([(ModuleId(0), defs.clone())]);
        let lowered = lower_module_types_with_defs(
            ModuleId(0),
            &module,
            &resolved,
            ProgramDefsContext {
                defs: Some(&program_defs),
            },
        );
        assert!(
            lowered
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("trait types are not valid")),
            "{:?}",
            lowered.diagnostics
        );
    }
}
