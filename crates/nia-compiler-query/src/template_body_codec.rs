// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical compiler-owned codec for checked function templates.
//!
//! Package metadata owns the outer template inventory and stable relocation
//! tables. This module owns the executable Function IR schema. Every session
//! identity crosses the wire through an explicit relocation index, and every
//! decoded body is structurally validated before it can enter the query graph.

use std::{cell::RefCell, fmt};

use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_function_ir::*;
use nia_ids::{ClosureId, GlobalDefId, InternedTyId, ModuleId};
use nia_source::SourceLocation;
use nia_span::Span;
use nia_ty::{ArrayLenTy, ConstGenericArg, ConstGenericValue, TraitId};

const MAGIC: &[u8; 8] = b"NIAFIR\0\0";
const SCHEMA: u32 = nia_compat::RELEASE_COMPATIBILITY;
const MAX_BYTES: usize = nia_package_metadata::MAX_PACKAGE_BYTES;
const MAX_ITEMS: usize = 1_000_000;
const MAX_DEPTH: usize = 256;

pub(crate) trait TemplateBodyEncodeContext {
    fn type_index(&self, ty: InternedTyId) -> Option<u32>;
    fn definition_index(&self, definition: GlobalDefId) -> Option<u32>;
    fn module_index(&self, module: ModuleId) -> Option<u32>;
}

/// Session-local relocation identities discovered by walking a checked body.
/// The encoder invokes every relocation callback, so this pass covers fields,
/// callees, nested places, closure owners, and identities that are not exposed
/// by the lighter backend reference summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateBodyRelocations {
    pub types: Vec<InternedTyId>,
    pub definitions: Vec<GlobalDefId>,
    pub modules: Vec<ModuleId>,
}

pub(crate) fn collect_checked_function_body_relocations(
    body: &FunctionBody,
) -> Result<TemplateBodyRelocations, TemplateBodyCodecError> {
    let context = CollectContext::default();
    let _ = encode_checked_function_body(body, &context)?;
    Ok(TemplateBodyRelocations {
        types: context.types.into_inner(),
        definitions: context.definitions.into_inner(),
        modules: context.modules.into_inner(),
    })
}

pub(crate) fn collect_checked_closure_entry_relocations(
    entries: &[FunctionClosureEntry],
) -> Result<TemplateBodyRelocations, TemplateBodyCodecError> {
    let context = CollectContext::default();
    let _ = encode_checked_closure_entries(entries, &context)?;
    Ok(TemplateBodyRelocations {
        types: context.types.into_inner(),
        definitions: context.definitions.into_inner(),
        modules: context.modules.into_inner(),
    })
}

#[derive(Default)]
struct CollectContext {
    types: RefCell<Vec<InternedTyId>>,
    definitions: RefCell<Vec<GlobalDefId>>,
    modules: RefCell<Vec<ModuleId>>,
}

impl TemplateBodyEncodeContext for CollectContext {
    fn type_index(&self, ty: InternedTyId) -> Option<u32> {
        if let Some(index) = self
            .types
            .borrow()
            .iter()
            .position(|candidate| *candidate == ty)
        {
            return Some(index as u32);
        }
        let mut values = self.types.borrow_mut();
        let index = values.len();
        values.push(ty);
        Some(index as u32)
    }

    fn definition_index(&self, definition: GlobalDefId) -> Option<u32> {
        if let Some(index) = self
            .definitions
            .borrow()
            .iter()
            .position(|candidate| *candidate == definition)
        {
            return Some(index as u32);
        }
        let mut values = self.definitions.borrow_mut();
        let index = values.len();
        values.push(definition);
        Some(index as u32)
    }

    fn module_index(&self, module: ModuleId) -> Option<u32> {
        if let Some(index) = self
            .modules
            .borrow()
            .iter()
            .position(|candidate| *candidate == module)
        {
            return Some(index as u32);
        }
        let mut values = self.modules.borrow_mut();
        let index = values.len();
        values.push(module);
        Some(index as u32)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateBodyCodecError(String);

impl TemplateBodyCodecError {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for TemplateBodyCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TemplateBodyCodecError {}

pub(crate) fn encode_checked_function_body(
    body: &FunctionBody,
    context: &dyn TemplateBodyEncodeContext,
) -> Result<Vec<u8>, TemplateBodyCodecError> {
    nia_function_ir::validate_function_body(body)
        .map_err(|error| TemplateBodyCodecError::invalid(error.message))?;
    let mut encoder = Encoder {
        bytes: Vec::new(),
        context,
    };
    encoder.bytes.extend_from_slice(MAGIC);
    encoder.u32(SCHEMA);
    encoder.body(body, 0)?;
    if encoder.bytes.len() > MAX_BYTES {
        return Err(TemplateBodyCodecError::invalid(
            "checked template body exceeds package size limit",
        ));
    }
    Ok(encoder.bytes)
}

pub(crate) fn encode_checked_closure_entries(
    entries: &[FunctionClosureEntry],
    context: &dyn TemplateBodyEncodeContext,
) -> Result<Vec<u8>, TemplateBodyCodecError> {
    for entry in entries {
        nia_function_ir::validate_function_closure_entry(entry)
            .map_err(|error| TemplateBodyCodecError::invalid(error.message))?;
    }
    let mut encoder = Encoder {
        bytes: Vec::new(),
        context,
    };
    encoder.len(entries.len())?;
    for entry in entries {
        encoder.closure(entry.closure_id)?;
        encoder.ty(entry.state_ty)?;
        encoder.u32(entry.state_param.0);
        encoder.len(entry.params.len())?;
        for param in &entry.params {
            encoder.u32(param.0);
        }
        encoder.ty(entry.return_type)?;
        encoder.body(&entry.body, 0)?;
    }
    if encoder.bytes.len() > MAX_BYTES {
        return Err(TemplateBodyCodecError::invalid(
            "checked closure entries exceed package size limit",
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

    fn len(&mut self, value: usize) -> Result<(), TemplateBodyCodecError> {
        if value > MAX_ITEMS {
            return Err(TemplateBodyCodecError::invalid(
                "checked template collection exceeds item limit",
            ));
        }
        self.u32(u32::try_from(value).map_err(|_| {
            TemplateBodyCodecError::invalid("checked template collection length overflows u32")
        })?);
        Ok(())
    }

    fn string(&mut self, value: &str) -> Result<(), TemplateBodyCodecError> {
        self.len(value.len())?;
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn span(&mut self, span: Span) -> Result<(), TemplateBodyCodecError> {
        self.u64(u64::try_from(span.start).map_err(|_| {
            TemplateBodyCodecError::invalid("checked template span start overflows u64")
        })?);
        self.u64(u64::try_from(span.end).map_err(|_| {
            TemplateBodyCodecError::invalid("checked template span end overflows u64")
        })?);
        Ok(())
    }

    fn ty(&mut self, ty: InternedTyId) -> Result<(), TemplateBodyCodecError> {
        let index = self.context.type_index(ty).ok_or_else(|| {
            TemplateBodyCodecError::invalid("checked template type has no stable relocation")
        })?;
        self.u32(index);
        Ok(())
    }

    fn definition(&mut self, definition: GlobalDefId) -> Result<(), TemplateBodyCodecError> {
        let index = self.context.definition_index(definition).ok_or_else(|| {
            TemplateBodyCodecError::invalid("checked template definition has no stable relocation")
        })?;
        self.u32(index);
        Ok(())
    }

    fn module(&mut self, module: ModuleId) -> Result<(), TemplateBodyCodecError> {
        let index = self.context.module_index(module).ok_or_else(|| {
            TemplateBodyCodecError::invalid("checked template module has no stable relocation")
        })?;
        self.u32(index);
        Ok(())
    }

    fn body(&mut self, body: &FunctionBody, depth: usize) -> Result<(), TemplateBodyCodecError> {
        check_depth(depth)?;
        self.span(body.span)?;
        self.ty(body.ty)?;
        self.len(body.locals.len())?;
        for local in &body.locals {
            self.local(local)?;
        }
        self.len(body.scopes.len())?;
        for scope in &body.scopes {
            self.scope(scope)?;
        }
        self.len(body.blocks.len())?;
        for block in &body.blocks {
            self.block(block, depth + 1)?;
        }
        self.u32(body.entry.0);
        Ok(())
    }

    fn local(&mut self, local: &FunctionLocal) -> Result<(), TemplateBodyCodecError> {
        self.u32(local.id.0);
        self.local_name(local.name);
        self.u8(match local.kind {
            FunctionLocalKind::Param => 0,
            FunctionLocalKind::MutableBinding => 1,
            FunctionLocalKind::ImmutableBinding => 2,
        });
        self.ty(local.ty)?;
        self.span(local.span)
    }

    fn local_name(&mut self, name: LocalName) {
        match name {
            LocalName::SelfValue => self.u8(0),
            LocalName::Named(name) => {
                self.u8(1);
                self.u64(name.raw());
            }
            LocalName::Generated(name) => {
                self.u8(2);
                self.u8(match name {
                    GeneratedLocalName::ForIterable => 0,
                    GeneratedLocalName::ForIterator => 1,
                    GeneratedLocalName::ForNext => 2,
                });
            }
            LocalName::Temporary(id) => {
                self.u8(3);
                self.u32(id);
            }
            LocalName::Anonymous => self.u8(4),
        }
    }

    fn scope(&mut self, scope: &FunctionScope) -> Result<(), TemplateBodyCodecError> {
        self.u32(scope.id.0);
        self.bool(scope.parent.is_some());
        if let Some(parent) = scope.parent {
            self.u32(parent.0);
        }
        self.span(scope.span)
    }

    fn block(&mut self, block: &FunctionBlock, depth: usize) -> Result<(), TemplateBodyCodecError> {
        check_depth(depth)?;
        self.u32(block.id.0);
        self.u32(block.scope.0);
        self.span(block.span)?;
        self.len(block.ops.len())?;
        for op in &block.ops {
            self.op(op, depth + 1)?;
        }
        self.terminator(&block.terminator, depth + 1)
    }

    fn op(&mut self, op: &FunctionOp, depth: usize) -> Result<(), TemplateBodyCodecError> {
        check_depth(depth)?;
        match op {
            FunctionOp::Binding(binding) => {
                self.u8(0);
                self.u32(binding.local_id.0);
                self.local_name(binding.name);
                self.ty(binding.ty)?;
                self.option_expr(binding.value.as_ref(), depth + 1)?;
                self.bool(binding.is_let);
            }
            FunctionOp::StoreLocal {
                local_id,
                value,
                span,
            } => {
                self.u8(1);
                self.u32(local_id.0);
                self.expr(value, depth + 1)?;
                self.span(*span)?;
            }
            FunctionOp::MemoryIntrinsic(memory) => {
                self.u8(2);
                self.memory_intrinsic(memory, depth + 1)?;
            }
            FunctionOp::Expr(expr) => {
                self.u8(3);
                self.expr(expr, depth + 1)?;
            }
            FunctionOp::Defer(body) => {
                self.u8(4);
                self.defer_body(body, depth + 1)?;
            }
        }
        Ok(())
    }

    fn defer_body(
        &mut self,
        body: &FunctionDeferBody,
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        check_depth(depth)?;
        self.span(body.span)?;
        self.len(body.scopes.len())?;
        for scope in &body.scopes {
            self.scope(scope)?;
        }
        self.len(body.blocks.len())?;
        for block in &body.blocks {
            self.block(block, depth + 1)?;
        }
        self.u32(body.entry.0);
        Ok(())
    }

    fn memory_intrinsic(
        &mut self,
        memory: &FunctionMemoryIntrinsic,
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        self.span(memory.span)?;
        self.u8(match memory.op {
            FunctionMemoryIntrinsicOp::Copy => 0,
            FunctionMemoryIntrinsicOp::Move => 1,
            FunctionMemoryIntrinsicOp::Set => 2,
        });
        self.ty(memory.elem_ty)?;
        self.expr(&memory.dest, depth + 1)?;
        match &memory.source {
            FunctionMemoryIntrinsicSource::Slice(source) => {
                self.u8(0);
                self.expr(source, depth + 1)?;
            }
            FunctionMemoryIntrinsicSource::Byte(value) => {
                self.u8(1);
                self.expr(value, depth + 1)?;
            }
        }
        Ok(())
    }

    fn terminator(
        &mut self,
        terminator: &FunctionTerminator,
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        check_depth(depth)?;
        match terminator {
            FunctionTerminator::Error { span } => {
                self.u8(0);
                self.span(*span)?;
            }
            FunctionTerminator::Branch { target, span } => {
                self.u8(1);
                self.u32(target.0);
                self.span(*span)?;
            }
            FunctionTerminator::Next { target, span } => {
                self.u8(2);
                self.u32(target.0);
                self.span(*span)?;
            }
            FunctionTerminator::If {
                cond,
                then_target,
                else_target,
                span,
            } => {
                self.u8(3);
                self.expr(cond, depth + 1)?;
                self.u32(then_target.0);
                self.u32(else_target.0);
                self.span(*span)?;
            }
            FunctionTerminator::Switch {
                target,
                arms,
                default,
                fallback,
                span,
            } => {
                self.u8(4);
                self.expr(target, depth + 1)?;
                self.len(arms.len())?;
                for arm in arms {
                    self.expr(&arm.pattern, depth + 1)?;
                    self.u32(arm.target.0);
                }
                self.bool(default.is_some());
                if let Some(default) = default {
                    self.u32(default.0);
                }
                self.u32(fallback.0);
                self.span(*span)?;
            }
            FunctionTerminator::Try {
                value,
                kind,
                error_conversion,
                success_local,
                success_target,
                span,
            } => {
                self.u8(5);
                self.expr(value, depth + 1)?;
                self.u8(match kind {
                    FunctionTryKind::Optional => 0,
                    FunctionTryKind::ErrorUnion => 1,
                });
                self.option_expr(error_conversion.as_deref(), depth + 1)?;
                self.u32(success_local.0);
                self.u32(success_target.0);
                self.span(*span)?;
            }
            FunctionTerminator::Loop {
                header,
                body,
                continue_target,
                break_target,
                span,
            } => {
                self.u8(6);
                match header {
                    FunctionForHeader::Infinite => self.u8(0),
                    FunctionForHeader::Condition(condition) => {
                        self.u8(1);
                        self.expr(condition, depth + 1)?;
                    }
                }
                self.u32(body.0);
                self.u32(continue_target.0);
                self.u32(break_target.0);
                self.span(*span)?;
            }
            FunctionTerminator::Return { value, span } => {
                self.u8(7);
                self.option_expr(value.as_ref(), depth + 1)?;
                self.span(*span)?;
            }
            FunctionTerminator::Tail { value, span } => {
                self.u8(8);
                self.option_expr(value.as_ref(), depth + 1)?;
                self.span(*span)?;
            }
        }
        Ok(())
    }

    fn option_expr(
        &mut self,
        expr: Option<&FunctionExpr>,
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        self.bool(expr.is_some());
        if let Some(expr) = expr {
            self.expr(expr, depth)?;
        }
        Ok(())
    }

    fn expr(&mut self, expr: &FunctionExpr, depth: usize) -> Result<(), TemplateBodyCodecError> {
        check_depth(depth)?;
        self.span(expr.span)?;
        self.ty(expr.ty)?;
        self.expr_kind(&expr.kind, depth + 1)
    }

    fn expr_kind(
        &mut self,
        kind: &FunctionExprKind,
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        check_depth(depth)?;
        match kind {
            FunctionExprKind::Error => self.u8(0),
            FunctionExprKind::Integer(value) => {
                self.u8(1);
                self.string(value)?;
            }
            FunctionExprKind::Float(value) => {
                self.u8(2);
                self.string(value)?;
            }
            FunctionExprKind::String(value) => {
                self.u8(3);
                self.len(value.len())?;
                for value in value {
                    self.u32(*value);
                }
            }
            FunctionExprKind::ByteString(value) => {
                self.u8(4);
                self.len(value.len())?;
                self.bytes.extend_from_slice(value);
            }
            FunctionExprKind::Char(value) => {
                self.u8(5);
                self.u32(*value);
            }
            FunctionExprKind::ByteChar(value) => {
                self.u8(6);
                self.string(value)?;
            }
            FunctionExprKind::Bool(value) => {
                self.u8(7);
                self.bool(*value);
            }
            FunctionExprKind::Null => self.u8(8),
            FunctionExprKind::Local(local) => {
                self.u8(9);
                self.u32(local.0);
            }
            FunctionExprKind::Global(definition) => {
                self.u8(10);
                self.definition(*definition)?;
            }
            FunctionExprKind::ConstGeneric(argument) => {
                self.u8(11);
                self.const_argument(argument)?;
            }
            FunctionExprKind::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => {
                self.u8(12);
                self.definition(*def_id)?;
                self.module(*arg_module_id)?;
                self.types(args)?;
                self.const_arguments(const_args)?;
            }
            FunctionExprKind::Function(definition) => {
                self.u8(13);
                self.definition(*definition)?;
            }
            FunctionExprKind::EnumConstructor(definition) => {
                self.u8(14);
                self.definition(*definition)?;
            }
            FunctionExprKind::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => {
                self.u8(15);
                self.definition(*def_id)?;
                self.module(*arg_module_id)?;
                self.option_ty(*self_arg)?;
                self.types(args)?;
                self.const_arguments(const_args)?;
            }
            FunctionExprKind::EnumVariant { variant, fields } => {
                self.u8(16);
                self.definition(*variant)?;
                self.exprs(fields, depth + 1)?;
            }
            FunctionExprKind::EnumVariantTag(variant) => {
                self.u8(17);
                self.definition(*variant)?;
            }
            FunctionExprKind::EnumTag { value } => {
                self.u8(18);
                self.expr(value, depth + 1)?;
            }
            FunctionExprKind::EnumPayloadField {
                value,
                variant,
                field,
            } => {
                self.u8(19);
                self.expr(value, depth + 1)?;
                self.definition(*variant)?;
                self.index(*field)?;
            }
            FunctionExprKind::BuiltinValue(value) => {
                self.u8(20);
                self.builtin_value(value)?;
            }
            FunctionExprKind::CallerLocation(location) => {
                self.u8(21);
                self.source_location(location)?;
            }
            FunctionExprKind::Trap => self.u8(22),
            FunctionExprKind::Range(range) => {
                self.u8(23);
                self.option_expr(range.start.as_deref(), depth + 1)?;
                self.option_expr(range.end.as_deref(), depth + 1)?;
                self.bool(range.inclusive);
            }
            FunctionExprKind::RangeBound { range, bound } => {
                self.u8(24);
                self.expr(range, depth + 1)?;
                self.u8(match bound {
                    FunctionRangeBound::Start => 0,
                    FunctionRangeBound::End => 1,
                });
            }
            FunctionExprKind::InlineAsm(asm) => {
                self.u8(25);
                self.inline_asm(asm, depth + 1)?;
            }
            FunctionExprKind::Atomic(atomic) => {
                self.u8(26);
                self.atomic(atomic, depth + 1)?;
            }
            FunctionExprKind::LoadUnaligned { ty, ptr } => {
                self.u8(27);
                self.ty(*ty)?;
                self.expr(ptr, depth + 1)?;
            }
            FunctionExprKind::Splat { value } => {
                self.u8(28);
                self.expr(value, depth + 1)?;
            }
            FunctionExprKind::ExtractElement { vector, index } => {
                self.u8(29);
                self.expr(vector, depth + 1)?;
                self.expr(index, depth + 1)?;
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.u8(30);
                self.expr(vector, depth + 1)?;
                self.expr(index, depth + 1)?;
                self.expr(value, depth + 1)?;
            }
            FunctionExprKind::Bitmask { vector } => {
                self.u8(31);
                self.expr(vector, depth + 1)?;
            }
            FunctionExprKind::BitIntrinsic { op, value } => {
                self.u8(32);
                self.u8(match op {
                    FunctionBitIntrinsicOp::Ctz => 0,
                    FunctionBitIntrinsicOp::Clz => 1,
                    FunctionBitIntrinsicOp::Popcount => 2,
                });
                self.expr(value, depth + 1)?;
            }
            FunctionExprKind::CharFromU32 { value } => {
                self.u8(33);
                self.expr(value, depth + 1)?;
            }
            FunctionExprKind::StaticArrayPointer {
                allocation,
                array,
                is_readonly,
            } => {
                self.u8(34);
                self.module(allocation.module_id())?;
                self.span(allocation.span())?;
                self.expr(array, depth + 1)?;
                self.bool(*is_readonly);
            }
            FunctionExprKind::ArrayLiteral { elems } => {
                self.u8(35);
                match elems {
                    FunctionArrayElements::List(values) => {
                        self.u8(0);
                        self.exprs(values, depth + 1)?;
                    }
                    FunctionArrayElements::Repeat { value, count } => {
                        self.u8(1);
                        self.expr(value, depth + 1)?;
                        self.array_len(count)?;
                    }
                }
            }
            FunctionExprKind::Tuple(values) => {
                self.u8(36);
                self.exprs(values, depth + 1)?;
            }
            FunctionExprKind::TupleField { value, index } => {
                self.u8(37);
                self.expr(value, depth + 1)?;
                self.index(*index)?;
            }
            FunctionExprKind::StructLiteral { def_id, fields } => {
                self.u8(38);
                self.definition(*def_id)?;
                self.fields(fields, depth + 1)?;
            }
            FunctionExprKind::UnionLiteral { def_id, field } => {
                self.u8(39);
                self.definition(*def_id)?;
                self.field(field, depth + 1)?;
            }
            FunctionExprKind::UnionStorageLiteral { bytes, relocations } => {
                self.u8(40);
                self.len(bytes.len())?;
                for byte in bytes {
                    match byte {
                        Some(byte) => {
                            self.u8(1);
                            self.u8(*byte);
                        }
                        None => self.u8(0),
                    }
                }
                self.len(relocations.len())?;
                for relocation in relocations {
                    self.index(relocation.offset)?;
                    self.index(relocation.width)?;
                    self.module(relocation.allocation.module_id())?;
                    self.span(relocation.allocation.span())?;
                    self.expr(&relocation.pointee, depth + 1)?;
                }
            }
            FunctionExprKind::Unary { op, expr } => {
                self.u8(41);
                self.u8(unary_tag(*op));
                self.expr(expr, depth + 1)?;
            }
            FunctionExprKind::OptionalSome { expr } => {
                self.u8(42);
                self.expr(expr, depth + 1)?;
            }
            FunctionExprKind::ErrorOk { expr } => {
                self.u8(43);
                self.expr(expr, depth + 1)?;
            }
            FunctionExprKind::ErrorErr { expr } => {
                self.u8(44);
                self.expr(expr, depth + 1)?;
            }
            FunctionExprKind::TaggedUnionTag { expr } => {
                self.u8(45);
                self.expr(expr, depth + 1)?;
            }
            FunctionExprKind::TaggedUnionPayload { expr } => {
                self.u8(46);
                self.expr(expr, depth + 1)?;
            }
            FunctionExprKind::Try { expr } => {
                self.u8(47);
                self.expr(expr, depth + 1)?;
            }
            FunctionExprKind::AddrOf(place) => {
                self.u8(48);
                self.place(place, depth + 1)?;
            }
            FunctionExprKind::Binary { lhs, op, rhs } => {
                self.u8(49);
                self.expr(lhs, depth + 1)?;
                self.u8(binary_tag(*op));
                self.expr(rhs, depth + 1)?;
            }
            FunctionExprKind::Assign { place, op, rhs } => {
                self.u8(50);
                self.place(place, depth + 1)?;
                self.u8(assign_tag(*op));
                self.expr(rhs, depth + 1)?;
            }
            FunctionExprKind::Discard(expr) => {
                self.u8(51);
                self.expr(expr, depth + 1)?;
            }
            FunctionExprKind::Cast { expr, ty } => {
                self.u8(52);
                self.expr(expr, depth + 1)?;
                self.ty(*ty)?;
            }
            FunctionExprKind::TraitObjectUpcast {
                expr,
                source_ty,
                target_ty,
            } => {
                self.u8(53);
                self.expr(expr, depth + 1)?;
                self.ty(*source_ty)?;
                self.ty(*target_ty)?;
            }
            FunctionExprKind::TraitObjectCoercion {
                expr,
                target_ty,
                self_ty,
            } => {
                self.u8(54);
                self.expr(expr, depth + 1)?;
                self.ty(*target_ty)?;
                self.ty(*self_ty)?;
            }
            FunctionExprKind::CallableCoercion { state, closure_id } => {
                self.u8(55);
                self.expr(state, depth + 1)?;
                self.closure(*closure_id)?;
            }
            FunctionExprKind::FunctionCallable { function } => {
                self.u8(56);
                self.expr(function, depth + 1)?;
            }
            FunctionExprKind::ClosureFunctionPointer { closure_id } => {
                self.u8(57);
                self.closure(*closure_id)?;
            }
            FunctionExprKind::Call { callee, args } => {
                self.u8(58);
                self.callee(callee, depth + 1)?;
                self.exprs(args, depth + 1)?;
            }
            FunctionExprKind::Field { lhs, field } => {
                self.u8(59);
                self.expr(lhs, depth + 1)?;
                self.definition(*field)?;
            }
            FunctionExprKind::Index { lhs, index } => {
                self.u8(60);
                self.expr(lhs, depth + 1)?;
                self.expr(index, depth + 1)?;
            }
            FunctionExprKind::Slice {
                lhs,
                range,
                is_readonly,
            } => {
                self.u8(61);
                self.expr(lhs, depth + 1)?;
                self.option_expr(range.start.as_deref(), depth + 1)?;
                self.option_expr(range.end.as_deref(), depth + 1)?;
                self.bool(range.inclusive);
                self.bool(*is_readonly);
            }
        }
        Ok(())
    }

    fn index(&mut self, value: usize) -> Result<(), TemplateBodyCodecError> {
        self.u32(u32::try_from(value).map_err(|_| {
            TemplateBodyCodecError::invalid("checked template index overflows u32")
        })?);
        Ok(())
    }

    fn types(&mut self, values: &[InternedTyId]) -> Result<(), TemplateBodyCodecError> {
        self.len(values.len())?;
        for value in values {
            self.ty(*value)?;
        }
        Ok(())
    }

    fn option_ty(&mut self, value: Option<InternedTyId>) -> Result<(), TemplateBodyCodecError> {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.ty(value)?;
        }
        Ok(())
    }

    fn exprs(
        &mut self,
        values: &[FunctionExpr],
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        self.len(values.len())?;
        for value in values {
            self.expr(value, depth)?;
        }
        Ok(())
    }

    fn const_arguments(
        &mut self,
        values: &[ConstGenericArg],
    ) -> Result<(), TemplateBodyCodecError> {
        self.len(values.len())?;
        for value in values {
            self.const_argument(value)?;
        }
        Ok(())
    }

    fn const_argument(&mut self, value: &ConstGenericArg) -> Result<(), TemplateBodyCodecError> {
        self.ty(value.ty)?;
        match value.value {
            ConstGenericValue::GenericParam(name) => {
                self.u8(0);
                self.u64(name.raw());
            }
            ConstGenericValue::ConstExpr(_) => {
                return Err(TemplateBodyCodecError::invalid(
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

    fn array_len(&mut self, value: &ArrayLenTy) -> Result<(), TemplateBodyCodecError> {
        match value {
            ArrayLenTy::Infer | ArrayLenTy::ConstExpr(_) => Err(TemplateBodyCodecError::invalid(
                "unresolved array length cannot cross a package boundary",
            )),
            ArrayLenTy::GenericParam(name) => {
                self.u8(0);
                self.u64(name.raw());
                Ok(())
            }
            ArrayLenTy::ConstValue(value) => {
                self.u8(1);
                self.u64(*value);
                Ok(())
            }
            ArrayLenTy::Builtin { builtin, ty } => {
                self.u8(2);
                self.u32(builtin.stable_tag());
                self.ty(*ty)
            }
        }
    }

    fn field(
        &mut self,
        field: &FunctionFieldInit,
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        self.bool(field.field.is_some());
        if let Some(field) = field.field {
            self.definition(field)?;
        }
        self.string(&field.name)?;
        self.expr(&field.value, depth)?;
        self.span(field.span)
    }

    fn fields(
        &mut self,
        fields: &[FunctionFieldInit],
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        self.len(fields.len())?;
        for field in fields {
            self.field(field, depth)?;
        }
        Ok(())
    }

    fn source_location(&mut self, location: &SourceLocation) -> Result<(), TemplateBodyCodecError> {
        self.string(&location.file)?;
        self.u32(location.line);
        self.u32(location.column);
        Ok(())
    }

    fn closure(&mut self, closure: ClosureId) -> Result<(), TemplateBodyCodecError> {
        self.definition(closure.owner)?;
        self.u32(closure.ordinal);
        Ok(())
    }

    fn builtin_value(
        &mut self,
        value: &FunctionBuiltinValue,
    ) -> Result<(), TemplateBodyCodecError> {
        match value {
            FunctionBuiltinValue::Usize(value) => {
                self.u8(0);
                self.u64(*value);
            }
            FunctionBuiltinValue::Layout { builtin, ty } => {
                self.u8(1);
                self.u32(builtin.stable_tag());
                self.ty(*ty)?;
            }
            FunctionBuiltinValue::FieldOffset { ty, field } => {
                self.u8(2);
                self.ty(*ty)?;
                self.definition(*field)?;
            }
            FunctionBuiltinValue::Int(value) => {
                self.u8(3);
                self.u128(value.bits());
                self.bool(value.is_signed());
            }
        }
        Ok(())
    }

    fn inline_asm(
        &mut self,
        asm: &FunctionInlineAsm,
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        self.string(&asm.code)?;
        self.len(asm.inputs.len())?;
        for input in &asm.inputs {
            self.string(&input.constraint)?;
            self.expr(&input.value, depth)?;
            self.span(input.span)?;
        }
        self.len(asm.outputs.len())?;
        for output in &asm.outputs {
            self.string(&output.constraint)?;
            self.place(&output.place, depth)?;
            self.span(output.span)?;
        }
        self.len(asm.clobbers.len())?;
        for clobber in &asm.clobbers {
            self.string(clobber)?;
        }
        self.len(asm.options.len())?;
        for option in &asm.options {
            self.u8(match option {
                FunctionAsmOption::Volatile => 0,
            });
        }
        Ok(())
    }

    fn atomic(
        &mut self,
        atomic: &FunctionAtomic,
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        match atomic {
            FunctionAtomic::Load { ty, ptr, order } => {
                self.u8(0);
                self.ty(*ty)?;
                self.expr(ptr, depth)?;
                self.u8(atomic_order_tag(*order));
            }
            FunctionAtomic::Store {
                ty,
                ptr,
                value,
                order,
            } => {
                self.u8(1);
                self.ty(*ty)?;
                self.expr(ptr, depth)?;
                self.expr(value, depth)?;
                self.u8(atomic_order_tag(*order));
            }
            FunctionAtomic::Rmw {
                ty,
                ptr,
                op,
                value,
                order,
            } => {
                self.u8(2);
                self.ty(*ty)?;
                self.expr(ptr, depth)?;
                self.u8(atomic_rmw_tag(*op));
                self.expr(value, depth)?;
                self.u8(atomic_order_tag(*order));
            }
            FunctionAtomic::Cmpxchg {
                ty,
                ptr,
                expected,
                desired,
                success,
                failure,
                weak,
            } => {
                self.u8(3);
                self.ty(*ty)?;
                self.expr(ptr, depth)?;
                self.expr(expected, depth)?;
                self.expr(desired, depth)?;
                self.u8(atomic_order_tag(*success));
                self.u8(atomic_order_tag(*failure));
                self.bool(*weak);
            }
            FunctionAtomic::Fence { order } => {
                self.u8(4);
                self.u8(atomic_order_tag(*order));
            }
        }
        Ok(())
    }

    fn place(&mut self, place: &FunctionPlace, depth: usize) -> Result<(), TemplateBodyCodecError> {
        check_depth(depth)?;
        self.span(place.span)?;
        self.ty(place.ty)?;
        match &place.base {
            FunctionPlaceBase::Local(local) => {
                self.u8(0);
                self.u32(local.0);
            }
            FunctionPlaceBase::Global(definition) => {
                self.u8(1);
                self.definition(*definition)?;
            }
            FunctionPlaceBase::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => {
                self.u8(2);
                self.definition(*def_id)?;
                self.module(*arg_module_id)?;
                self.types(args)?;
                self.const_arguments(const_args)?;
            }
            FunctionPlaceBase::Deref(expr) => {
                self.u8(3);
                self.expr(expr, depth + 1)?;
            }
            FunctionPlaceBase::Error => self.u8(4),
        }
        self.len(place.elems.len())?;
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Field(field) => {
                    self.u8(0);
                    self.definition(*field)?;
                }
                FunctionPlaceElem::TupleField(index) => {
                    self.u8(1);
                    self.index(*index)?;
                }
                FunctionPlaceElem::Index(index) => {
                    self.u8(2);
                    self.expr(index, depth + 1)?;
                }
                FunctionPlaceElem::Error => self.u8(3),
            }
        }
        Ok(())
    }

    fn callee(
        &mut self,
        callee: &FunctionCallee,
        depth: usize,
    ) -> Result<(), TemplateBodyCodecError> {
        check_depth(depth)?;
        match callee {
            FunctionCallee::Tracked { callee, location } => {
                self.u8(0);
                self.callee(callee, depth + 1)?;
                self.source_location(location)?;
            }
            FunctionCallee::ClosureEntry { closure_id, state } => {
                self.u8(1);
                self.closure(*closure_id)?;
                self.expr(state, depth + 1)?;
            }
            FunctionCallee::Function(definition) => {
                self.u8(2);
                self.definition(*definition)?;
            }
            FunctionCallee::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => {
                self.u8(3);
                self.instance_ref(*def_id, *arg_module_id, *self_arg, args, const_args)?;
            }
            FunctionCallee::Method {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
                receiver_kind,
                receiver,
            } => {
                self.u8(4);
                self.instance_ref(*def_id, *arg_module_id, *self_arg, args, const_args)?;
                self.u32(receiver_kind.stable_tag());
                self.expr(receiver, depth + 1)?;
            }
            FunctionCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
                receiver_kind,
                receiver,
            } => {
                self.u8(5);
                self.definition(*trait_id)?;
                self.definition(*method_id)?;
                self.u64(method_name.raw());
                self.ty(*self_ty)?;
                self.types(trait_args)?;
                self.const_arguments(trait_const_args)?;
                self.types(args)?;
                self.const_arguments(const_args)?;
                self.u32(receiver_kind.stable_tag());
                self.expr(receiver, depth + 1)?;
            }
            FunctionCallee::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                trait_const_args,
                args,
                const_args,
            } => {
                self.u8(6);
                self.definition(*trait_id)?;
                self.definition(*method_id)?;
                self.u64(method_name.raw());
                self.ty(*self_ty)?;
                self.types(trait_args)?;
                self.const_arguments(trait_const_args)?;
                self.types(args)?;
                self.const_arguments(const_args)?;
            }
            FunctionCallee::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                trait_const_args,
                slot,
                params,
                return_type,
                receiver_kind,
                receiver,
            } => {
                self.u8(7);
                self.ty(*object_ty)?;
                self.trait_id(*trait_id)?;
                self.definition(*method_id)?;
                self.u64(method_name.raw());
                self.types(trait_args)?;
                self.const_arguments(trait_const_args)?;
                self.index(*slot)?;
                self.types(params)?;
                self.ty(*return_type)?;
                self.u32(receiver_kind.stable_tag());
                self.expr(receiver, depth + 1)?;
            }
            FunctionCallee::BuiltinMethod {
                method,
                self_ty,
                receiver,
            } => {
                self.u8(8);
                self.u8(builtin_method_tag(*method));
                self.ty(*self_ty)?;
                self.expr(receiver, depth + 1)?;
            }
            FunctionCallee::BuiltinTraitMethodCall {
                trait_id,
                method,
                self_ty,
                trait_args,
                receiver,
            } => {
                self.u8(9);
                self.u32(trait_id.stable_tag());
                self.u32(method.stable_tag());
                self.ty(*self_ty)?;
                self.types(trait_args)?;
                self.expr(receiver, depth + 1)?;
            }
            FunctionCallee::BuiltinOperator(operator) => {
                self.u8(10);
                self.u32(operator.trait_id.stable_tag());
                match operator.op {
                    FunctionBuiltinOperatorOp::Unary(op) => {
                        self.u8(0);
                        self.u8(unary_tag(op));
                    }
                    FunctionBuiltinOperatorOp::Binary(op) => {
                        self.u8(1);
                        self.u8(binary_tag(op));
                    }
                }
            }
            FunctionCallee::Callable(expr) => {
                self.u8(11);
                self.expr(expr, depth + 1)?;
            }
            FunctionCallee::FunctionPointer(expr) => {
                self.u8(12);
                self.expr(expr, depth + 1)?;
            }
        }
        Ok(())
    }

    fn instance_ref(
        &mut self,
        definition: GlobalDefId,
        module: ModuleId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Result<(), TemplateBodyCodecError> {
        self.definition(definition)?;
        self.module(module)?;
        self.option_ty(self_arg)?;
        self.types(args)?;
        self.const_arguments(const_args)
    }

    fn trait_id(&mut self, trait_id: TraitId) -> Result<(), TemplateBodyCodecError> {
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
}

fn check_depth(depth: usize) -> Result<(), TemplateBodyCodecError> {
    if depth > MAX_DEPTH {
        Err(TemplateBodyCodecError::invalid(
            "checked template body exceeds recursion limit",
        ))
    } else {
        Ok(())
    }
}

fn unary_tag(value: UnaryOp) -> u8 {
    match value {
        UnaryOp::Neg => 0,
        UnaryOp::Not => 1,
        UnaryOp::BitNot => 2,
        UnaryOp::RefReadOnly => 3,
        UnaryOp::Ref => 4,
        UnaryOp::Deref => 5,
    }
}

fn binary_tag(value: BinaryOp) -> u8 {
    match value {
        BinaryOp::Mul => 0,
        BinaryOp::Div => 1,
        BinaryOp::Rem => 2,
        BinaryOp::Add => 3,
        BinaryOp::Sub => 4,
        BinaryOp::Shl => 5,
        BinaryOp::Shr => 6,
        BinaryOp::Lt => 7,
        BinaryOp::Le => 8,
        BinaryOp::Gt => 9,
        BinaryOp::Ge => 10,
        BinaryOp::Eq => 11,
        BinaryOp::Ne => 12,
        BinaryOp::BitAnd => 13,
        BinaryOp::BitXor => 14,
        BinaryOp::BitOr => 15,
        BinaryOp::And => 16,
        BinaryOp::Or => 17,
    }
}

fn assign_tag(value: AssignOp) -> u8 {
    match value {
        AssignOp::Assign => 0,
        AssignOp::Add => 1,
        AssignOp::Sub => 2,
        AssignOp::Shl => 3,
        AssignOp::Shr => 4,
        AssignOp::Mul => 5,
        AssignOp::Div => 6,
        AssignOp::Rem => 7,
        AssignOp::BitAnd => 8,
        AssignOp::BitXor => 9,
        AssignOp::BitOr => 10,
    }
}

fn builtin_method_tag(value: FunctionBuiltinMethod) -> u8 {
    match value {
        FunctionBuiltinMethod::SliceLen => 0,
        FunctionBuiltinMethod::SlicePtr => 1,
        FunctionBuiltinMethod::SlicePtrMut => 2,
        FunctionBuiltinMethod::Start => 3,
        FunctionBuiltinMethod::End => 4,
        FunctionBuiltinMethod::Iter => 5,
    }
}

fn atomic_order_tag(value: AtomicOrder) -> u8 {
    match value {
        AtomicOrder::Unordered => 0,
        AtomicOrder::Monotonic => 1,
        AtomicOrder::Acquire => 2,
        AtomicOrder::Release => 3,
        AtomicOrder::AcqRel => 4,
        AtomicOrder::SeqCst => 5,
    }
}

fn atomic_rmw_tag(value: AtomicRmwOp) -> u8 {
    match value {
        AtomicRmwOp::Xchg => 0,
        AtomicRmwOp::Add => 1,
        AtomicRmwOp::Sub => 2,
        AtomicRmwOp::And => 3,
        AtomicRmwOp::Nand => 4,
        AtomicRmwOp::Or => 5,
        AtomicRmwOp::Xor => 6,
        AtomicRmwOp::Max => 7,
        AtomicRmwOp::Min => 8,
        AtomicRmwOp::UMax => 9,
        AtomicRmwOp::UMin => 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{LocalId, ModuleIdAllocator};
    use nia_ty::{PrimitiveTy, TypeStore};

    struct Context {
        types: Vec<InternedTyId>,
        definitions: Vec<GlobalDefId>,
        modules: Vec<ModuleId>,
    }

    impl TemplateBodyEncodeContext for Context {
        fn type_index(&self, ty: InternedTyId) -> Option<u32> {
            self.types
                .iter()
                .position(|candidate| *candidate == ty)
                .map(|index| index as u32)
        }

        fn definition_index(&self, definition: GlobalDefId) -> Option<u32> {
            self.definitions
                .iter()
                .position(|candidate| *candidate == definition)
                .map(|index| index as u32)
        }

        fn module_index(&self, module: ModuleId) -> Option<u32> {
            self.modules
                .iter()
                .position(|candidate| *candidate == module)
                .map(|index| index as u32)
        }
    }

    fn fixture() -> (FunctionBody, InternedTyId, InternedTyId) {
        let mut modules = ModuleIdAllocator::new();
        let module = modules.allocate();
        let first_store = TypeStore::new();
        let second_store = TypeStore::new();
        let first_ty = first_store
            .append_for_module(module)
            .primitive(PrimitiveTy::I32);
        let second_ty = second_store
            .append_for_module(module)
            .primitive(PrimitiveTy::I32);
        let span = Span::new(0, 1);
        let body = FunctionBody {
            span,
            locals: vec![FunctionLocal {
                id: LocalId(0),
                name: LocalName::Anonymous,
                kind: FunctionLocalKind::Param,
                ty: first_ty,
                span,
            }],
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: vec![FunctionOp::Expr(FunctionExpr {
                    span,
                    ty: first_ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                })],
                terminator: FunctionTerminator::Return {
                    value: Some(FunctionExpr {
                        span,
                        ty: first_ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(0),
            ty: first_ty,
        };
        (body, first_ty, second_ty)
    }

    #[test]
    fn rejects_unresolved_const_arguments() {
        let (mut body, first_ty, _) = fixture();
        body.blocks[0].ops.push(FunctionOp::Expr(FunctionExpr {
            span: Span::new(0, 1),
            ty: first_ty,
            kind: FunctionExprKind::ConstGeneric(ConstGenericArg {
                ty: first_ty,
                value: ConstGenericValue::ConstExpr(nia_ids::GlobalConstExprId {
                    module_id: ModuleIdAllocator::new().allocate(),
                    const_expr_id: nia_ids::ConstExprId(0),
                }),
            }),
        }));
        let source = Context {
            types: vec![first_ty],
            definitions: Vec::new(),
            modules: Vec::new(),
        };
        assert!(encode_checked_function_body(&body, &source).is_err());
    }

    #[test]
    fn relocation_collector_walks_and_deduplicates_all_identity_classes() {
        let (mut body, first_ty, _) = fixture();
        let mut modules = ModuleIdAllocator::new();
        let owner_module = modules.allocate();
        let argument_module = modules.allocate();
        let first_definition = GlobalDefId {
            module_id: owner_module,
            def_id: nia_ids::DefId(7),
        };
        let second_definition = GlobalDefId {
            module_id: owner_module,
            def_id: nia_ids::DefId(8),
        };
        body.blocks[0].ops.extend([
            FunctionOp::Expr(FunctionExpr {
                span: Span::new(0, 1),
                ty: first_ty,
                kind: FunctionExprKind::Global(first_definition),
            }),
            FunctionOp::Expr(FunctionExpr {
                span: Span::new(0, 1),
                ty: first_ty,
                kind: FunctionExprKind::GlobalInstance {
                    def_id: second_definition,
                    arg_module_id: argument_module,
                    args: vec![first_ty, first_ty],
                    const_args: Vec::new(),
                },
            }),
        ]);

        let relocations = collect_checked_function_body_relocations(&body).expect("collect");

        assert_eq!(relocations.types, vec![first_ty]);
        assert_eq!(
            relocations.definitions,
            vec![first_definition, second_definition]
        );
        assert_eq!(relocations.modules, vec![argument_module]);
    }
}
