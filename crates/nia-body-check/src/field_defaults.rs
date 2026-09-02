// SPDX-License-Identifier: GPL-3.0-or-later
use std::{cell::Cell, collections::HashMap};

use nia_body_ir::*;
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{ClosureId, GlobalDefId, InternedTyId, LocalId};
use nia_symbol::SymbolMap;
use nia_ty::{ArrayLenTy, ConstGenericArg, ConstGenericValue};

use crate::BodyChecker;

impl BodyChecker<'_> {
    pub(super) fn lower_field_default_templates(&mut self) {
        let fields = self.field_default_sources.keys().copied().collect::<Vec<_>>();
        for field in fields {
            let _ = self.field_default_template(field);
        }
    }

    fn field_default_template(&mut self, field: GlobalDefId) -> Option<TypedExpr> {
        if let Some(template) = self.field_default_templates.get(&field) {
            return Some(template.clone());
        }
        if field.module_id != self.defs.module_id {
            return self
                .program
                .field_default_template
                .and_then(|load| load(field))
                .map(|template| (*template).clone());
        }
        let source = self.field_default_sources.get(&field)?.clone();
        if !self.active_field_default_templates.insert(field) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                source.value.span,
                "recursive struct field default expansion",
            ));
            return Some(TypedExpr {
                span: source.value.span,
                ty: source.ty,
                kind: TypedExprKind::Error,
            });
        }
        let template = self.lower_expr_with_ty(&source.value, Some(source.ty));
        self.active_field_default_templates.remove(&field);
        self.field_default_templates.insert(field, template.clone());
        Some(template)
    }

    pub(crate) fn lower_struct_literal_fields(
        &mut self,
        literal: &nia_ast::Expr,
        def_id: GlobalDefId,
        aggregate_ty: InternedTyId,
        explicit: &[nia_ast::FieldInit],
    ) -> Vec<TypedFieldInit> {
        let Some(resolved) = self.resolved_struct_signature(def_id) else {
            return explicit
                .iter()
                .map(|field| self.lower_explicit_field(aggregate_ty, field))
                .collect();
        };
        let declared_names = resolved
            .signature
            .fields
            .iter()
            .map(|field| field.name)
            .collect::<Vec<_>>();
        let mut lowered = Vec::with_capacity(resolved.signature.fields.len());
        for declared in resolved.signature.fields {
            let matching = explicit
                .iter()
                .filter(|field| field.name == declared.name)
                .collect::<Vec<_>>();
            if matching.is_empty() && declared.has_default {
                let field = GlobalDefId {
                    module_id: def_id.module_id,
                    def_id: declared.def_id,
                };
                if let Some(value) = self.instantiate_local_field_default(
                    def_id,
                    aggregate_ty,
                    field,
                ) {
                    lowered.push(TypedFieldInit {
                        field: Some(field),
                        name: self.symbol_name(declared.name),
                        value,
                        span: declared.span,
                    });
                } else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        literal.span,
                        format!(
                            "default value for field `{}` is unavailable",
                            self.symbol_name(declared.name)
                        ),
                    ));
                }
            } else {
                lowered.extend(
                    matching
                        .into_iter()
                        .map(|field| self.lower_explicit_field(aggregate_ty, field)),
                );
            }
        }
        for field in explicit {
            if !declared_names.contains(&field.name) {
                lowered.push(self.lower_explicit_field(aggregate_ty, field));
            }
        }
        lowered
    }

    fn lower_explicit_field(
        &mut self,
        aggregate_ty: InternedTyId,
        field: &nia_ast::FieldInit,
    ) -> TypedFieldInit {
        let field_ty = self.field_ty_for_aggregate_ty(aggregate_ty, &field.name);
        TypedFieldInit {
            field: self.field_def_for_aggregate_ty(aggregate_ty, &field.name),
            name: self.symbol_name(field.name),
            value: self.lower_expr_with_ty(&field.value, field_ty),
            span: field.span,
        }
    }

    fn instantiate_local_field_default(
        &mut self,
        struct_id: GlobalDefId,
        aggregate_ty: InternedTyId,
        field: GlobalDefId,
    ) -> Option<TypedExpr> {
        if struct_id.module_id != self.defs.module_id {
            return None;
        }
        let template = self.field_default_template(field)?;
        let (args, const_args) = match self.interner.get(aggregate_ty)?.clone() {
            nia_ty::TyKind::Nominal {
                args, const_args, ..
            } => (args, const_args),
            _ => return None,
        };
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(struct_id, &args, &const_args);
        Some(self.instantiate_field_default_template(
            template,
            &substitutions,
            &const_substitutions,
        ))
    }

    fn instantiate_field_default_template(
        &mut self,
        mut template: TypedExpr,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> TypedExpr {
        let mut instantiator = TemplateInstantiator {
            type_store: self.type_store,
            append: &self.interner.append,
            substitutions,
            const_substitutions,
            local_ids: HashMap::new(),
            closure_ids: HashMap::new(),
            next_local: Cell::new(self.next_instantiated_local_id),
            next_closure: Cell::new(self.next_closure_ordinal),
            closure_owner: self.current_def_id,
        };
        instantiator.expr(&mut template);
        self.next_instantiated_local_id = instantiator.next_local.get();
        self.next_closure_ordinal = instantiator.next_closure.get();
        template
    }
}

struct TemplateInstantiator<'a> {
    type_store: &'a nia_ty::TypeStore,
    append: &'a nia_ty::TypeStoreAppend,
    substitutions: &'a SymbolMap<InternedTyId>,
    const_substitutions: &'a SymbolMap<ConstGenericArg>,
    local_ids: HashMap<LocalId, LocalId>,
    closure_ids: HashMap<ClosureId, ClosureId>,
    next_local: Cell<u32>,
    next_closure: Cell<u32>,
    closure_owner: Option<GlobalDefId>,
}

impl TemplateInstantiator<'_> {
    fn local(&mut self, id: &mut LocalId) {
        *id = *self.local_ids.entry(*id).or_insert_with(|| {
            let next = self.next_local.get();
            self.next_local.set(next.saturating_add(1));
            LocalId(next)
        });
    }

    fn closure(&mut self, id: &mut ClosureId) {
        *id = *self.closure_ids.entry(*id).or_insert_with(|| {
            let Some(owner) = self.closure_owner else {
                return *id;
            };
            let ordinal = self.next_closure.get();
            self.next_closure.set(ordinal.saturating_add(1));
            ClosureId { owner, ordinal }
        });
    }

    fn ty(&self, ty: &mut InternedTyId) {
        *ty = nia_ty::substitute_ty_with_closures(
            self.type_store,
            self.append,
            *ty,
            &|name| self.substitutions.get(name).copied(),
            &|name| self.const_substitutions.get(name).cloned(),
            None,
            &|id| self.closure_ids.get(&id).copied().unwrap_or(id),
        );
    }

    fn const_arg(&self, arg: &mut ConstGenericArg) {
        if let ConstGenericValue::GenericParam(name) = arg.value
            && let Some(value) = self.const_substitutions.get(&name)
        {
            *arg = value.clone();
        }
        self.ty(&mut arg.ty);
    }

    fn tys(&self, tys: &mut [InternedTyId]) {
        for ty in tys {
            self.ty(ty);
        }
    }

    fn const_args(&self, args: &mut [ConstGenericArg]) {
        for arg in args {
            self.const_arg(arg);
        }
    }

    fn array_len(&self, len: &mut ArrayLenTy) {
        match len {
            ArrayLenTy::GenericParam(name) => {
                if let Some(arg) = self.const_substitutions.get(name)
                    && let Some(substituted) = nia_ty::array_len_from_const_arg(arg)
                {
                    *len = substituted;
                }
            }
            ArrayLenTy::Builtin { ty, .. } => self.ty(ty),
            ArrayLenTy::Infer | ArrayLenTy::ConstValue(_) | ArrayLenTy::ConstExpr(_) => {}
        }
    }

    fn body(&mut self, body: &mut TypedBody) {
        self.ty(&mut body.ty);
        for local in &mut body.locals {
            self.local(&mut local.id);
            self.ty(&mut local.ty);
        }
        for stmt in &mut body.stmts {
            self.stmt(stmt);
        }
        if let Some(tail) = &mut body.tail {
            self.expr(tail);
        }
    }

    fn stmt(&mut self, stmt: &mut TypedStmt) {
        match &mut stmt.kind {
            TypedStmtKind::Binding(binding) => {
                self.local(&mut binding.local_id);
                self.ty(&mut binding.ty);
                if let Some(value) = &mut binding.value {
                    self.expr(value);
                }
            }
            TypedStmtKind::PatternBinding(binding) => {
                self.pattern(&mut binding.pattern);
                self.expr(&mut binding.value);
            }
            TypedStmtKind::Expr(expr) | TypedStmtKind::Defer(expr) => self.expr(expr),
            TypedStmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.expr(expr);
                }
            }
            TypedStmtKind::ForIn(for_in) => {
                self.pattern(&mut for_in.pattern);
                self.ty(&mut for_in.item_ty);
                self.ty(&mut for_in.bool_ty);
                self.ty(&mut for_in.iterable_self_ty);
                self.ty(&mut for_in.iterator_ty);
                self.expr(&mut for_in.iter);
                self.body(&mut for_in.body);
            }
            TypedStmtKind::While(while_) => {
                self.expr(&mut while_.cond);
                self.body(&mut while_.body);
            }
            TypedStmtKind::Loop(loop_) => self.body(&mut loop_.body),
            TypedStmtKind::Break | TypedStmtKind::Continue => {}
        }
    }

    fn pattern(&mut self, pattern: &mut TypedPattern) {
        self.ty(&mut pattern.ty);
        match &mut pattern.kind {
            TypedPatternKind::Bind { local_id, .. } => self.local(local_id),
            TypedPatternKind::Pointer(inner)
            | TypedPatternKind::MutPointer(inner)
            | TypedPatternKind::OptionalSome(inner)
            | TypedPatternKind::ErrorOk(inner)
            | TypedPatternKind::ErrorErr(inner) => self.pattern(inner),
            TypedPatternKind::Tuple(patterns) => {
                for pattern in patterns {
                    self.pattern(pattern);
                }
            }
            TypedPatternKind::Nominal {
                constructor,
                fields,
            } => {
                if let TypedNominalPatternConstructor::EnumVariant { backing_type, .. } = constructor
                {
                    self.ty(backing_type);
                }
                for pattern in fields {
                    self.pattern(pattern);
                }
            }
            TypedPatternKind::Expr(expr) => self.expr(expr),
            TypedPatternKind::Range { start, end, .. } => {
                self.expr(start);
                self.expr(end);
            }
            TypedPatternKind::Wildcard
            | TypedPatternKind::OptionalNull
            | TypedPatternKind::CheckedInt { .. }
            | TypedPatternKind::CheckedIntRange { .. } => {}
        }
    }

    fn expr(&mut self, expr: &mut TypedExpr) {
        if let TypedExprKind::Closure { closure_id, .. } = &mut expr.kind {
            self.closure(closure_id);
        }
        self.ty(&mut expr.ty);
        match &mut expr.kind {
            TypedExprKind::Local(id) => self.local(id),
            TypedExprKind::ConstGeneric(arg) => self.const_arg(arg),
            TypedExprKind::FunctionInstance {
                args, const_args, ..
            } => {
                self.tys(args);
                self.const_args(const_args);
            }
            TypedExprKind::EnumVariant { fields, .. } | TypedExprKind::Tuple(fields) => {
                for field in fields {
                    self.expr(field);
                }
            }
            TypedExprKind::Closure {
                captures,
                params,
                body,
                ..
            } => {
                for capture in captures {
                    self.local(&mut capture.local_id);
                    self.expr(&mut capture.value);
                }
                for param in params {
                    self.local(param);
                }
                self.body(body);
            }
            TypedExprKind::BuiltinValue(value) => self.builtin(value),
            TypedExprKind::Range(range) => {
                if let Some(start) = &mut range.start {
                    self.expr(start);
                }
                if let Some(end) = &mut range.end {
                    self.expr(end);
                }
            }
            TypedExprKind::InlineAsm(asm) => {
                for input in &mut asm.inputs {
                    self.expr(&mut input.value);
                }
                for output in &mut asm.outputs {
                    self.place(&mut output.place);
                }
            }
            TypedExprKind::MemoryIntrinsic(memory) => {
                self.ty(&mut memory.elem_ty);
                self.expr(&mut memory.dest);
                match &mut memory.source {
                    TypedMemoryIntrinsicSource::Slice(expr)
                    | TypedMemoryIntrinsicSource::Byte(expr) => self.expr(expr),
                }
            }
            TypedExprKind::Atomic(atomic) => self.atomic(atomic),
            TypedExprKind::LoadUnaligned { ty, ptr } => {
                self.ty(ty);
                self.expr(ptr);
            }
            TypedExprKind::Splat { value }
            | TypedExprKind::BitIntrinsic { value, .. }
            | TypedExprKind::CharFromU32 { value }
            | TypedExprKind::StaticArrayPointer { array: value, .. }
            | TypedExprKind::Unary { expr: value, .. }
            | TypedExprKind::OptionalSome { expr: value }
            | TypedExprKind::ErrorOk { expr: value }
            | TypedExprKind::ErrorErr { expr: value }
            | TypedExprKind::Discard(value) => self.expr(value),
            TypedExprKind::ExtractElement { vector, index } => {
                self.expr(vector);
                self.expr(index);
            }
            TypedExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.expr(vector);
                self.expr(index);
                self.expr(value);
            }
            TypedExprKind::Bitmask { vector } => self.expr(vector),
            TypedExprKind::ArrayLiteral { elems } => match elems {
                TypedArrayElements::List(values) => {
                    for value in values {
                        self.expr(value);
                    }
                }
                TypedArrayElements::Repeat { value, count } => {
                    self.expr(value);
                    self.array_len(count);
                }
            },
            TypedExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.expr(&mut field.value);
                }
            }
            TypedExprKind::UnionLiteral { field, .. } => self.expr(&mut field.value),
            TypedExprKind::UnionStorageLiteral { relocations, .. } => {
                for relocation in relocations {
                    self.expr(&mut relocation.pointee);
                }
            }
            TypedExprKind::Try {
                expr,
                error_conversion,
            } => {
                self.expr(expr);
                if let Some(conversion) = error_conversion {
                    self.ty(&mut conversion.source_ty);
                    self.ty(&mut conversion.target_ty);
                    self.tys(&mut conversion.trait_args);
                }
            }
            TypedExprKind::Binary { lhs, rhs, .. }
            | TypedExprKind::Index { lhs, index: rhs } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            TypedExprKind::Assign { place, rhs, .. } => {
                self.place(place);
                self.expr(rhs);
            }
            TypedExprKind::Cast { expr, ty } => {
                self.expr(expr);
                self.ty(ty);
            }
            TypedExprKind::TraitObjectUpcast {
                expr,
                source_ty,
                target_ty,
            } => {
                self.expr(expr);
                self.ty(source_ty);
                self.ty(target_ty);
            }
            TypedExprKind::TraitObjectCoercion {
                expr,
                target_ty,
                self_ty,
            } => {
                self.expr(expr);
                self.ty(target_ty);
                self.ty(self_ty);
            }
            TypedExprKind::CallableCoercion { state, closure_id } => {
                self.expr(state);
                self.closure(closure_id);
            }
            TypedExprKind::ClosureFunctionPointer { closure_id } => self.closure(closure_id),
            TypedExprKind::Call { callee, args } => {
                self.callee(callee);
                for arg in args {
                    self.expr(arg);
                }
            }
            TypedExprKind::Field { lhs, .. } | TypedExprKind::TupleField { lhs, .. } => {
                self.expr(lhs)
            }
            TypedExprKind::Slice { lhs, range, .. } => {
                self.expr(lhs);
                if let Some(start) = &mut range.start {
                    self.expr(start);
                }
                if let Some(end) = &mut range.end {
                    self.expr(end);
                }
            }
            TypedExprKind::Block(body) => self.body(body),
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond);
                self.body(then_branch);
                if let Some(else_branch) = else_branch {
                    self.expr(else_branch);
                }
            }
            TypedExprKind::IfPattern(if_pattern) => {
                self.expr(&mut if_pattern.target);
                self.ty(&mut if_pattern.bool_ty);
                self.pattern(&mut if_pattern.pattern);
                self.body(&mut if_pattern.then_branch);
                if let Some(else_branch) = &mut if_pattern.else_branch {
                    self.expr(else_branch);
                }
            }
            TypedExprKind::Match(matched) => {
                self.expr(&mut matched.target);
                self.ty(&mut matched.bool_ty);
                for arm in &mut matched.arms {
                    for pattern in &mut arm.patterns {
                        self.pattern(pattern);
                    }
                    match &mut arm.body {
                        TypedMatchArmBody::Expr(expr) => self.expr(expr),
                        TypedMatchArmBody::Stmt(stmt) => self.stmt(stmt),
                        TypedMatchArmBody::Block(body) => self.body(body),
                    }
                }
            }
            TypedExprKind::Error
            | TypedExprKind::Integer(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::String(_)
            | TypedExprKind::ByteString(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::ByteChar(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Null
            | TypedExprKind::Global(_)
            | TypedExprKind::Function(_)
            | TypedExprKind::Trap => {}
        }
    }

    fn builtin(&self, builtin: &mut BuiltinConst) {
        match builtin {
            BuiltinConst::Layout { ty, .. } | BuiltinConst::FieldOffset { ty, .. } => self.ty(ty),
            BuiltinConst::Usize(_) | BuiltinConst::Int(_) => {}
        }
    }

    fn atomic(&mut self, atomic: &mut TypedAtomic) {
        match atomic {
            TypedAtomic::Load { ty, ptr, .. } => {
                self.ty(ty);
                self.expr(ptr);
            }
            TypedAtomic::Store { ty, ptr, value, .. }
            | TypedAtomic::Rmw { ty, ptr, value, .. } => {
                self.ty(ty);
                self.expr(ptr);
                self.expr(value);
            }
            TypedAtomic::Cmpxchg {
                ty,
                ptr,
                expected,
                desired,
                ..
            } => {
                self.ty(ty);
                self.expr(ptr);
                self.expr(expected);
                self.expr(desired);
            }
            TypedAtomic::Fence { .. } => {}
        }
    }

    fn place(&mut self, place: &mut TypedPlace) {
        self.ty(&mut place.ty);
        match &mut place.base {
            PlaceBase::Local(id) => self.local(id),
            PlaceBase::Deref(expr) => self.expr(expr),
            PlaceBase::Global(_) | PlaceBase::Error => {}
        }
        for elem in &mut place.elems {
            if let PlaceElem::Index(expr) = elem {
                self.expr(expr);
            }
        }
    }

    fn callee(&mut self, callee: &mut TypedCallee) {
        match callee {
            TypedCallee::Closure(expr)
            | TypedCallee::Callable(expr)
            | TypedCallee::FunctionPointer(expr) => self.expr(expr),
            TypedCallee::FunctionInstance {
                args, const_args, ..
            }
            | TypedCallee::Method {
                args, const_args, ..
            } => {
                self.tys(args);
                self.const_args(const_args);
                if let TypedCallee::Method { receiver, .. } = callee {
                    self.expr(receiver);
                }
            }
            TypedCallee::TraitMethod {
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
                receiver,
                ..
            } => {
                self.ty(self_ty);
                self.tys(trait_args);
                self.const_args(trait_const_args);
                self.tys(args);
                self.const_args(const_args);
                self.expr(receiver);
            }
            TypedCallee::TraitAssociatedFunction {
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
                ..
            } => {
                self.ty(self_ty);
                self.tys(trait_args);
                self.const_args(trait_const_args);
                self.tys(args);
                self.const_args(const_args);
            }
            TypedCallee::DynamicTraitMethod {
                object_ty,
                trait_args,
                trait_const_args,
                params,
                return_type,
                receiver,
                ..
            } => {
                self.ty(object_ty);
                self.tys(trait_args);
                self.const_args(trait_const_args);
                self.tys(params);
                self.ty(return_type);
                self.expr(receiver);
            }
            TypedCallee::BuiltinMethod {
                self_ty, receiver, ..
            } => {
                self.ty(self_ty);
                self.expr(receiver);
            }
            TypedCallee::BuiltinPlaceMethod(method) => {
                self.ty(&mut method.self_ty);
                self.tys(&mut method.trait_args);
                self.expr(&mut method.receiver);
            }
            TypedCallee::Function(_) | TypedCallee::BuiltinOperator(_) => {}
        }
    }
}
