// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical compiler-owned codec for checked function templates.
//!
//! Package metadata owns the outer template inventory and stable relocation
//! tables. This module owns the executable Function IR schema. Every session
//! identity crosses the wire through an explicit relocation index, and every
//! decoded body is structurally validated before it can enter the query graph.

use std::{cell::RefCell, fmt, io::Cursor};

use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_function_ir::*;
use nia_ids::{
    BuiltinTraitMethod, ClosureId, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId, ModuleId,
    ReceiverKind,
};
use nia_source::SourceLocation;
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_ty::{ArrayLenTy, BuiltinTrait, ConstGenericArg, ConstGenericValue, IntConst, TraitId};

const MAGIC: &[u8; 8] = b"NIAFIR01";
const SCHEMA: u32 = 1;
const MAX_BYTES: usize = nia_package_metadata::MAX_PACKAGE_BYTES;
const MAX_ITEMS: usize = 1_000_000;
const MAX_DEPTH: usize = 256;

pub(crate) trait TemplateBodyEncodeContext {
    fn type_index(&self, ty: InternedTyId) -> Option<u32>;
    fn definition_index(&self, definition: GlobalDefId) -> Option<u32>;
    fn module_index(&self, module: ModuleId) -> Option<u32>;
}

pub(crate) trait TemplateBodyDecodeContext {
    fn type_at(&self, index: u32) -> Option<InternedTyId>;
    fn definition_at(&self, index: u32) -> Option<GlobalDefId>;
    fn module_at(&self, index: u32) -> Option<ModuleId>;
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
    let mut context = CollectContext::default();
    let _ = encode_checked_function_body(body, &mut context)?;
    Ok(TemplateBodyRelocations {
        types: context.types.into_inner(),
        definitions: context.definitions.into_inner(),
        modules: context.modules.into_inner(),
    })
}

pub(crate) fn collect_checked_closure_entry_relocations(
    entries: &[FunctionClosureEntry],
) -> Result<TemplateBodyRelocations, TemplateBodyCodecError> {
    let mut context = CollectContext::default();
    let _ = encode_checked_closure_entries(entries, &mut context)?;
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

pub(crate) fn decode_checked_function_body(
    bytes: &[u8],
    context: &dyn TemplateBodyDecodeContext,
) -> Result<FunctionBody, TemplateBodyCodecError> {
    if bytes.len() > MAX_BYTES {
        return Err(TemplateBodyCodecError::invalid(
            "checked template body exceeds package size limit",
        ));
    }
    let mut decoder = Decoder {
        cursor: Cursor::new(bytes),
        context,
    };
    if decoder.bytes(8)? != MAGIC {
        return Err(TemplateBodyCodecError::invalid(
            "checked template body has invalid magic",
        ));
    }
    if decoder.u32()? != SCHEMA {
        return Err(TemplateBodyCodecError::invalid(
            "checked template body has incompatible schema",
        ));
    }
    let body = decoder.body(0)?;
    if decoder.cursor.position() != bytes.len() as u64 {
        return Err(TemplateBodyCodecError::invalid(
            "checked template body has trailing bytes",
        ));
    }
    nia_function_ir::validate_function_body(&body)
        .map_err(|error| TemplateBodyCodecError::invalid(error.message))?;
    Ok(body)
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

pub(crate) fn decode_checked_closure_entries(
    bytes: &[u8],
    context: &dyn TemplateBodyDecodeContext,
) -> Result<Vec<FunctionClosureEntry>, TemplateBodyCodecError> {
    if bytes.len() > MAX_BYTES {
        return Err(TemplateBodyCodecError::invalid(
            "checked closure entries exceed package size limit",
        ));
    }
    let mut decoder = Decoder {
        cursor: Cursor::new(bytes),
        context,
    };
    let count = decoder.len()?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let closure_id = decoder.closure()?;
        let state_ty = decoder.ty()?;
        let state_param = LocalId(decoder.u32()?);
        let param_count = decoder.len()?;
        let mut params = Vec::with_capacity(param_count);
        for _ in 0..param_count {
            params.push(LocalId(decoder.u32()?));
        }
        let return_type = decoder.ty()?;
        let body = decoder.body(0)?;
        let entry = FunctionClosureEntry {
            closure_id,
            state_ty,
            state_param,
            params,
            return_type,
            body,
        };
        nia_function_ir::validate_function_closure_entry(&entry)
            .map_err(|error| TemplateBodyCodecError::invalid(error.message))?;
        entries.push(entry);
    }
    if decoder.cursor.position() != bytes.len() as u64 {
        return Err(TemplateBodyCodecError::invalid(
            "checked closure entries have trailing bytes",
        ));
    }
    Ok(entries)
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

struct Decoder<'a> {
    cursor: Cursor<&'a [u8]>,
    context: &'a dyn TemplateBodyDecodeContext,
}

impl Decoder<'_> {
    fn bytes(&mut self, count: usize) -> Result<&[u8], TemplateBodyCodecError> {
        let start = usize::try_from(self.cursor.position())
            .map_err(|_| TemplateBodyCodecError::invalid("checked template offset overflow"))?;
        let end = start
            .checked_add(count)
            .filter(|end| *end <= self.cursor.get_ref().len())
            .ok_or_else(|| TemplateBodyCodecError::invalid("truncated checked template body"))?;
        self.cursor.set_position(end as u64);
        Ok(&self.cursor.get_ref()[start..end])
    }

    fn u8(&mut self) -> Result<u8, TemplateBodyCodecError> {
        Ok(self.bytes(1)?[0])
    }

    fn bool(&mut self) -> Result<bool, TemplateBodyCodecError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(TemplateBodyCodecError::invalid(
                "invalid checked template boolean",
            )),
        }
    }

    fn u32(&mut self) -> Result<u32, TemplateBodyCodecError> {
        Ok(u32::from_le_bytes(
            self.bytes(4)?.try_into().expect("exact width"),
        ))
    }

    fn u64(&mut self) -> Result<u64, TemplateBodyCodecError> {
        Ok(u64::from_le_bytes(
            self.bytes(8)?.try_into().expect("exact width"),
        ))
    }

    fn u128(&mut self) -> Result<u128, TemplateBodyCodecError> {
        Ok(u128::from_le_bytes(
            self.bytes(16)?.try_into().expect("exact width"),
        ))
    }

    fn len(&mut self) -> Result<usize, TemplateBodyCodecError> {
        let value = self.u32()? as usize;
        if value > MAX_ITEMS {
            return Err(TemplateBodyCodecError::invalid(
                "checked template collection exceeds item limit",
            ));
        }
        Ok(value)
    }

    fn string(&mut self) -> Result<String, TemplateBodyCodecError> {
        let len = self.len()?;
        String::from_utf8(self.bytes(len)?.to_vec())
            .map_err(|_| TemplateBodyCodecError::invalid("checked template string is not UTF-8"))
    }

    fn span(&mut self) -> Result<Span, TemplateBodyCodecError> {
        let start = usize::try_from(self.u64()?)
            .map_err(|_| TemplateBodyCodecError::invalid("checked template span overflow"))?;
        let end = usize::try_from(self.u64()?)
            .map_err(|_| TemplateBodyCodecError::invalid("checked template span overflow"))?;
        if start > end {
            return Err(TemplateBodyCodecError::invalid(
                "checked template span is reversed",
            ));
        }
        Ok(Span::new(start, end))
    }

    fn ty(&mut self) -> Result<InternedTyId, TemplateBodyCodecError> {
        let index = self.u32()?;
        self.context.type_at(index).ok_or_else(|| {
            TemplateBodyCodecError::invalid("checked template type relocation is out of range")
        })
    }

    fn definition(&mut self) -> Result<GlobalDefId, TemplateBodyCodecError> {
        let index = self.u32()?;
        self.context.definition_at(index).ok_or_else(|| {
            TemplateBodyCodecError::invalid(
                "checked template definition relocation is out of range",
            )
        })
    }

    fn module(&mut self) -> Result<ModuleId, TemplateBodyCodecError> {
        let index = self.u32()?;
        self.context.module_at(index).ok_or_else(|| {
            TemplateBodyCodecError::invalid("checked template module relocation is out of range")
        })
    }

    fn body(&mut self, depth: usize) -> Result<FunctionBody, TemplateBodyCodecError> {
        check_depth(depth)?;
        let span = self.span()?;
        let ty = self.ty()?;
        let local_count = self.len()?;
        let mut locals = Vec::with_capacity(local_count);
        for _ in 0..local_count {
            locals.push(self.local()?);
        }
        let scope_count = self.len()?;
        let mut scopes = Vec::with_capacity(scope_count);
        for _ in 0..scope_count {
            scopes.push(self.scope()?);
        }
        let block_count = self.len()?;
        let mut blocks = Vec::with_capacity(block_count);
        for _ in 0..block_count {
            blocks.push(self.block(depth + 1)?);
        }
        Ok(FunctionBody {
            span,
            locals,
            scopes,
            blocks,
            entry: FunctionBlockId(self.u32()?),
            ty,
        })
    }

    fn local(&mut self) -> Result<FunctionLocal, TemplateBodyCodecError> {
        let id = LocalId(self.u32()?);
        let name = self.local_name()?;
        let kind = match self.u8()? {
            0 => FunctionLocalKind::Param,
            1 => FunctionLocalKind::MutableBinding,
            2 => FunctionLocalKind::ImmutableBinding,
            _ => return Err(TemplateBodyCodecError::invalid("invalid local kind")),
        };
        Ok(FunctionLocal {
            id,
            name,
            kind,
            ty: self.ty()?,
            span: self.span()?,
        })
    }

    fn local_name(&mut self) -> Result<LocalName, TemplateBodyCodecError> {
        match self.u8()? {
            0 => Ok(LocalName::SelfValue),
            1 => Ok(LocalName::Named(SymbolId::from_stable_hash(self.u64()?))),
            2 => Ok(LocalName::Generated(match self.u8()? {
                0 => GeneratedLocalName::ForIterable,
                1 => GeneratedLocalName::ForIterator,
                2 => GeneratedLocalName::ForNext,
                _ => {
                    return Err(TemplateBodyCodecError::invalid(
                        "invalid generated local name",
                    ));
                }
            })),
            3 => Ok(LocalName::Temporary(self.u32()?)),
            4 => Ok(LocalName::Anonymous),
            _ => Err(TemplateBodyCodecError::invalid("invalid local name")),
        }
    }

    fn scope(&mut self) -> Result<FunctionScope, TemplateBodyCodecError> {
        let id = FunctionScopeId(self.u32()?);
        let parent = self.bool()?.then(|| self.u32()).transpose()?;
        Ok(FunctionScope {
            id,
            parent: parent.map(FunctionScopeId),
            span: self.span()?,
        })
    }

    fn block(&mut self, depth: usize) -> Result<FunctionBlock, TemplateBodyCodecError> {
        check_depth(depth)?;
        let id = FunctionBlockId(self.u32()?);
        let scope = FunctionScopeId(self.u32()?);
        let span = self.span()?;
        let count = self.len()?;
        let mut ops = Vec::with_capacity(count);
        for _ in 0..count {
            ops.push(self.op(depth + 1)?);
        }
        Ok(FunctionBlock {
            id,
            scope,
            span,
            ops,
            terminator: self.terminator(depth + 1)?,
        })
    }

    fn op(&mut self, depth: usize) -> Result<FunctionOp, TemplateBodyCodecError> {
        check_depth(depth)?;
        match self.u8()? {
            0 => Ok(FunctionOp::Binding(FunctionBinding {
                local_id: LocalId(self.u32()?),
                name: self.local_name()?,
                ty: self.ty()?,
                value: self.option_expr(depth + 1)?,
                is_let: self.bool()?,
            })),
            1 => Ok(FunctionOp::StoreLocal {
                local_id: LocalId(self.u32()?),
                value: self.expr(depth + 1)?,
                span: self.span()?,
            }),
            2 => Ok(FunctionOp::MemoryIntrinsic(Box::new(
                self.memory_intrinsic(depth + 1)?,
            ))),
            3 => Ok(FunctionOp::Expr(self.expr(depth + 1)?)),
            4 => Ok(FunctionOp::Defer(self.defer_body(depth + 1)?)),
            _ => Err(TemplateBodyCodecError::invalid(
                "invalid checked template operation tag",
            )),
        }
    }

    fn defer_body(&mut self, depth: usize) -> Result<FunctionDeferBody, TemplateBodyCodecError> {
        check_depth(depth)?;
        let span = self.span()?;
        let scope_count = self.len()?;
        let mut scopes = Vec::with_capacity(scope_count);
        for _ in 0..scope_count {
            scopes.push(self.scope()?);
        }
        let block_count = self.len()?;
        let mut blocks = Vec::with_capacity(block_count);
        for _ in 0..block_count {
            blocks.push(self.block(depth + 1)?);
        }
        Ok(FunctionDeferBody {
            span,
            scopes,
            blocks,
            entry: FunctionBlockId(self.u32()?),
        })
    }

    fn memory_intrinsic(
        &mut self,
        depth: usize,
    ) -> Result<FunctionMemoryIntrinsic, TemplateBodyCodecError> {
        let span = self.span()?;
        let op = match self.u8()? {
            0 => FunctionMemoryIntrinsicOp::Copy,
            1 => FunctionMemoryIntrinsicOp::Move,
            2 => FunctionMemoryIntrinsicOp::Set,
            _ => return Err(TemplateBodyCodecError::invalid("invalid memory operation")),
        };
        let elem_ty = self.ty()?;
        let dest = self.expr(depth + 1)?;
        let source = match self.u8()? {
            0 => FunctionMemoryIntrinsicSource::Slice(self.expr(depth + 1)?),
            1 => FunctionMemoryIntrinsicSource::Byte(self.expr(depth + 1)?),
            _ => return Err(TemplateBodyCodecError::invalid("invalid memory source")),
        };
        Ok(FunctionMemoryIntrinsic {
            span,
            op,
            elem_ty,
            dest,
            source,
        })
    }

    fn terminator(&mut self, depth: usize) -> Result<FunctionTerminator, TemplateBodyCodecError> {
        check_depth(depth)?;
        Ok(match self.u8()? {
            0 => FunctionTerminator::Error { span: self.span()? },
            1 => FunctionTerminator::Branch {
                target: FunctionBlockId(self.u32()?),
                span: self.span()?,
            },
            2 => FunctionTerminator::Next {
                target: FunctionBlockId(self.u32()?),
                span: self.span()?,
            },
            3 => FunctionTerminator::If {
                cond: self.expr(depth + 1)?,
                then_target: FunctionBlockId(self.u32()?),
                else_target: FunctionBlockId(self.u32()?),
                span: self.span()?,
            },
            4 => {
                let target = self.expr(depth + 1)?;
                let count = self.len()?;
                let mut arms = Vec::with_capacity(count);
                for _ in 0..count {
                    arms.push(FunctionSwitchArm {
                        pattern: self.expr(depth + 1)?,
                        target: FunctionBlockId(self.u32()?),
                    });
                }
                let default = self.bool()?.then(|| self.u32()).transpose()?;
                FunctionTerminator::Switch {
                    target,
                    arms,
                    default: default.map(FunctionBlockId),
                    fallback: FunctionBlockId(self.u32()?),
                    span: self.span()?,
                }
            }
            5 => FunctionTerminator::Try {
                value: self.expr(depth + 1)?,
                kind: match self.u8()? {
                    0 => FunctionTryKind::Optional,
                    1 => FunctionTryKind::ErrorUnion,
                    _ => return Err(TemplateBodyCodecError::invalid("invalid try kind")),
                },
                error_conversion: self.option_expr(depth + 1)?.map(Box::new),
                success_local: LocalId(self.u32()?),
                success_target: FunctionBlockId(self.u32()?),
                span: self.span()?,
            },
            6 => {
                let header = match self.u8()? {
                    0 => FunctionForHeader::Infinite,
                    1 => FunctionForHeader::Condition(Box::new(self.expr(depth + 1)?)),
                    _ => return Err(TemplateBodyCodecError::invalid("invalid loop header")),
                };
                FunctionTerminator::Loop {
                    header,
                    body: FunctionBlockId(self.u32()?),
                    continue_target: FunctionBlockId(self.u32()?),
                    break_target: FunctionBlockId(self.u32()?),
                    span: self.span()?,
                }
            }
            7 => FunctionTerminator::Return {
                value: self.option_expr(depth + 1)?,
                span: self.span()?,
            },
            8 => FunctionTerminator::Tail {
                value: self.option_expr(depth + 1)?,
                span: self.span()?,
            },
            _ => {
                return Err(TemplateBodyCodecError::invalid(
                    "invalid checked template terminator tag",
                ));
            }
        })
    }

    fn option_expr(
        &mut self,
        depth: usize,
    ) -> Result<Option<FunctionExpr>, TemplateBodyCodecError> {
        self.bool()?.then(|| self.expr(depth)).transpose()
    }

    fn expr(&mut self, depth: usize) -> Result<FunctionExpr, TemplateBodyCodecError> {
        check_depth(depth)?;
        Ok(FunctionExpr {
            span: self.span()?,
            ty: self.ty()?,
            kind: self.expr_kind(depth + 1)?,
        })
    }

    fn expr_kind(&mut self, depth: usize) -> Result<FunctionExprKind, TemplateBodyCodecError> {
        check_depth(depth)?;
        Ok(match self.u8()? {
            0 => FunctionExprKind::Error,
            1 => FunctionExprKind::Integer(self.string()?),
            2 => FunctionExprKind::Float(self.string()?),
            3 => {
                let count = self.len()?;
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.u32()?);
                }
                FunctionExprKind::String(values)
            }
            4 => {
                let count = self.len()?;
                FunctionExprKind::ByteString(self.bytes(count)?.to_vec())
            }
            5 => FunctionExprKind::Char(self.u32()?),
            6 => FunctionExprKind::ByteChar(self.string()?),
            7 => FunctionExprKind::Bool(self.bool()?),
            8 => FunctionExprKind::Null,
            9 => FunctionExprKind::Local(LocalId(self.u32()?)),
            10 => FunctionExprKind::Global(self.definition()?),
            11 => FunctionExprKind::ConstGeneric(self.const_argument()?),
            12 => FunctionExprKind::GlobalInstance {
                def_id: self.definition()?,
                arg_module_id: self.module()?,
                args: self.types()?,
                const_args: self.const_arguments()?,
            },
            13 => FunctionExprKind::Function(self.definition()?),
            14 => FunctionExprKind::EnumConstructor(self.definition()?),
            15 => FunctionExprKind::FunctionInstance {
                def_id: self.definition()?,
                arg_module_id: self.module()?,
                self_arg: self.option_ty()?,
                args: self.types()?,
                const_args: self.const_arguments()?,
            },
            16 => FunctionExprKind::EnumVariant {
                variant: self.definition()?,
                fields: self.exprs(depth + 1)?,
            },
            17 => FunctionExprKind::EnumVariantTag(self.definition()?),
            18 => FunctionExprKind::EnumTag {
                value: Box::new(self.expr(depth + 1)?),
            },
            19 => FunctionExprKind::EnumPayloadField {
                value: Box::new(self.expr(depth + 1)?),
                variant: self.definition()?,
                field: self.index()?,
            },
            20 => FunctionExprKind::BuiltinValue(self.builtin_value()?),
            21 => FunctionExprKind::CallerLocation(self.source_location()?),
            22 => FunctionExprKind::Trap,
            23 => FunctionExprKind::Range(FunctionRange {
                start: self.option_expr(depth + 1)?.map(Box::new),
                end: self.option_expr(depth + 1)?.map(Box::new),
                inclusive: self.bool()?,
            }),
            24 => FunctionExprKind::RangeBound {
                range: Box::new(self.expr(depth + 1)?),
                bound: match self.u8()? {
                    0 => FunctionRangeBound::Start,
                    1 => FunctionRangeBound::End,
                    _ => return Err(TemplateBodyCodecError::invalid("invalid range bound")),
                },
            },
            25 => FunctionExprKind::InlineAsm(self.inline_asm(depth + 1)?),
            26 => FunctionExprKind::Atomic(self.atomic(depth + 1)?),
            27 => FunctionExprKind::LoadUnaligned {
                ty: self.ty()?,
                ptr: Box::new(self.expr(depth + 1)?),
            },
            28 => FunctionExprKind::Splat {
                value: Box::new(self.expr(depth + 1)?),
            },
            29 => FunctionExprKind::ExtractElement {
                vector: Box::new(self.expr(depth + 1)?),
                index: Box::new(self.expr(depth + 1)?),
            },
            30 => FunctionExprKind::InsertElement {
                vector: Box::new(self.expr(depth + 1)?),
                index: Box::new(self.expr(depth + 1)?),
                value: Box::new(self.expr(depth + 1)?),
            },
            31 => FunctionExprKind::Bitmask {
                vector: Box::new(self.expr(depth + 1)?),
            },
            32 => FunctionExprKind::BitIntrinsic {
                op: match self.u8()? {
                    0 => FunctionBitIntrinsicOp::Ctz,
                    1 => FunctionBitIntrinsicOp::Clz,
                    2 => FunctionBitIntrinsicOp::Popcount,
                    _ => {
                        return Err(TemplateBodyCodecError::invalid(
                            "invalid bit intrinsic operation",
                        ));
                    }
                },
                value: Box::new(self.expr(depth + 1)?),
            },
            33 => FunctionExprKind::CharFromU32 {
                value: Box::new(self.expr(depth + 1)?),
            },
            34 => FunctionExprKind::StaticArrayPointer {
                allocation: PromotedAllocationId::new(self.module()?, self.span()?),
                array: Box::new(self.expr(depth + 1)?),
                is_readonly: self.bool()?,
            },
            35 => FunctionExprKind::ArrayLiteral {
                elems: match self.u8()? {
                    0 => FunctionArrayElements::List(self.exprs(depth + 1)?),
                    1 => FunctionArrayElements::Repeat {
                        value: Box::new(self.expr(depth + 1)?),
                        count: self.array_len()?,
                    },
                    _ => return Err(TemplateBodyCodecError::invalid("invalid array elements")),
                },
            },
            36 => FunctionExprKind::Tuple(self.exprs(depth + 1)?),
            37 => FunctionExprKind::TupleField {
                value: Box::new(self.expr(depth + 1)?),
                index: self.index()?,
            },
            38 => FunctionExprKind::StructLiteral {
                def_id: self.definition()?,
                fields: self.fields(depth + 1)?,
            },
            39 => FunctionExprKind::UnionLiteral {
                def_id: self.definition()?,
                field: Box::new(self.field(depth + 1)?),
            },
            40 => {
                let byte_count = self.len()?;
                let mut bytes = Vec::with_capacity(byte_count);
                for _ in 0..byte_count {
                    bytes.push(match self.u8()? {
                        0 => None,
                        1 => Some(self.u8()?),
                        _ => {
                            return Err(TemplateBodyCodecError::invalid(
                                "invalid union storage byte",
                            ));
                        }
                    });
                }
                let relocation_count = self.len()?;
                let mut relocations = Vec::with_capacity(relocation_count);
                for _ in 0..relocation_count {
                    relocations.push(FunctionUnionRelocation {
                        offset: self.index()?,
                        width: self.index()?,
                        allocation: PromotedAllocationId::new(self.module()?, self.span()?),
                        pointee: Box::new(self.expr(depth + 1)?),
                    });
                }
                FunctionExprKind::UnionStorageLiteral { bytes, relocations }
            }
            41 => FunctionExprKind::Unary {
                op: unary_from_tag(self.u8()?)?,
                expr: Box::new(self.expr(depth + 1)?),
            },
            42 => FunctionExprKind::OptionalSome {
                expr: Box::new(self.expr(depth + 1)?),
            },
            43 => FunctionExprKind::ErrorOk {
                expr: Box::new(self.expr(depth + 1)?),
            },
            44 => FunctionExprKind::ErrorErr {
                expr: Box::new(self.expr(depth + 1)?),
            },
            45 => FunctionExprKind::TaggedUnionTag {
                expr: Box::new(self.expr(depth + 1)?),
            },
            46 => FunctionExprKind::TaggedUnionPayload {
                expr: Box::new(self.expr(depth + 1)?),
            },
            47 => FunctionExprKind::Try {
                expr: Box::new(self.expr(depth + 1)?),
            },
            48 => FunctionExprKind::AddrOf(self.place(depth + 1)?),
            49 => FunctionExprKind::Binary {
                lhs: Box::new(self.expr(depth + 1)?),
                op: binary_from_tag(self.u8()?)?,
                rhs: Box::new(self.expr(depth + 1)?),
            },
            50 => FunctionExprKind::Assign {
                place: self.place(depth + 1)?,
                op: assign_from_tag(self.u8()?)?,
                rhs: Box::new(self.expr(depth + 1)?),
            },
            51 => FunctionExprKind::Discard(Box::new(self.expr(depth + 1)?)),
            52 => FunctionExprKind::Cast {
                expr: Box::new(self.expr(depth + 1)?),
                ty: self.ty()?,
            },
            53 => FunctionExprKind::TraitObjectUpcast {
                expr: Box::new(self.expr(depth + 1)?),
                source_ty: self.ty()?,
                target_ty: self.ty()?,
            },
            54 => FunctionExprKind::TraitObjectCoercion {
                expr: Box::new(self.expr(depth + 1)?),
                target_ty: self.ty()?,
                self_ty: self.ty()?,
            },
            55 => FunctionExprKind::CallableCoercion {
                state: Box::new(self.expr(depth + 1)?),
                closure_id: self.closure()?,
            },
            56 => FunctionExprKind::FunctionCallable {
                function: Box::new(self.expr(depth + 1)?),
            },
            57 => FunctionExprKind::ClosureFunctionPointer {
                closure_id: self.closure()?,
            },
            58 => FunctionExprKind::Call {
                callee: self.callee(depth + 1)?,
                args: self.exprs(depth + 1)?,
            },
            59 => FunctionExprKind::Field {
                lhs: Box::new(self.expr(depth + 1)?),
                field: self.definition()?,
            },
            60 => FunctionExprKind::Index {
                lhs: Box::new(self.expr(depth + 1)?),
                index: Box::new(self.expr(depth + 1)?),
            },
            61 => FunctionExprKind::Slice {
                lhs: Box::new(self.expr(depth + 1)?),
                range: FunctionSliceRange {
                    start: self.option_expr(depth + 1)?.map(Box::new),
                    end: self.option_expr(depth + 1)?.map(Box::new),
                    inclusive: self.bool()?,
                },
                is_readonly: self.bool()?,
            },
            _ => {
                return Err(TemplateBodyCodecError::invalid(
                    "invalid checked template expression tag",
                ));
            }
        })
    }

    fn index(&mut self) -> Result<usize, TemplateBodyCodecError> {
        Ok(self.u32()? as usize)
    }

    fn types(&mut self) -> Result<Vec<InternedTyId>, TemplateBodyCodecError> {
        let count = self.len()?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(self.ty()?);
        }
        Ok(values)
    }

    fn option_ty(&mut self) -> Result<Option<InternedTyId>, TemplateBodyCodecError> {
        self.bool()?.then(|| self.ty()).transpose()
    }

    fn exprs(&mut self, depth: usize) -> Result<Vec<FunctionExpr>, TemplateBodyCodecError> {
        let count = self.len()?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(self.expr(depth)?);
        }
        Ok(values)
    }

    fn const_arguments(&mut self) -> Result<Vec<ConstGenericArg>, TemplateBodyCodecError> {
        let count = self.len()?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(self.const_argument()?);
        }
        Ok(values)
    }

    fn const_argument(&mut self) -> Result<ConstGenericArg, TemplateBodyCodecError> {
        let ty = self.ty()?;
        let value = match self.u8()? {
            0 => ConstGenericValue::GenericParam(SymbolId::from_stable_hash(self.u64()?)),
            1 => {
                let bits = self.u128()?;
                let signed = self.bool()?;
                ConstGenericValue::Int(if signed {
                    IntConst::signed_bits(bits)
                } else {
                    IntConst::unsigned(bits)
                })
            }
            2 => ConstGenericValue::Bool(self.bool()?),
            3 => ConstGenericValue::Char(char::from_u32(self.u32()?).ok_or_else(|| {
                TemplateBodyCodecError::invalid("invalid const generic character")
            })?),
            _ => {
                return Err(TemplateBodyCodecError::invalid(
                    "invalid const generic value tag",
                ));
            }
        };
        Ok(ConstGenericArg { ty, value })
    }

    fn array_len(&mut self) -> Result<ArrayLenTy, TemplateBodyCodecError> {
        Ok(match self.u8()? {
            0 => ArrayLenTy::GenericParam(SymbolId::from_stable_hash(self.u64()?)),
            1 => ArrayLenTy::ConstValue(self.u64()?),
            2 => ArrayLenTy::Builtin {
                builtin: LayoutBuiltin::from_stable_tag(self.u32()?).ok_or_else(|| {
                    TemplateBodyCodecError::invalid("invalid layout builtin identity")
                })?,
                ty: self.ty()?,
            },
            _ => return Err(TemplateBodyCodecError::invalid("invalid array length tag")),
        })
    }

    fn field(&mut self, depth: usize) -> Result<FunctionFieldInit, TemplateBodyCodecError> {
        let field = self.bool()?.then(|| self.definition()).transpose()?;
        Ok(FunctionFieldInit {
            field,
            name: self.string()?,
            value: self.expr(depth)?,
            span: self.span()?,
        })
    }

    fn fields(&mut self, depth: usize) -> Result<Vec<FunctionFieldInit>, TemplateBodyCodecError> {
        let count = self.len()?;
        let mut fields = Vec::with_capacity(count);
        for _ in 0..count {
            fields.push(self.field(depth)?);
        }
        Ok(fields)
    }

    fn source_location(&mut self) -> Result<SourceLocation, TemplateBodyCodecError> {
        Ok(SourceLocation {
            file: self.string()?,
            line: self.u32()?,
            column: self.u32()?,
        })
    }

    fn closure(&mut self) -> Result<ClosureId, TemplateBodyCodecError> {
        Ok(ClosureId {
            owner: self.definition()?,
            ordinal: self.u32()?,
        })
    }

    fn builtin_value(&mut self) -> Result<FunctionBuiltinValue, TemplateBodyCodecError> {
        Ok(match self.u8()? {
            0 => FunctionBuiltinValue::Usize(self.u64()?),
            1 => FunctionBuiltinValue::Layout {
                builtin: LayoutBuiltin::from_stable_tag(self.u32()?).ok_or_else(|| {
                    TemplateBodyCodecError::invalid("invalid layout builtin identity")
                })?,
                ty: self.ty()?,
            },
            2 => FunctionBuiltinValue::FieldOffset {
                ty: self.ty()?,
                field: self.definition()?,
            },
            3 => {
                let bits = self.u128()?;
                let signed = self.bool()?;
                FunctionBuiltinValue::Int(if signed {
                    IntConst::signed_bits(bits)
                } else {
                    IntConst::unsigned(bits)
                })
            }
            _ => return Err(TemplateBodyCodecError::invalid("invalid builtin value tag")),
        })
    }

    fn inline_asm(&mut self, depth: usize) -> Result<FunctionInlineAsm, TemplateBodyCodecError> {
        let code = self.string()?;
        let input_count = self.len()?;
        let mut inputs = Vec::with_capacity(input_count);
        for _ in 0..input_count {
            inputs.push(FunctionAsmInput {
                constraint: self.string()?,
                value: self.expr(depth)?,
                span: self.span()?,
            });
        }
        let output_count = self.len()?;
        let mut outputs = Vec::with_capacity(output_count);
        for _ in 0..output_count {
            outputs.push(FunctionAsmOutput {
                constraint: self.string()?,
                place: self.place(depth)?,
                span: self.span()?,
            });
        }
        let clobber_count = self.len()?;
        let mut clobbers = Vec::with_capacity(clobber_count);
        for _ in 0..clobber_count {
            clobbers.push(self.string()?);
        }
        let option_count = self.len()?;
        let mut options = Vec::with_capacity(option_count);
        for _ in 0..option_count {
            options.push(match self.u8()? {
                0 => FunctionAsmOption::Volatile,
                _ => return Err(TemplateBodyCodecError::invalid("invalid asm option")),
            });
        }
        Ok(FunctionInlineAsm {
            code,
            inputs,
            outputs,
            clobbers,
            options,
        })
    }

    fn atomic(&mut self, depth: usize) -> Result<FunctionAtomic, TemplateBodyCodecError> {
        Ok(match self.u8()? {
            0 => FunctionAtomic::Load {
                ty: self.ty()?,
                ptr: Box::new(self.expr(depth)?),
                order: atomic_order_from_tag(self.u8()?)?,
            },
            1 => FunctionAtomic::Store {
                ty: self.ty()?,
                ptr: Box::new(self.expr(depth)?),
                value: Box::new(self.expr(depth)?),
                order: atomic_order_from_tag(self.u8()?)?,
            },
            2 => FunctionAtomic::Rmw {
                ty: self.ty()?,
                ptr: Box::new(self.expr(depth)?),
                op: atomic_rmw_from_tag(self.u8()?)?,
                value: Box::new(self.expr(depth)?),
                order: atomic_order_from_tag(self.u8()?)?,
            },
            3 => FunctionAtomic::Cmpxchg {
                ty: self.ty()?,
                ptr: Box::new(self.expr(depth)?),
                expected: Box::new(self.expr(depth)?),
                desired: Box::new(self.expr(depth)?),
                success: atomic_order_from_tag(self.u8()?)?,
                failure: atomic_order_from_tag(self.u8()?)?,
                weak: self.bool()?,
            },
            4 => FunctionAtomic::Fence {
                order: atomic_order_from_tag(self.u8()?)?,
            },
            _ => return Err(TemplateBodyCodecError::invalid("invalid atomic operation")),
        })
    }

    fn place(&mut self, depth: usize) -> Result<FunctionPlace, TemplateBodyCodecError> {
        check_depth(depth)?;
        let span = self.span()?;
        let ty = self.ty()?;
        let base = match self.u8()? {
            0 => FunctionPlaceBase::Local(LocalId(self.u32()?)),
            1 => FunctionPlaceBase::Global(self.definition()?),
            2 => FunctionPlaceBase::GlobalInstance {
                def_id: self.definition()?,
                arg_module_id: self.module()?,
                args: self.types()?,
                const_args: self.const_arguments()?,
            },
            3 => FunctionPlaceBase::Deref(Box::new(self.expr(depth + 1)?)),
            4 => FunctionPlaceBase::Error,
            _ => {
                return Err(TemplateBodyCodecError::invalid(
                    "invalid checked template place base tag",
                ));
            }
        };
        let elem_count = self.len()?;
        let mut elems = Vec::with_capacity(elem_count);
        for _ in 0..elem_count {
            elems.push(match self.u8()? {
                0 => FunctionPlaceElem::Field(self.definition()?),
                1 => FunctionPlaceElem::TupleField(self.index()?),
                2 => FunctionPlaceElem::Index(Box::new(self.expr(depth + 1)?)),
                3 => FunctionPlaceElem::Error,
                _ => {
                    return Err(TemplateBodyCodecError::invalid(
                        "invalid checked template place projection tag",
                    ));
                }
            });
        }
        Ok(FunctionPlace {
            span,
            ty,
            base,
            elems,
        })
    }

    fn callee(&mut self, depth: usize) -> Result<FunctionCallee, TemplateBodyCodecError> {
        check_depth(depth)?;
        Ok(match self.u8()? {
            0 => FunctionCallee::Tracked {
                callee: Box::new(self.callee(depth + 1)?),
                location: self.source_location()?,
            },
            1 => FunctionCallee::ClosureEntry {
                closure_id: self.closure()?,
                state: Box::new(self.expr(depth + 1)?),
            },
            2 => FunctionCallee::Function(self.definition()?),
            3 => {
                let (def_id, arg_module_id, self_arg, args, const_args) = self.instance_ref()?;
                FunctionCallee::FunctionInstance {
                    def_id,
                    arg_module_id,
                    self_arg,
                    args,
                    const_args,
                }
            }
            4 => {
                let (def_id, arg_module_id, self_arg, args, const_args) = self.instance_ref()?;
                FunctionCallee::Method {
                    def_id,
                    arg_module_id,
                    self_arg,
                    args,
                    const_args,
                    receiver_kind: self.receiver_kind()?,
                    receiver: Box::new(self.expr(depth + 1)?),
                }
            }
            5 => FunctionCallee::TraitMethod {
                trait_id: self.definition()?,
                method_id: self.definition()?,
                method_name: SymbolId::from_stable_hash(self.u64()?),
                self_ty: self.ty()?,
                trait_args: self.types()?,
                trait_const_args: self.const_arguments()?,
                args: self.types()?,
                const_args: self.const_arguments()?,
                receiver_kind: self.receiver_kind()?,
                receiver: Box::new(self.expr(depth + 1)?),
            },
            6 => FunctionCallee::TraitAssociatedFunction {
                trait_id: self.definition()?,
                method_id: self.definition()?,
                method_name: SymbolId::from_stable_hash(self.u64()?),
                self_ty: self.ty()?,
                trait_args: self.types()?,
                trait_const_args: self.const_arguments()?,
                args: self.types()?,
                const_args: self.const_arguments()?,
            },
            7 => FunctionCallee::DynamicTraitMethod {
                object_ty: self.ty()?,
                trait_id: self.trait_id()?,
                method_id: self.definition()?,
                method_name: SymbolId::from_stable_hash(self.u64()?),
                trait_args: self.types()?,
                trait_const_args: self.const_arguments()?,
                slot: self.index()?,
                params: self.types()?,
                return_type: self.ty()?,
                receiver_kind: self.receiver_kind()?,
                receiver: Box::new(self.expr(depth + 1)?),
            },
            8 => FunctionCallee::BuiltinMethod {
                method: builtin_method_from_tag(self.u8()?)?,
                self_ty: self.ty()?,
                receiver: Box::new(self.expr(depth + 1)?),
            },
            9 => FunctionCallee::BuiltinTraitMethodCall {
                trait_id: BuiltinTrait::from_stable_tag(self.u32()?).ok_or_else(|| {
                    TemplateBodyCodecError::invalid("invalid builtin trait identity")
                })?,
                method: BuiltinTraitMethod::from_stable_tag(self.u32()?).ok_or_else(|| {
                    TemplateBodyCodecError::invalid("invalid builtin trait method identity")
                })?,
                self_ty: self.ty()?,
                trait_args: self.types()?,
                receiver: Box::new(self.expr(depth + 1)?),
            },
            10 => {
                let trait_id = BuiltinTrait::from_stable_tag(self.u32()?).ok_or_else(|| {
                    TemplateBodyCodecError::invalid("invalid builtin operator trait identity")
                })?;
                let op = match self.u8()? {
                    0 => FunctionBuiltinOperatorOp::Unary(unary_from_tag(self.u8()?)?),
                    1 => FunctionBuiltinOperatorOp::Binary(binary_from_tag(self.u8()?)?),
                    _ => {
                        return Err(TemplateBodyCodecError::invalid(
                            "invalid builtin operator form",
                        ));
                    }
                };
                FunctionCallee::BuiltinOperator(FunctionBuiltinOperator { trait_id, op })
            }
            11 => FunctionCallee::Callable(Box::new(self.expr(depth + 1)?)),
            12 => FunctionCallee::FunctionPointer(Box::new(self.expr(depth + 1)?)),
            _ => {
                return Err(TemplateBodyCodecError::invalid(
                    "invalid checked template callee tag",
                ));
            }
        })
    }

    fn instance_ref(
        &mut self,
    ) -> Result<
        (
            GlobalDefId,
            ModuleId,
            Option<InternedTyId>,
            Vec<InternedTyId>,
            Vec<ConstGenericArg>,
        ),
        TemplateBodyCodecError,
    > {
        Ok((
            self.definition()?,
            self.module()?,
            self.option_ty()?,
            self.types()?,
            self.const_arguments()?,
        ))
    }

    fn trait_id(&mut self) -> Result<TraitId, TemplateBodyCodecError> {
        match self.u8()? {
            0 => Ok(TraitId::Source(self.definition()?)),
            1 => Ok(TraitId::Builtin(
                BuiltinTrait::from_stable_tag(self.u32()?).ok_or_else(|| {
                    TemplateBodyCodecError::invalid("invalid builtin trait identity")
                })?,
            )),
            _ => Err(TemplateBodyCodecError::invalid(
                "invalid checked template trait identity tag",
            )),
        }
    }

    fn receiver_kind(&mut self) -> Result<ReceiverKind, TemplateBodyCodecError> {
        ReceiverKind::from_stable_tag(self.u32()?)
            .ok_or_else(|| TemplateBodyCodecError::invalid("invalid receiver passing mode"))
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

fn unary_from_tag(tag: u8) -> Result<UnaryOp, TemplateBodyCodecError> {
    match tag {
        0 => Ok(UnaryOp::Neg),
        1 => Ok(UnaryOp::Not),
        2 => Ok(UnaryOp::BitNot),
        3 => Ok(UnaryOp::RefReadOnly),
        4 => Ok(UnaryOp::Ref),
        5 => Ok(UnaryOp::Deref),
        _ => Err(TemplateBodyCodecError::invalid("invalid unary operator")),
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

fn binary_from_tag(tag: u8) -> Result<BinaryOp, TemplateBodyCodecError> {
    match tag {
        0 => Ok(BinaryOp::Mul),
        1 => Ok(BinaryOp::Div),
        2 => Ok(BinaryOp::Rem),
        3 => Ok(BinaryOp::Add),
        4 => Ok(BinaryOp::Sub),
        5 => Ok(BinaryOp::Shl),
        6 => Ok(BinaryOp::Shr),
        7 => Ok(BinaryOp::Lt),
        8 => Ok(BinaryOp::Le),
        9 => Ok(BinaryOp::Gt),
        10 => Ok(BinaryOp::Ge),
        11 => Ok(BinaryOp::Eq),
        12 => Ok(BinaryOp::Ne),
        13 => Ok(BinaryOp::BitAnd),
        14 => Ok(BinaryOp::BitXor),
        15 => Ok(BinaryOp::BitOr),
        16 => Ok(BinaryOp::And),
        17 => Ok(BinaryOp::Or),
        _ => Err(TemplateBodyCodecError::invalid("invalid binary operator")),
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

fn assign_from_tag(tag: u8) -> Result<AssignOp, TemplateBodyCodecError> {
    match tag {
        0 => Ok(AssignOp::Assign),
        1 => Ok(AssignOp::Add),
        2 => Ok(AssignOp::Sub),
        3 => Ok(AssignOp::Shl),
        4 => Ok(AssignOp::Shr),
        5 => Ok(AssignOp::Mul),
        6 => Ok(AssignOp::Div),
        7 => Ok(AssignOp::Rem),
        8 => Ok(AssignOp::BitAnd),
        9 => Ok(AssignOp::BitXor),
        10 => Ok(AssignOp::BitOr),
        _ => Err(TemplateBodyCodecError::invalid(
            "invalid assignment operator",
        )),
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

fn builtin_method_from_tag(tag: u8) -> Result<FunctionBuiltinMethod, TemplateBodyCodecError> {
    match tag {
        0 => Ok(FunctionBuiltinMethod::SliceLen),
        1 => Ok(FunctionBuiltinMethod::SlicePtr),
        2 => Ok(FunctionBuiltinMethod::SlicePtrMut),
        3 => Ok(FunctionBuiltinMethod::Start),
        4 => Ok(FunctionBuiltinMethod::End),
        5 => Ok(FunctionBuiltinMethod::Iter),
        _ => Err(TemplateBodyCodecError::invalid("invalid builtin method")),
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

fn atomic_order_from_tag(tag: u8) -> Result<AtomicOrder, TemplateBodyCodecError> {
    match tag {
        0 => Ok(AtomicOrder::Unordered),
        1 => Ok(AtomicOrder::Monotonic),
        2 => Ok(AtomicOrder::Acquire),
        3 => Ok(AtomicOrder::Release),
        4 => Ok(AtomicOrder::AcqRel),
        5 => Ok(AtomicOrder::SeqCst),
        _ => Err(TemplateBodyCodecError::invalid("invalid atomic ordering")),
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

fn atomic_rmw_from_tag(tag: u8) -> Result<AtomicRmwOp, TemplateBodyCodecError> {
    match tag {
        0 => Ok(AtomicRmwOp::Xchg),
        1 => Ok(AtomicRmwOp::Add),
        2 => Ok(AtomicRmwOp::Sub),
        3 => Ok(AtomicRmwOp::And),
        4 => Ok(AtomicRmwOp::Nand),
        5 => Ok(AtomicRmwOp::Or),
        6 => Ok(AtomicRmwOp::Xor),
        7 => Ok(AtomicRmwOp::Max),
        8 => Ok(AtomicRmwOp::Min),
        9 => Ok(AtomicRmwOp::UMax),
        10 => Ok(AtomicRmwOp::UMin),
        _ => Err(TemplateBodyCodecError::invalid(
            "invalid atomic read-modify-write operation",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::ModuleIdAllocator;
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

    impl TemplateBodyDecodeContext for Context {
        fn type_at(&self, index: u32) -> Option<InternedTyId> {
            self.types.get(index as usize).copied()
        }

        fn definition_at(&self, index: u32) -> Option<GlobalDefId> {
            self.definitions.get(index as usize).copied()
        }

        fn module_at(&self, index: u32) -> Option<ModuleId> {
            self.modules.get(index as usize).copied()
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
    fn round_trip_and_cross_session_type_relocation() {
        let (body, first_ty, second_ty) = fixture();
        let source = Context {
            types: vec![first_ty],
            definitions: Vec::new(),
            modules: Vec::new(),
        };
        let bytes = encode_checked_function_body(&body, &source).expect("encode");
        let target = Context {
            types: vec![second_ty],
            definitions: Vec::new(),
            modules: Vec::new(),
        };
        let decoded = decode_checked_function_body(&bytes, &target).expect("decode");
        assert_eq!(decoded.locals[0].ty, second_ty);
        assert_eq!(decoded.ty, second_ty);
        assert_eq!(decoded.blocks[0].ops.len(), 1);
        assert!(nia_function_ir::validate_function_body(&decoded).is_ok());
    }

    #[test]
    fn rejects_corrupt_headers_trailing_bytes_and_missing_relocations() {
        let (body, first_ty, _) = fixture();
        let source = Context {
            types: vec![first_ty],
            definitions: Vec::new(),
            modules: Vec::new(),
        };
        let bytes = encode_checked_function_body(&body, &source).expect("encode");

        let mut bad_magic = bytes.clone();
        bad_magic[0] ^= 1;
        assert!(decode_checked_function_body(&bad_magic, &source).is_err());

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode_checked_function_body(&trailing, &source).is_err());

        let missing = Context {
            types: Vec::new(),
            definitions: Vec::new(),
            modules: Vec::new(),
        };
        assert!(decode_checked_function_body(&bytes, &missing).is_err());
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
