// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical compiler-owned codec for resolved const-function templates.
//!
//! The const evaluator consumes this IR directly. Session-local types and
//! definitions therefore cross the package boundary only through the outer
//! template relocation tables; locals, symbols, and spans already have stable
//! value representations within a function body.

use std::{cell::RefCell, fmt};

use nia_const_ir::*;
use nia_ids::{GlobalDefId, InternedTyId, LocalId, TraitId, ValueBuiltin};
use nia_sema_ir::{AssociatedConstProjection, BuiltinAssociatedValue, PrimitiveIntLimit};
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_ty::{ConstGenericArg, ConstGenericValue, PrimitiveTy};

use crate::template_body_codec::{TemplateBodyEncodeContext, TemplateBodyRelocations};

const MAGIC: &[u8; 8] = b"NIACTF\0\0";
const SCHEMA: u32 = nia_compat::RELEASE_COMPATIBILITY;
const MAX_BYTES: usize = nia_package_metadata::MAX_PACKAGE_BYTES;
const MAX_ITEMS: usize = 1_000_000;
const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CtfeTemplateCodecError(String);

impl CtfeTemplateCodecError {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CtfeTemplateCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CtfeTemplateCodecError {}

pub(crate) fn collect_resolved_const_function_relocations(
    function: &ResolvedConstFunction,
) -> Result<TemplateBodyRelocations, CtfeTemplateCodecError> {
    let context = CollectContext::default();
    let _ = encode_resolved_const_function(function, &context)?;
    Ok(TemplateBodyRelocations {
        types: context.types.into_inner(),
        definitions: context.definitions.into_inner(),
        modules: Vec::new(),
    })
}

#[derive(Default)]
struct CollectContext {
    types: RefCell<Vec<InternedTyId>>,
    definitions: RefCell<Vec<GlobalDefId>>,
}

impl TemplateBodyEncodeContext for CollectContext {
    fn type_index(&self, ty: InternedTyId) -> Option<u32> {
        intern_relocation(&self.types, ty)
    }

    fn definition_index(&self, definition: GlobalDefId) -> Option<u32> {
        intern_relocation(&self.definitions, definition)
    }

    fn module_index(&self, _module: nia_ids::ModuleId) -> Option<u32> {
        None
    }
}

fn intern_relocation<T: Copy + PartialEq>(values: &RefCell<Vec<T>>, value: T) -> Option<u32> {
    if let Some(index) = values
        .borrow()
        .iter()
        .position(|candidate| *candidate == value)
    {
        return u32::try_from(index).ok();
    }
    let mut values = values.borrow_mut();
    let index = u32::try_from(values.len()).ok()?;
    values.push(value);
    Some(index)
}

pub(crate) fn encode_resolved_const_function(
    function: &ResolvedConstFunction,
    context: &dyn TemplateBodyEncodeContext,
) -> Result<Vec<u8>, CtfeTemplateCodecError> {
    let mut encoder = Encoder {
        bytes: Vec::new(),
        context,
    };
    encoder.bytes.extend_from_slice(MAGIC);
    encoder.u32(SCHEMA);
    encoder.function(function, 0)?;
    if encoder.bytes.len() > MAX_BYTES {
        return Err(CtfeTemplateCodecError::invalid(
            "CTFE template body exceeds package size limit",
        ));
    }
    Ok(encoder.bytes)
}

struct Encoder<'a> {
    bytes: Vec<u8>,
    context: &'a dyn TemplateBodyEncodeContext,
}

impl Encoder<'_> {
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u128(&mut self, value: u128) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn len(&mut self, value: usize) -> Result<(), CtfeTemplateCodecError> {
        if value > MAX_ITEMS {
            return Err(CtfeTemplateCodecError::invalid(
                "CTFE template collection exceeds item limit",
            ));
        }
        self.u32(u32::try_from(value).map_err(|_| {
            CtfeTemplateCodecError::invalid("CTFE template collection length overflows u32")
        })?);
        Ok(())
    }

    fn string(&mut self, value: &str) -> Result<(), CtfeTemplateCodecError> {
        self.len(value.len())?;
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn span(&mut self, span: Span) -> Result<(), CtfeTemplateCodecError> {
        self.u64(u64::try_from(span.start).map_err(|_| {
            CtfeTemplateCodecError::invalid("CTFE template span start overflows u64")
        })?);
        self.u64(u64::try_from(span.end).map_err(|_| {
            CtfeTemplateCodecError::invalid("CTFE template span end overflows u64")
        })?);
        Ok(())
    }

    fn symbol(&mut self, symbol: SymbolId) {
        self.u64(symbol.raw());
    }

    fn local(&mut self, local: LocalId) {
        self.u32(local.0);
    }

    fn ty(&mut self, ty: InternedTyId) -> Result<(), CtfeTemplateCodecError> {
        let index = self.context.type_index(ty).ok_or_else(|| {
            CtfeTemplateCodecError::invalid("CTFE template type has no stable relocation")
        })?;
        self.u32(index);
        Ok(())
    }

    fn definition(&mut self, definition: GlobalDefId) -> Result<(), CtfeTemplateCodecError> {
        let index = self.context.definition_index(definition).ok_or_else(|| {
            CtfeTemplateCodecError::invalid("CTFE template definition has no stable relocation")
        })?;
        self.u32(index);
        Ok(())
    }

    fn function(
        &mut self,
        function: &ResolvedConstFunction,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        check_depth(depth)?;
        self.span(function.span())?;
        self.len(function.params().len())?;
        for parameter in function.params() {
            self.param(parameter)?;
        }
        self.block(function.body(), depth + 1)
    }

    fn param(&mut self, parameter: &ResolvedConstParam) -> Result<(), CtfeTemplateCodecError> {
        self.span(parameter.span())?;
        self.symbol(parameter.name());
        self.local(parameter.local_id());
        self.option_ty(parameter.ty())?;
        self.bool(parameter.receiver().is_some());
        if let Some(receiver) = parameter.receiver() {
            self.u32(receiver.stable_tag());
        }
        Ok(())
    }

    fn block(
        &mut self,
        block: &ResolvedConstBlock,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        check_depth(depth)?;
        self.span(block.span())?;
        self.len(block.stmts().len())?;
        for statement in block.stmts() {
            self.stmt(statement, depth + 1)?;
        }
        self.option_expr(block.tail(), depth + 1)
    }

    fn stmt(
        &mut self,
        statement: &ResolvedConstStmt,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        check_depth(depth)?;
        self.span(statement.span())?;
        match statement.kind() {
            ResolvedConstStmtKind::Binding(binding) => {
                self.u8(0);
                self.binding(binding, depth + 1)?;
            }
            ResolvedConstStmtKind::PatternBinding(binding) => {
                self.u8(1);
                self.pattern_binding(binding, depth + 1)?;
            }
            ResolvedConstStmtKind::Expr(expr) => {
                self.u8(2);
                self.expr(expr, depth + 1)?;
            }
            ResolvedConstStmtKind::Return(expr) => {
                self.u8(3);
                self.option_expr(expr.as_ref(), depth + 1)?;
            }
            ResolvedConstStmtKind::Break => self.u8(4),
            ResolvedConstStmtKind::Continue => self.u8(5),
            ResolvedConstStmtKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.u8(6);
                self.expr(cond, depth + 1)?;
                self.block(then_branch, depth + 1)?;
                self.bool(else_branch.is_some());
                if let Some(else_branch) = else_branch {
                    self.block(else_branch, depth + 1)?;
                }
            }
            ResolvedConstStmtKind::ForIn(value) => {
                self.u8(7);
                self.pattern(value.pattern(), depth + 1)?;
                self.expr(value.iter(), depth + 1)?;
                self.block(value.body(), depth + 1)?;
            }
            ResolvedConstStmtKind::While { cond, body } => {
                self.u8(8);
                self.expr(cond, depth + 1)?;
                self.block(body, depth + 1)?;
            }
            ResolvedConstStmtKind::Loop { body } => {
                self.u8(9);
                self.block(body, depth + 1)?;
            }
        }
        Ok(())
    }

    fn binding(
        &mut self,
        binding: &ResolvedConstBinding,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.span(binding.span())?;
        self.symbol(binding.name());
        self.local(binding.local_id());
        self.option_ty(binding.explicit_type())?;
        self.bool(binding.is_mutable());
        self.expr(binding.value(), depth)
    }

    fn pattern_binding(
        &mut self,
        binding: &ResolvedConstPatternBinding,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.span(binding.span())?;
        self.pattern(binding.pattern(), depth)?;
        self.option_ty(binding.explicit_type())?;
        self.bool(binding.is_mutable());
        self.expr(binding.value(), depth)
    }

    fn expr(
        &mut self,
        expr: &ResolvedConstExpr,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        check_depth(depth)?;
        self.span(expr.span())?;
        self.expr_kind(expr.kind(), depth + 1)
    }

    fn expr_kind(
        &mut self,
        kind: &ResolvedConstExprKind,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        check_depth(depth)?;
        match kind {
            ResolvedConstExprKind::Integer(value) => {
                self.u8(0);
                self.string(value)?;
            }
            ResolvedConstExprKind::Char(value) => {
                self.u8(1);
                self.string(value)?;
            }
            ResolvedConstExprKind::ByteChar(value) => {
                self.u8(2);
                self.string(value)?;
            }
            ResolvedConstExprKind::Float(value) => {
                self.u8(3);
                self.string(value)?;
            }
            ResolvedConstExprKind::String(value) => {
                self.u8(4);
                self.string_literal(value)?;
            }
            ResolvedConstExprKind::ByteString(value) => {
                self.u8(5);
                self.string_literal(value)?;
            }
            ResolvedConstExprKind::Bool(value) => {
                self.u8(6);
                self.bool(*value);
            }
            ResolvedConstExprKind::Null => self.u8(7),
            ResolvedConstExprKind::Name(value) => {
                self.u8(8);
                self.name_resolution(value)?;
            }
            ResolvedConstExprKind::Field { lhs, name } => {
                self.u8(9);
                self.expr(lhs, depth + 1)?;
                self.symbol(*name);
            }
            ResolvedConstExprKind::Method { receiver, name } => {
                self.u8(10);
                self.expr(receiver, depth + 1)?;
                self.symbol(*name);
            }
            ResolvedConstExprKind::AssociatedFunction { target, name } => {
                self.u8(11);
                self.associated_target(target)?;
                self.symbol(*name);
            }
            ResolvedConstExprKind::Index { lhs, index } => {
                self.u8(12);
                self.expr(lhs, depth + 1)?;
                self.expr(index, depth + 1)?;
            }
            ResolvedConstExprKind::Slice { lhs, range } => {
                self.u8(13);
                self.expr(lhs, depth + 1)?;
                self.slice_range(range, depth + 1)?;
            }
            ResolvedConstExprKind::Tuple(values) => {
                self.u8(14);
                self.exprs(values, depth + 1)?;
            }
            ResolvedConstExprKind::TupleField { lhs, index } => {
                self.u8(15);
                self.expr(lhs, depth + 1)?;
                self.u64(u64::try_from(*index).map_err(|_| {
                    CtfeTemplateCodecError::invalid("CTFE tuple field index overflows u64")
                })?);
            }
            ResolvedConstExprKind::ArrayLiteral { elems } => {
                self.u8(16);
                self.array_elements(elems, depth + 1)?;
            }
            ResolvedConstExprKind::StructLiteral { ty, fields } => {
                self.u8(17);
                self.ty(*ty)?;
                self.fields(fields, depth + 1)?;
            }
            ResolvedConstExprKind::TupleStructLiteral {
                def_id,
                generic_args,
                fields,
            } => {
                self.u8(18);
                self.definition(*def_id)?;
                self.generic_args(generic_args, depth + 1)?;
                self.fields(fields, depth + 1)?;
            }
            ResolvedConstExprKind::EnumStructLiteral { variant, fields } => {
                self.u8(19);
                self.expr(variant, depth + 1)?;
                self.fields(fields, depth + 1)?;
            }
            ResolvedConstExprKind::CompileError { message } => {
                self.u8(20);
                self.expr(message, depth + 1)?;
            }
            ResolvedConstExprKind::Trap => self.u8(21),
            ResolvedConstExprKind::BuiltinConstValue(value) => {
                self.u8(22);
                self.u32(value.stable_tag());
            }
            ResolvedConstExprKind::BuiltinValue(value) => {
                self.u8(23);
                self.u8(match value {
                    ValueBuiltin::Error => 0,
                });
            }
            ResolvedConstExprKind::LayoutBuiltin { builtin, type_arg } => {
                self.u8(24);
                self.u32(builtin.stable_tag());
                self.type_arg(type_arg)?;
            }
            ResolvedConstExprKind::FieldOffsetBuiltin { type_arg, field } => {
                self.u8(25);
                self.type_arg(type_arg)?;
                self.symbol(*field);
            }
            ResolvedConstExprKind::Embed { path } => {
                self.u8(26);
                self.string_literal(path)?;
            }
            ResolvedConstExprKind::Call {
                callee,
                generic_args,
                args,
            } => {
                self.u8(27);
                self.expr(callee, depth + 1)?;
                self.generic_args(generic_args, depth + 1)?;
                self.exprs(args, depth + 1)?;
            }
            ResolvedConstExprKind::Unary { op, expr } => {
                self.u8(28);
                self.u8(unary_tag(*op));
                self.expr(expr, depth + 1)?;
            }
            ResolvedConstExprKind::OptionalSome { expr } => {
                self.u8(29);
                self.expr(expr, depth + 1)?;
            }
            ResolvedConstExprKind::ErrorOk { expr } => {
                self.u8(30);
                self.expr(expr, depth + 1)?;
            }
            ResolvedConstExprKind::ErrorErr { expr } => {
                self.u8(31);
                self.expr(expr, depth + 1)?;
            }
            ResolvedConstExprKind::Try { expr } => {
                self.u8(32);
                self.expr(expr, depth + 1)?;
            }
            ResolvedConstExprKind::Binary { lhs, op, rhs } => {
                self.u8(33);
                self.expr(lhs, depth + 1)?;
                self.u8(binary_tag(*op));
                self.expr(rhs, depth + 1)?;
            }
            ResolvedConstExprKind::Assign(assign) => {
                self.u8(34);
                self.assign(assign, depth + 1)?;
            }
            ResolvedConstExprKind::Range(range) => {
                self.u8(35);
                self.range(range, depth + 1)?;
            }
            ResolvedConstExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.u8(36);
                self.expr(cond, depth + 1)?;
                self.block(then_branch, depth + 1)?;
                self.option_expr(else_branch.as_deref(), depth + 1)?;
            }
            ResolvedConstExprKind::Match(value) => {
                self.u8(37);
                self.match_expr(value, depth + 1)?;
            }
            ResolvedConstExprKind::Cast { expr, ty } => {
                self.u8(38);
                self.expr(expr, depth + 1)?;
                self.ty(*ty)?;
            }
            ResolvedConstExprKind::Block(block) => {
                self.u8(39);
                self.block(block, depth + 1)?;
            }
        }
        Ok(())
    }

    fn assign(
        &mut self,
        assign: &ResolvedConstAssign,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        match assign.lhs().kind() {
            ResolvedConstAssignTargetKind::Local {
                span,
                name,
                local_id,
                path,
            } => {
                self.u8(0);
                self.span(*span)?;
                self.symbol(*name);
                self.local(*local_id);
                self.len(path.len())?;
                for element in path {
                    match element.kind() {
                        ResolvedConstAssignPathElemKind::Field { span, name } => {
                            self.u8(0);
                            self.span(*span)?;
                            self.symbol(*name);
                        }
                        ResolvedConstAssignPathElemKind::Index { span, index } => {
                            self.u8(1);
                            self.span(*span)?;
                            self.expr(index, depth + 1)?;
                        }
                    }
                }
            }
        }
        self.u8(assign_tag(assign.op()));
        self.expr(assign.rhs(), depth + 1)
    }

    fn match_expr(
        &mut self,
        value: &ResolvedConstMatch,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.span(value.span())?;
        self.expr(value.target(), depth + 1)?;
        self.len(value.arms().len())?;
        for arm in value.arms() {
            self.span(arm.span())?;
            self.len(arm.patterns().len())?;
            for pattern in arm.patterns() {
                self.pattern(pattern, depth + 1)?;
            }
            match arm.body().kind() {
                ResolvedConstMatchArmBodyKind::Expr(expr) => {
                    self.u8(0);
                    self.expr(expr, depth + 1)?;
                }
                ResolvedConstMatchArmBodyKind::Stmt(statement) => {
                    self.u8(1);
                    self.stmt(statement, depth + 1)?;
                }
                ResolvedConstMatchArmBodyKind::Block(block) => {
                    self.u8(2);
                    self.block(block, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    fn pattern(
        &mut self,
        pattern: &ResolvedConstPattern,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        check_depth(depth)?;
        match pattern.kind() {
            ResolvedConstPatternKind::Wildcard { span } => {
                self.u8(0);
                self.span(*span)?;
            }
            ResolvedConstPatternKind::Bind {
                name,
                local_id,
                span,
            } => {
                self.u8(1);
                self.symbol(*name);
                self.local(*local_id);
                self.span(*span)?;
            }
            ResolvedConstPatternKind::Pointer { pattern, span } => {
                self.u8(2);
                self.pattern(pattern, depth + 1)?;
                self.span(*span)?;
            }
            ResolvedConstPatternKind::MutPointer { pattern, span } => {
                self.u8(3);
                self.pattern(pattern, depth + 1)?;
                self.span(*span)?;
            }
            ResolvedConstPatternKind::OptionalSome { pattern, span } => {
                self.u8(4);
                self.pattern(pattern, depth + 1)?;
                self.span(*span)?;
            }
            ResolvedConstPatternKind::OptionalNull { span } => {
                self.u8(5);
                self.span(*span)?;
            }
            ResolvedConstPatternKind::ErrorOk { pattern, span } => {
                self.u8(6);
                self.pattern(pattern, depth + 1)?;
                self.span(*span)?;
            }
            ResolvedConstPatternKind::ErrorErr { pattern, span } => {
                self.u8(7);
                self.pattern(pattern, depth + 1)?;
                self.span(*span)?;
            }
            ResolvedConstPatternKind::Tuple { patterns, span } => {
                self.u8(8);
                self.len(patterns.len())?;
                for pattern in patterns {
                    self.pattern(pattern, depth + 1)?;
                }
                self.span(*span)?;
            }
            ResolvedConstPatternKind::EnumVariant {
                variant,
                fields,
                span,
            } => {
                self.u8(9);
                self.expr(variant, depth + 1)?;
                self.pattern_fields(fields, depth + 1)?;
                self.span(*span)?;
            }
            ResolvedConstPatternKind::Struct {
                def_id,
                fields,
                rest,
                span,
            } => {
                self.u8(10);
                self.definition(*def_id)?;
                self.named_patterns(fields, depth + 1)?;
                self.option_span(*rest)?;
                self.span(*span)?;
            }
            ResolvedConstPatternKind::Expr(expr) => {
                self.u8(11);
                self.expr(expr, depth + 1)?;
            }
            ResolvedConstPatternKind::Range {
                start,
                end,
                inclusive,
                span,
            } => {
                self.u8(12);
                self.expr(start, depth + 1)?;
                self.expr(end, depth + 1)?;
                self.bool(*inclusive);
                self.span(*span)?;
            }
        }
        Ok(())
    }

    fn pattern_fields(
        &mut self,
        fields: &ConstEnumPatternFields<ResolvedConstPattern>,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        match fields {
            ConstEnumPatternFields::Tuple(patterns) => {
                self.u8(0);
                self.len(patterns.len())?;
                for pattern in patterns {
                    self.pattern(pattern, depth + 1)?;
                }
            }
            ConstEnumPatternFields::Named { fields, rest } => {
                self.u8(1);
                self.named_patterns(fields, depth + 1)?;
                self.option_span(*rest)?;
            }
        }
        Ok(())
    }

    fn named_patterns(
        &mut self,
        fields: &[ConstNamedPatternField<ResolvedConstPattern>],
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.len(fields.len())?;
        for field in fields {
            self.symbol(field.name);
            self.pattern(&field.pattern, depth + 1)?;
            self.span(field.span)?;
        }
        Ok(())
    }

    fn fields(
        &mut self,
        fields: &[ResolvedConstFieldInit],
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.len(fields.len())?;
        for field in fields {
            self.span(field.span())?;
            self.symbol(field.name());
            self.expr(field.value(), depth + 1)?;
        }
        Ok(())
    }

    fn array_elements(
        &mut self,
        elements: &ResolvedConstArrayElements,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        match elements.kind() {
            ResolvedConstArrayElementsKind::List(values) => {
                self.u8(0);
                self.exprs(values, depth + 1)?;
            }
            ResolvedConstArrayElementsKind::Repeat { value, count } => {
                self.u8(1);
                self.expr(value, depth + 1)?;
                self.expr(count, depth + 1)?;
            }
        }
        Ok(())
    }

    fn range(
        &mut self,
        range: &ResolvedConstRange,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.option_expr(range.start(), depth + 1)?;
        self.option_expr(range.end(), depth + 1)?;
        self.bool(range.is_inclusive());
        Ok(())
    }

    fn slice_range(
        &mut self,
        range: &ResolvedConstSliceRange,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.option_expr(range.start(), depth + 1)?;
        self.option_expr(range.end(), depth + 1)?;
        self.bool(range.is_inclusive());
        Ok(())
    }

    fn associated_target(
        &mut self,
        target: &ResolvedConstAssociatedTarget,
    ) -> Result<(), CtfeTemplateCodecError> {
        match target {
            ResolvedConstAssociatedTarget::Type(arg) => {
                self.u8(0);
                self.type_arg(arg)?;
            }
            ResolvedConstAssociatedTarget::Nominal { def_id, args } => {
                self.u8(1);
                self.definition(*def_id)?;
                self.len(args.len())?;
                for arg in args {
                    self.type_arg(arg)?;
                }
            }
        }
        Ok(())
    }

    fn generic_args(
        &mut self,
        args: &[ResolvedConstGenericArg],
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.len(args.len())?;
        for arg in args {
            match arg {
                ResolvedConstGenericArg::Infer(span) => {
                    self.u8(0);
                    self.span(*span)?;
                }
                ResolvedConstGenericArg::Type(arg) => {
                    self.u8(1);
                    self.type_arg(arg)?;
                }
                ResolvedConstGenericArg::Const(expr) => {
                    self.u8(2);
                    self.expr(expr, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    fn type_arg(&mut self, arg: &ResolvedConstTypeArg) -> Result<(), CtfeTemplateCodecError> {
        self.span(arg.span())?;
        self.span(arg.ty_span())?;
        self.ty(arg.ty())
    }

    fn name_resolution(
        &mut self,
        resolution: &ConstNameResolution,
    ) -> Result<(), CtfeTemplateCodecError> {
        match resolution {
            ConstNameResolution::Local(local) => {
                self.u8(0);
                self.local(*local);
            }
            ConstNameResolution::Global(definition) => {
                self.u8(1);
                self.definition(*definition)?;
            }
            ConstNameResolution::GenericParam(name) => {
                self.u8(2);
                self.symbol(*name);
            }
            ConstNameResolution::BuiltinAssociatedValue(value) => {
                self.u8(3);
                self.builtin_associated_value(*value);
            }
            ConstNameResolution::AssociatedConstProjection(projection) => {
                self.u8(4);
                self.associated_const_projection(projection)?;
            }
        }
        Ok(())
    }

    fn builtin_associated_value(&mut self, value: BuiltinAssociatedValue) {
        match value {
            BuiltinAssociatedValue::PrimitiveIntLimit { primitive, kind } => {
                self.u8(0);
                self.u8(primitive_tag(primitive));
                self.u8(match kind {
                    PrimitiveIntLimit::Min => 0,
                    PrimitiveIntLimit::Max => 1,
                });
            }
        }
    }

    fn associated_const_projection(
        &mut self,
        projection: &AssociatedConstProjection,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.ty(projection.self_ty)?;
        self.trait_id(projection.trait_id)?;
        self.len(projection.trait_args.len())?;
        for ty in &projection.trait_args {
            self.ty(*ty)?;
        }
        self.len(projection.trait_const_args.len())?;
        for arg in &projection.trait_const_args {
            self.const_generic_arg(arg)?;
        }
        self.symbol(projection.name);
        Ok(())
    }

    fn trait_id(&mut self, trait_id: TraitId) -> Result<(), CtfeTemplateCodecError> {
        match trait_id {
            TraitId::Source(definition) => {
                self.u8(0);
                self.definition(definition)?;
            }
            TraitId::Builtin(builtin) => {
                self.u8(1);
                self.u32(builtin.stable_tag());
            }
        }
        Ok(())
    }

    fn const_generic_arg(&mut self, arg: &ConstGenericArg) -> Result<(), CtfeTemplateCodecError> {
        self.ty(arg.ty)?;
        match arg.value {
            ConstGenericValue::GenericParam(name) => {
                self.u8(0);
                self.symbol(name);
            }
            ConstGenericValue::ConstExpr(_) => {
                return Err(CtfeTemplateCodecError::invalid(
                    "unevaluated const argument cannot cross a package boundary",
                ));
            }
            ConstGenericValue::Int(value) => {
                self.u8(1);
                self.u128(value.bits());
                self.bool(value.is_signed());
            }
            ConstGenericValue::Bool(value) => {
                self.u8(2);
                self.bool(value);
            }
            ConstGenericValue::Char(value) => {
                self.u8(3);
                self.u32(value.into());
            }
        }
        Ok(())
    }

    fn string_literal(
        &mut self,
        literal: &ConstStringLiteral,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.len(literal.parts.len())?;
        for part in &literal.parts {
            self.string(part)?;
        }
        Ok(())
    }

    fn exprs(
        &mut self,
        values: &[ResolvedConstExpr],
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.len(values.len())?;
        for value in values {
            self.expr(value, depth + 1)?;
        }
        Ok(())
    }

    fn option_expr(
        &mut self,
        value: Option<&ResolvedConstExpr>,
        depth: usize,
    ) -> Result<(), CtfeTemplateCodecError> {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.expr(value, depth + 1)?;
        }
        Ok(())
    }

    fn option_ty(&mut self, value: Option<InternedTyId>) -> Result<(), CtfeTemplateCodecError> {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.ty(value)?;
        }
        Ok(())
    }

    fn option_span(&mut self, value: Option<Span>) -> Result<(), CtfeTemplateCodecError> {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.span(value)?;
        }
        Ok(())
    }
}

fn check_depth(depth: usize) -> Result<(), CtfeTemplateCodecError> {
    if depth > MAX_DEPTH {
        Err(CtfeTemplateCodecError::invalid(
            "CTFE template body exceeds recursion limit",
        ))
    } else {
        Ok(())
    }
}

fn unary_tag(value: ConstUnaryOp) -> u8 {
    match value {
        ConstUnaryOp::Neg => 0,
        ConstUnaryOp::Not => 1,
        ConstUnaryOp::BitNot => 2,
        ConstUnaryOp::RefReadOnly => 3,
        ConstUnaryOp::Ref => 4,
        ConstUnaryOp::Deref => 5,
    }
}

fn binary_tag(value: ConstBinaryOp) -> u8 {
    match value {
        ConstBinaryOp::Mul => 0,
        ConstBinaryOp::Div => 1,
        ConstBinaryOp::Rem => 2,
        ConstBinaryOp::Add => 3,
        ConstBinaryOp::Sub => 4,
        ConstBinaryOp::Shl => 5,
        ConstBinaryOp::Shr => 6,
        ConstBinaryOp::Lt => 7,
        ConstBinaryOp::Le => 8,
        ConstBinaryOp::Gt => 9,
        ConstBinaryOp::Ge => 10,
        ConstBinaryOp::Eq => 11,
        ConstBinaryOp::Ne => 12,
        ConstBinaryOp::BitAnd => 13,
        ConstBinaryOp::BitXor => 14,
        ConstBinaryOp::BitOr => 15,
        ConstBinaryOp::And => 16,
        ConstBinaryOp::Or => 17,
    }
}

fn assign_tag(value: ConstAssignOp) -> u8 {
    match value {
        ConstAssignOp::Assign => 0,
        ConstAssignOp::Add => 1,
        ConstAssignOp::Sub => 2,
        ConstAssignOp::Shl => 3,
        ConstAssignOp::Shr => 4,
        ConstAssignOp::Mul => 5,
        ConstAssignOp::Div => 6,
        ConstAssignOp::Rem => 7,
        ConstAssignOp::BitAnd => 8,
        ConstAssignOp::BitXor => 9,
        ConstAssignOp::BitOr => 10,
    }
}

fn primitive_tag(value: PrimitiveTy) -> u8 {
    match value {
        PrimitiveTy::I8 => 0,
        PrimitiveTy::I16 => 1,
        PrimitiveTy::I32 => 2,
        PrimitiveTy::I64 => 3,
        PrimitiveTy::I128 => 4,
        PrimitiveTy::Isize => 5,
        PrimitiveTy::U8 => 6,
        PrimitiveTy::U16 => 7,
        PrimitiveTy::U32 => 8,
        PrimitiveTy::U64 => 9,
        PrimitiveTy::U128 => 10,
        PrimitiveTy::Usize => 11,
        PrimitiveTy::F32 => 12,
        PrimitiveTy::F64 => 13,
        PrimitiveTy::Bool => 14,
        PrimitiveTy::Char => 15,
        PrimitiveTy::Never => 16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyContext;

    impl TemplateBodyEncodeContext for EmptyContext {
        fn type_index(&self, _ty: InternedTyId) -> Option<u32> {
            None
        }

        fn definition_index(&self, _definition: GlobalDefId) -> Option<u32> {
            None
        }

        fn module_index(&self, _module: nia_ids::ModuleId) -> Option<u32> {
            None
        }
    }

    #[test]
    fn resolved_const_function_uses_explicit_schema() {
        let span = Span::new(0, 3);
        let function = ResolvedConstFunction::from_parts(
            span,
            Vec::new(),
            ResolvedConstBlock::new(
                span,
                Vec::new(),
                Some(Box::new(ResolvedConstExpr::from_parts(
                    span,
                    ResolvedConstExprKind::Binary {
                        lhs: Box::new(ResolvedConstExpr::from_parts(
                            span,
                            ResolvedConstExprKind::Integer("21".into()),
                        )),
                        op: ConstBinaryOp::Add,
                        rhs: Box::new(ResolvedConstExpr::from_parts(
                            span,
                            ResolvedConstExprKind::Integer("2".into()),
                        )),
                    },
                ))),
            ),
        );
        let context = EmptyContext;
        let bytes = encode_resolved_const_function(&function, &context).unwrap();
        assert_eq!(&bytes[..8], MAGIC);
        assert_eq!(&bytes[8..12], &SCHEMA.to_le_bytes());
    }
}
