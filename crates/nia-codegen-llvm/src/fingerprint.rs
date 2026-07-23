// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{AssignOp, BinaryOp, UnaryOp};
use nia_backend_ir::*;
use nia_function_ir::*;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, ReceiverKind};
use nia_layout::{StructLayout, TypeLayout};
use nia_llvm::target::TargetMachineIdentity;
use nia_opt::{
    InlineThreshold, NiaOptimizationLevel, OptimizationDepth, OptimizationPolicy,
    SpecializationPolicy,
};
use nia_query::QueryFingerprintBuilder;
use nia_static_ir::{StaticAddressElem, StaticInit};
use nia_ty::{
    ArrayLenTy, AssociatedTypeBindingTy, ConstGenericArg, ConstGenericValue, IntConst, TraitId,
    TyKind,
};

use crate::{
    CodegenUnitFingerprintComponents, CodegenUnitFingerprintSet, LlvmCodegenOptions,
    compiler_builtins::CompilerBuiltinSymbols, program_index::ProgramIndex,
};

#[derive(Clone, Copy)]
pub(super) enum ArtifactTarget<'a> {
    LlvmIr,
    NativeObject(&'a TargetMachineIdentity),
}

pub(super) fn source_unit_fingerprint(
    partition: &CodegenPartition,
    index: &ProgramIndex,
    options: LlvmCodegenOptions,
    target: ArtifactTarget<'_>,
) -> CodegenUnitFingerprintSet {
    let mut policy = Encoder::new("nia.llvm.source-policy.v2", index);
    policy.compiler_contract();
    policy.codegen_unit_key(&partition.key);
    policy.optimization(options.optimization);
    policy.artifact_kind(target);

    let owner = index.program().module_for_partition(partition);
    let mut definition = Encoder::new("nia.llvm.source-definition.v3", index);
    definition.partition_definitions(partition, owner);

    let mut declaration = Encoder::new("nia.llvm.source-declarations.v2", index);
    let mut declarations = index.program().modules.iter().collect::<Vec<_>>();
    declarations.sort_unstable_by(|left, right| {
        left.source_identity
            .normalized_path()
            .cmp(right.source_identity.normalized_path())
    });
    declaration.len(declarations.len());
    for module in declarations {
        declaration.module_declarations(module);
    }

    let mut target_component = Encoder::new("nia.llvm.source-target.v2", index);
    target_component.artifact_target(target);
    CodegenUnitFingerprintSet::new(CodegenUnitFingerprintComponents {
        policy: policy.finish(),
        definition: definition.finish(),
        declarations: declaration.finish(),
        target: target_component.finish(),
    })
}

pub(super) fn compiler_builtins_fingerprint(
    symbols: &CompilerBuiltinSymbols,
    options: LlvmCodegenOptions,
    target: &TargetMachineIdentity,
) -> CodegenUnitFingerprintSet {
    let mut policy = QueryFingerprintBuilder::new("nia.llvm.builtins-policy.v2");
    policy.write_str(env!("CARGO_PKG_VERSION"));
    policy.write_u64(llvm_sys_version());
    write_optimization(&mut policy, options.optimization);

    let mut definition = QueryFingerprintBuilder::new("nia.llvm.builtins-definition.v2");
    definition.write_u8(u8::from(symbols.u128_div_rem));
    definition.write_u8(u8::from(symbols.i128_div_rem));

    let declarations = QueryFingerprintBuilder::new("nia.llvm.builtins-declarations.v2");
    let mut target_component = QueryFingerprintBuilder::new("nia.llvm.builtins-target.v2");
    write_target_identity(&mut target_component, target);
    CodegenUnitFingerprintSet::new(CodegenUnitFingerprintComponents {
        policy: finish_builder(policy),
        definition: finish_builder(definition),
        declarations: finish_builder(declarations),
        target: finish_builder(target_component),
    })
}

fn finish_builder(builder: QueryFingerprintBuilder) -> CodegenUnitFingerprint {
    CodegenUnitFingerprint::from_parts(builder.finish().parts())
}

struct Encoder<'a> {
    builder: QueryFingerprintBuilder,
    index: &'a ProgramIndex,
}

impl<'a> Encoder<'a> {
    fn new(domain: &str, index: &'a ProgramIndex) -> Self {
        Self {
            builder: QueryFingerprintBuilder::new(domain),
            index,
        }
    }

    fn finish(self) -> CodegenUnitFingerprint {
        CodegenUnitFingerprint::from_parts(self.builder.finish().parts())
    }

    fn compiler_contract(&mut self) {
        self.builder.write_str(env!("CARGO_PKG_VERSION"));
        self.builder.write_u64(llvm_sys_version());
    }

    fn tag(&mut self, tag: u8) {
        self.builder.write_u8(tag);
    }

    fn bool(&mut self, value: bool) {
        self.builder.write_u8(u8::from(value));
    }

    fn u32(&mut self, value: u32) {
        self.builder.write_u64(u64::from(value));
    }

    fn usize(&mut self, value: usize) {
        self.builder.write_u64(value as u64);
    }

    fn len(&mut self, value: usize) {
        self.usize(value);
    }

    fn u128(&mut self, value: u128) {
        self.builder.write_u64(value as u64);
        self.builder.write_u64((value >> 64) as u64);
    }

    fn symbol(&mut self, symbol: nia_symbol::SymbolId) {
        self.builder.write_u64(symbol.raw());
    }

    fn module_id(&mut self, module_id: ModuleId) {
        let module = self.index.module(module_id).unwrap_or_else(|| {
            panic!("Nia ICE: codegen fingerprint references missing module {module_id:?}")
        });
        self.builder
            .write_str(module.source_identity.normalized_path());
    }

    fn global_def(&mut self, def_id: GlobalDefId) {
        self.module_id(def_id.module_id);
        self.builder.write_u64(def_id.def_id.0);
    }

    fn global_const_expr(&mut self, expr: GlobalConstExprId) {
        self.module_id(expr.module_id);
        self.u32(expr.const_expr_id.0);
    }

    fn codegen_unit_key(&mut self, key: &CodegenUnitKey) {
        match key {
            CodegenUnitKey::SourceModule {
                source_identity,
                ordinal,
            } => {
                self.tag(0);
                self.builder.write_str(source_identity.normalized_path());
                self.u32(*ordinal);
            }
            CodegenUnitKey::CompilerBuiltins => self.tag(1),
        }
    }

    fn optimization(&mut self, policy: OptimizationPolicy) {
        write_optimization(&mut self.builder, policy);
    }

    fn artifact_target(&mut self, target: ArtifactTarget<'_>) {
        match target {
            ArtifactTarget::LlvmIr => self.tag(0),
            ArtifactTarget::NativeObject(identity) => {
                self.tag(1);
                write_target_identity(&mut self.builder, identity);
            }
        }
    }

    fn artifact_kind(&mut self, target: ArtifactTarget<'_>) {
        self.tag(match target {
            ArtifactTarget::LlvmIr => 0,
            ArtifactTarget::NativeObject(_) => 1,
        });
    }

    fn partition_definitions(&mut self, partition: &CodegenPartition, module: &BackendModule) {
        self.len(partition.global_definitions().len());
        for &index in partition.global_definitions() {
            let item = &module.globals[index];
            self.global(item, item.init.as_ref());
        }
        self.len(partition.global_instance_definitions().len());
        for &index in partition.global_instance_definitions() {
            let item = &module.global_instances[index];
            self.global_instance(item, item.init.as_ref());
        }
        self.len(partition.function_definitions().len());
        for &index in partition.function_definitions() {
            let item = &module.functions[index];
            self.function(
                item.def_id,
                item.name,
                item.link_name.as_deref(),
                &item.generics,
                &item.params,
                item.return_type,
                item.is_extern,
                item.is_variadic,
                &item.attributes,
                item.function_body.as_ref(),
            );
        }
        self.len(partition.function_instance_definitions().len());
        for &index in partition.function_instance_definitions() {
            let item = &module.function_instances[index];
            self.function_instance(item, item.function_body.as_ref());
        }
        self.len(partition.vtable_definitions().len());
        for &index in partition.vtable_definitions() {
            self.trait_object_vtable(&module.trait_object_vtables[index]);
        }
    }

    fn module_declarations(&mut self, module: &BackendModule) {
        self.builder
            .write_str(module.source_identity.normalized_path());
        self.builder.write_str(&module.name);
        self.bool(false);
        self.builder.write_u64(module.layouts.target.pointer_size);
        self.builder.write_u64(module.layouts.target.pointer_align);

        let mut array_lengths = module.const_eval.array_lengths.iter().collect::<Vec<_>>();
        array_lengths.sort_unstable_by_key(|(id, _)| id.const_expr_id);
        self.len(array_lengths.len());
        for (id, value) in array_lengths {
            self.global_const_expr(*id);
            self.builder.write_u64(*value);
        }

        let mut structs = module.structs.iter().collect::<Vec<_>>();
        structs.sort_unstable_by_key(|item| item.def_id.def_id);
        self.len(structs.len());
        for item in structs {
            self.aggregate(
                item.def_id,
                item.name,
                &item.generics,
                &item.fields,
                item.is_extern,
            );
            self.optional_struct_layout(self.index.struct_layout(item.def_id));
        }
        let mut unions = module.unions.iter().collect::<Vec<_>>();
        unions.sort_unstable_by_key(|item| item.def_id.def_id);
        self.len(unions.len());
        for item in unions {
            self.aggregate(
                item.def_id,
                item.name,
                &item.generics,
                &item.fields,
                item.is_extern,
            );
            self.optional_struct_layout(self.index.union_layout(item.def_id));
        }
        let mut struct_instances = module.struct_instances.iter().collect::<Vec<_>>();
        struct_instances.sort_unstable_by(|left, right| left.symbol.cmp(&right.symbol));
        self.len(struct_instances.len());
        for item in struct_instances {
            self.aggregate_instance(
                item.def_id,
                item.name,
                &item.args,
                &item.const_args,
                &item.symbol,
                &item.fields,
                item.is_extern,
            );
            self.optional_struct_layout(self.index.struct_instance_layout(
                item.def_id,
                &item.args,
                &item.const_args,
            ));
        }
        let mut union_instances = module.union_instances.iter().collect::<Vec<_>>();
        union_instances.sort_unstable_by(|left, right| left.symbol.cmp(&right.symbol));
        self.len(union_instances.len());
        for item in union_instances {
            self.aggregate_instance(
                item.def_id,
                item.name,
                &item.args,
                &item.const_args,
                &item.symbol,
                &item.fields,
                item.is_extern,
            );
            self.optional_struct_layout(self.index.union_instance_layout(
                item.def_id,
                &item.args,
                &item.const_args,
            ));
        }

        let mut enums = module.enums.iter().collect::<Vec<_>>();
        enums.sort_unstable_by_key(|item| item.def_id.def_id);
        self.len(enums.len());
        for item in enums {
            self.global_def(item.def_id);
            self.symbol(item.name);
            self.ty(item.backing_type);
            self.len(item.variants.len());
            for variant in &item.variants {
                self.global_def(variant.def_id);
                self.symbol(variant.name);
                self.optional_i128(variant.value);
            }
        }

        let mut globals = module.globals.iter().collect::<Vec<_>>();
        globals.sort_unstable_by_key(|item| item.def_id.def_id);
        self.len(globals.len());
        for item in globals {
            self.global(item, None);
        }
        let mut global_instances = module.global_instances.iter().collect::<Vec<_>>();
        global_instances.sort_unstable_by(|left, right| left.symbol.cmp(&right.symbol));
        self.len(global_instances.len());
        for item in global_instances {
            self.global_instance(item, None);
        }

        let mut functions = module.functions.iter().collect::<Vec<_>>();
        functions.sort_unstable_by_key(|item| item.def_id.def_id);
        self.len(functions.len());
        for item in functions {
            self.function(
                item.def_id,
                item.name,
                item.link_name.as_deref(),
                &item.generics,
                &item.params,
                item.return_type,
                item.is_extern,
                item.is_variadic,
                &item.attributes,
                None,
            );
        }
        let mut function_instances = module.function_instances.iter().collect::<Vec<_>>();
        function_instances.sort_unstable_by(|left, right| left.symbol.cmp(&right.symbol));
        self.len(function_instances.len());
        for item in function_instances {
            self.function_instance(item, None);
        }

        let mut trait_object_vtables = module
            .trait_object_vtables
            .iter()
            .map(|item| (self.trait_object_vtable_sort_key(item), item))
            .collect::<Vec<_>>();
        trait_object_vtables.sort_unstable_by_key(|(key, _)| *key);
        self.len(trait_object_vtables.len());
        for (_, item) in trait_object_vtables {
            self.trait_object_vtable(item);
        }

        let mut generic_instantiations = module
            .generic_instantiations
            .iter()
            .map(|item| (self.generic_instantiation_sort_key(item), item))
            .collect::<Vec<_>>();
        generic_instantiations.sort_unstable_by_key(|(key, _)| *key);
        self.len(generic_instantiations.len());
        for (_, item) in generic_instantiations {
            self.global_def(item.def_id);
            self.module_id(item.arg_module_id);
            self.optional_ty(item.self_arg);
            self.types(&item.args);
            self.const_args(&item.const_args);
            self.optional_global_def(item.source_def_id);
        }
    }

    fn trait_object_vtable_sort_key(&self, item: &BackendTraitObjectVtable) -> [u64; 2] {
        let mut encoder = Encoder::new("nia.llvm.vtable-sort-key.v1", self.index);
        encoder.ty(item.key.self_ty);
        encoder.ty(item.key.object_ty);
        encoder.trait_id(item.trait_id);
        encoder.types(&item.trait_args);
        encoder.finish().parts()
    }

    fn generic_instantiation_sort_key(&self, item: &BackendGenericInstantiation) -> [u64; 2] {
        let mut encoder = Encoder::new("nia.llvm.generic-instantiation-sort-key.v1", self.index);
        encoder.global_def(item.def_id);
        encoder.module_id(item.arg_module_id);
        encoder.optional_ty(item.self_arg);
        encoder.types(&item.args);
        encoder.const_args(&item.const_args);
        encoder.optional_global_def(item.source_def_id);
        encoder.finish().parts()
    }

    fn global(&mut self, item: &BackendGlobal, init: Option<&StaticInit>) {
        self.global_def(item.def_id);
        self.symbol(item.name);
        self.optional_str(item.link_name.as_deref());
        self.ty(item.ty);
        self.bool(item.is_let);
        self.bool(item.is_extern);
        self.optional_static_init(init);
    }

    fn global_instance(&mut self, item: &BackendGlobalInstance, init: Option<&StaticInit>) {
        self.global_def(item.def_id);
        self.symbol(item.name);
        self.module_id(item.arg_module_id);
        self.types(&item.args);
        self.const_args(&item.const_args);
        self.builder.write_str(&item.symbol);
        self.ty(item.ty);
        self.bool(item.is_let);
        self.optional_static_init(init);
    }

    fn function_instance(&mut self, item: &BackendFunctionInstance, body: Option<&FunctionBody>) {
        self.global_def(item.def_id);
        self.symbol(item.name);
        self.module_id(item.arg_module_id);
        self.optional_ty(item.self_arg);
        self.types(&item.args);
        self.const_args(&item.const_args);
        self.builder.write_str(&item.symbol);
        self.params(&item.params);
        self.ty(item.return_type);
        self.bool(item.is_extern);
        self.bool(item.is_variadic);
        self.function_attributes(&item.attributes);
        self.optional_function_body(body);
    }

    fn trait_object_vtable(&mut self, item: &BackendTraitObjectVtable) {
        self.ty(item.key.self_ty);
        self.ty(item.key.object_ty);
        self.trait_id(item.trait_id);
        self.types(&item.trait_args);
        self.len(item.entries.len());
        for entry in &item.entries {
            self.trait_id(entry.trait_id);
            self.global_def(entry.method_id);
            self.symbol(entry.method_name);
            self.usize(entry.slot);
            match &entry.function {
                BackendTraitObjectVtableFunction::Function(def_id) => {
                    self.tag(0);
                    self.global_def(*def_id);
                }
                BackendTraitObjectVtableFunction::FunctionInstance {
                    def_id,
                    arg_module_id,
                    self_arg,
                    args,
                    const_args,
                } => {
                    self.tag(1);
                    self.global_def(*def_id);
                    self.module_id(*arg_module_id);
                    self.optional_ty(*self_arg);
                    self.types(args);
                    self.const_args(const_args);
                }
            }
        }
    }

    fn aggregate(
        &mut self,
        def_id: GlobalDefId,
        name: nia_symbol::SymbolId,
        generics: &[nia_symbol::SymbolId],
        fields: &[BackendField],
        is_extern: bool,
    ) {
        self.global_def(def_id);
        self.symbol(name);
        self.symbols(generics);
        self.fields(fields);
        self.bool(is_extern);
    }

    #[allow(clippy::too_many_arguments)]
    fn aggregate_instance(
        &mut self,
        def_id: GlobalDefId,
        name: nia_symbol::SymbolId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
        symbol: &str,
        fields: &[BackendField],
        is_extern: bool,
    ) {
        self.global_def(def_id);
        self.symbol(name);
        self.types(args);
        self.const_args(const_args);
        self.builder.write_str(symbol);
        self.fields(fields);
        self.bool(is_extern);
    }

    fn fields(&mut self, fields: &[BackendField]) {
        self.len(fields.len());
        for field in fields {
            self.global_def(field.def_id);
            self.symbol(field.name);
            self.ty(field.ty);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn function(
        &mut self,
        def_id: GlobalDefId,
        name: nia_symbol::SymbolId,
        link_name: Option<&str>,
        generics: &[nia_symbol::SymbolId],
        params: &[BackendParam],
        return_type: InternedTyId,
        is_extern: bool,
        is_variadic: bool,
        attributes: &[BackendFunctionAttribute],
        body: Option<&FunctionBody>,
    ) {
        self.global_def(def_id);
        self.symbol(name);
        self.optional_str(link_name);
        self.symbols(generics);
        self.params(params);
        self.ty(return_type);
        self.bool(is_extern);
        self.bool(is_variadic);
        self.function_attributes(attributes);
        self.optional_function_body(body);
    }

    fn params(&mut self, params: &[BackendParam]) {
        self.len(params.len());
        for param in params {
            self.optional_local(param.local_id);
            self.optional_symbol(param.name);
            self.optional_receiver(param.receiver);
            self.ty(param.passing_ty);
            self.ty(param.local_ty);
        }
    }

    fn function_attributes(&mut self, attributes: &[BackendFunctionAttribute]) {
        self.len(attributes.len());
        for attribute in attributes {
            self.tag(match attribute {
                BackendFunctionAttribute::Naked => 0,
            });
        }
    }

    fn optional_function_body(&mut self, body: Option<&FunctionBody>) {
        match body {
            Some(body) => {
                self.tag(1);
                self.function_body(body);
            }
            None => self.tag(0),
        }
    }

    fn function_body(&mut self, body: &FunctionBody) {
        self.len(body.locals.len());
        for local in &body.locals {
            self.local(local.id);
            self.tag(local.kind as u8);
            self.ty(local.ty);
        }
        self.scopes(&body.scopes);
        self.blocks(&body.blocks);
        self.block_id(body.entry);
        self.ty(body.ty);
    }

    fn scopes(&mut self, scopes: &[FunctionScope]) {
        self.len(scopes.len());
        for scope in scopes {
            self.scope_id(scope.id);
            match scope.parent {
                Some(parent) => {
                    self.tag(1);
                    self.scope_id(parent);
                }
                None => self.tag(0),
            }
        }
    }

    fn blocks(&mut self, blocks: &[FunctionBlock]) {
        self.len(blocks.len());
        for block in blocks {
            self.block_id(block.id);
            self.scope_id(block.scope);
            self.len(block.ops.len());
            for op in &block.ops {
                self.function_op(op);
            }
            self.terminator(&block.terminator);
        }
    }

    fn function_op(&mut self, op: &FunctionOp) {
        match op {
            FunctionOp::Binding(binding) => {
                self.tag(0);
                self.local(binding.local_id);
                self.ty(binding.ty);
                self.optional_expr(binding.value.as_ref());
                self.bool(binding.is_let);
            }
            FunctionOp::StoreLocal {
                local_id, value, ..
            } => {
                self.tag(1);
                self.local(*local_id);
                self.expr(value);
            }
            FunctionOp::MemoryIntrinsic(intrinsic) => {
                self.tag(2);
                self.tag(intrinsic.op as u8);
                self.ty(intrinsic.elem_ty);
                self.expr(&intrinsic.dest);
                match &intrinsic.source {
                    FunctionMemoryIntrinsicSource::Slice(value) => {
                        self.tag(0);
                        self.expr(value);
                    }
                    FunctionMemoryIntrinsicSource::Byte(value) => {
                        self.tag(1);
                        self.expr(value);
                    }
                }
            }
            FunctionOp::Expr(expr) => {
                self.tag(3);
                self.expr(expr);
            }
            FunctionOp::Defer(body) => {
                self.tag(4);
                self.scopes(&body.scopes);
                self.blocks(&body.blocks);
                self.block_id(body.entry);
            }
        }
    }

    fn terminator(&mut self, terminator: &FunctionTerminator) {
        match terminator {
            FunctionTerminator::Error { .. } => self.tag(0),
            FunctionTerminator::Branch { target, .. } => {
                self.tag(1);
                self.block_id(*target);
            }
            FunctionTerminator::Next { target, .. } => {
                self.tag(2);
                self.block_id(*target);
            }
            FunctionTerminator::If {
                cond,
                then_target,
                else_target,
                ..
            } => {
                self.tag(3);
                self.expr(cond);
                self.block_id(*then_target);
                self.block_id(*else_target);
            }
            FunctionTerminator::Switch {
                target,
                arms,
                default,
                fallback,
                ..
            } => {
                self.tag(4);
                self.expr(target);
                self.len(arms.len());
                for arm in arms {
                    self.expr(&arm.pattern);
                    self.block_id(arm.target);
                }
                self.optional_block(*default);
                self.block_id(*fallback);
            }
            FunctionTerminator::Try {
                value,
                kind,
                success_local,
                success_target,
                ..
            } => {
                self.tag(5);
                self.expr(value);
                self.tag(*kind as u8);
                self.local(*success_local);
                self.block_id(*success_target);
            }
            FunctionTerminator::Loop {
                header,
                body,
                continue_target,
                break_target,
                ..
            } => {
                self.tag(6);
                match header {
                    FunctionForHeader::Infinite => self.tag(0),
                    FunctionForHeader::Condition(condition) => {
                        self.tag(1);
                        self.expr(condition);
                    }
                }
                self.block_id(*body);
                self.block_id(*continue_target);
                self.block_id(*break_target);
            }
            FunctionTerminator::Return { value, .. } => {
                self.tag(7);
                self.optional_expr(value.as_ref());
            }
            FunctionTerminator::Tail { value, .. } => {
                self.tag(8);
                self.optional_expr(value.as_ref());
            }
        }
    }

    fn expr(&mut self, expr: &FunctionExpr) {
        self.ty(expr.ty);
        self.expr_kind(&expr.kind);
    }

    fn expr_kind(&mut self, kind: &FunctionExprKind) {
        match kind {
            FunctionExprKind::Error => self.tag(0),
            FunctionExprKind::Integer(value) => {
                self.tag(1);
                self.builder.write_str(value);
            }
            FunctionExprKind::Float(value) => {
                self.tag(2);
                self.builder.write_str(value);
            }
            FunctionExprKind::String(value) => {
                self.tag(3);
                self.u32s(value);
            }
            FunctionExprKind::ByteString(value) => {
                self.tag(4);
                self.builder.write_bytes(value);
            }
            FunctionExprKind::Char(value) => {
                self.tag(5);
                self.u32(*value);
            }
            FunctionExprKind::ByteChar(value) => {
                self.tag(6);
                self.builder.write_str(value);
            }
            FunctionExprKind::Bool(value) => {
                self.tag(7);
                self.bool(*value);
            }
            FunctionExprKind::Null => self.tag(8),
            FunctionExprKind::Local(local) => {
                self.tag(9);
                self.local(*local);
            }
            FunctionExprKind::Global(def_id) => {
                self.tag(10);
                self.global_def(*def_id);
            }
            FunctionExprKind::ConstGeneric(arg) => {
                self.tag(11);
                self.const_arg(arg);
            }
            FunctionExprKind::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => {
                self.tag(12);
                self.global_def(*def_id);
                self.module_id(*arg_module_id);
                self.types(args);
                self.const_args(const_args);
            }
            FunctionExprKind::Function(def_id) => {
                self.tag(13);
                self.global_def(*def_id);
            }
            FunctionExprKind::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => {
                self.tag(14);
                self.global_def(*def_id);
                self.module_id(*arg_module_id);
                self.optional_ty(*self_arg);
                self.types(args);
                self.const_args(const_args);
            }
            FunctionExprKind::EnumVariant(def_id) => {
                self.tag(15);
                self.global_def(*def_id);
            }
            FunctionExprKind::BuiltinValue(value) => {
                self.tag(16);
                self.builtin_value(value);
            }
            FunctionExprKind::Trap => self.tag(17),
            FunctionExprKind::Range(range) => {
                self.tag(18);
                self.optional_expr(range.start.as_deref());
                self.optional_expr(range.end.as_deref());
                self.bool(range.inclusive);
            }
            FunctionExprKind::RangeBound { range, bound } => {
                self.tag(19);
                self.expr(range);
                self.tag(*bound as u8);
            }
            FunctionExprKind::InlineAsm(asm) => {
                self.tag(20);
                self.inline_asm(asm);
            }
            FunctionExprKind::Atomic(atomic) => {
                self.tag(21);
                self.atomic(atomic);
            }
            FunctionExprKind::LoadUnaligned { ty, ptr } => {
                self.tag(22);
                self.ty(*ty);
                self.expr(ptr);
            }
            FunctionExprKind::Splat { value } => {
                self.tag(23);
                self.expr(value);
            }
            FunctionExprKind::ExtractElement { vector, index } => {
                self.tag(24);
                self.expr(vector);
                self.expr(index);
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.tag(25);
                self.expr(vector);
                self.expr(index);
                self.expr(value);
            }
            FunctionExprKind::Bitmask { vector } => {
                self.tag(26);
                self.expr(vector);
            }
            FunctionExprKind::BitIntrinsic { op, value } => {
                self.tag(27);
                self.tag(*op as u8);
                self.expr(value);
            }
            FunctionExprKind::CharFromU32 { value } => {
                self.tag(28);
                self.expr(value);
            }
            FunctionExprKind::StaticArrayPointer { array, is_readonly } => {
                self.tag(29);
                self.expr(array);
                self.bool(*is_readonly);
            }
            FunctionExprKind::ArrayLiteral { elems } => {
                self.tag(30);
                match elems {
                    FunctionArrayElements::List(values) => {
                        self.tag(0);
                        self.exprs(values);
                    }
                    FunctionArrayElements::Repeat { value, count } => {
                        self.tag(1);
                        self.expr(value);
                        self.array_len(count);
                    }
                }
            }
            FunctionExprKind::StructLiteral { def_id, fields } => {
                self.tag(31);
                self.global_def(*def_id);
                self.function_fields(fields);
            }
            FunctionExprKind::UnionLiteral { def_id, field } => {
                self.tag(32);
                self.global_def(*def_id);
                self.function_field(field);
            }
            FunctionExprKind::Unary { op, expr } => {
                self.tag(33);
                self.unary(*op);
                self.expr(expr);
            }
            FunctionExprKind::OptionalSome { expr } => {
                self.tag(34);
                self.expr(expr);
            }
            FunctionExprKind::ErrorOk { expr } => {
                self.tag(35);
                self.expr(expr);
            }
            FunctionExprKind::ErrorErr { expr } => {
                self.tag(36);
                self.expr(expr);
            }
            FunctionExprKind::TaggedUnionTag { expr } => {
                self.tag(37);
                self.expr(expr);
            }
            FunctionExprKind::TaggedUnionPayload { expr } => {
                self.tag(38);
                self.expr(expr);
            }
            FunctionExprKind::Try { expr } => {
                self.tag(39);
                self.expr(expr);
            }
            FunctionExprKind::AddrOf(place) => {
                self.tag(40);
                self.place(place);
            }
            FunctionExprKind::Binary { lhs, op, rhs } => {
                self.tag(41);
                self.expr(lhs);
                self.binary(*op);
                self.expr(rhs);
            }
            FunctionExprKind::Assign { place, op, rhs } => {
                self.tag(42);
                self.place(place);
                self.assign(*op);
                self.expr(rhs);
            }
            FunctionExprKind::Discard(expr) => {
                self.tag(43);
                self.expr(expr);
            }
            FunctionExprKind::Cast { expr, ty } => {
                self.tag(44);
                self.expr(expr);
                self.ty(*ty);
            }
            FunctionExprKind::TraitObjectUpcast {
                expr,
                source_ty,
                target_ty,
            } => {
                self.tag(45);
                self.expr(expr);
                self.ty(*source_ty);
                self.ty(*target_ty);
            }
            FunctionExprKind::TraitObjectCoercion {
                expr,
                target_ty,
                self_ty,
            } => {
                self.tag(46);
                self.expr(expr);
                self.ty(*target_ty);
                self.ty(*self_ty);
            }
            FunctionExprKind::Call { callee, args } => {
                self.tag(47);
                self.callee(callee);
                self.exprs(args);
            }
            FunctionExprKind::Field { lhs, field } => {
                self.tag(48);
                self.expr(lhs);
                self.global_def(*field);
            }
            FunctionExprKind::Index { lhs, index } => {
                self.tag(49);
                self.expr(lhs);
                self.expr(index);
            }
            FunctionExprKind::Slice {
                lhs,
                range,
                is_readonly,
            } => {
                self.tag(50);
                self.expr(lhs);
                self.optional_expr(range.start.as_deref());
                self.optional_expr(range.end.as_deref());
                self.bool(range.inclusive);
                self.bool(*is_readonly);
            }
        }
    }

    fn builtin_value(&mut self, value: &FunctionBuiltinValue) {
        match value {
            FunctionBuiltinValue::Usize(value) => {
                self.tag(0);
                self.builder.write_u64(*value);
            }
            FunctionBuiltinValue::Layout { builtin, ty } => {
                self.tag(1);
                self.tag(*builtin as u8);
                self.ty(*ty);
            }
            FunctionBuiltinValue::FieldOffset { ty, field } => {
                self.tag(2);
                self.ty(*ty);
                self.global_def(*field);
            }
            FunctionBuiltinValue::Int(value) => {
                self.tag(3);
                self.int_const(*value);
            }
        }
    }

    fn inline_asm(&mut self, asm: &FunctionInlineAsm) {
        self.builder.write_str(&asm.code);
        self.len(asm.inputs.len());
        for input in &asm.inputs {
            self.builder.write_str(&input.constraint);
            self.expr(&input.value);
        }
        self.len(asm.outputs.len());
        for output in &asm.outputs {
            self.builder.write_str(&output.constraint);
            self.place(&output.place);
        }
        self.len(asm.clobbers.len());
        for clobber in &asm.clobbers {
            self.builder.write_str(clobber);
        }
        self.len(asm.options.len());
        for option in &asm.options {
            self.tag(*option as u8);
        }
    }

    fn atomic(&mut self, atomic: &FunctionAtomic) {
        match atomic {
            FunctionAtomic::Load { ty, ptr, order } => {
                self.tag(0);
                self.ty(*ty);
                self.expr(ptr);
                self.tag(*order as u8);
            }
            FunctionAtomic::Store {
                ty,
                ptr,
                value,
                order,
            } => {
                self.tag(1);
                self.ty(*ty);
                self.expr(ptr);
                self.expr(value);
                self.tag(*order as u8);
            }
            FunctionAtomic::Rmw {
                ty,
                ptr,
                op,
                value,
                order,
            } => {
                self.tag(2);
                self.ty(*ty);
                self.expr(ptr);
                self.tag(*op as u8);
                self.expr(value);
                self.tag(*order as u8);
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
                self.tag(3);
                self.ty(*ty);
                self.expr(ptr);
                self.expr(expected);
                self.expr(desired);
                self.tag(*success as u8);
                self.tag(*failure as u8);
                self.bool(*weak);
            }
            FunctionAtomic::Fence { order } => {
                self.tag(4);
                self.tag(*order as u8);
            }
        }
    }

    fn function_fields(&mut self, fields: &[FunctionFieldInit]) {
        self.len(fields.len());
        for field in fields {
            self.function_field(field);
        }
    }

    fn function_field(&mut self, field: &FunctionFieldInit) {
        self.optional_global_def(field.field);
        self.expr(&field.value);
    }

    fn callee(&mut self, callee: &FunctionCallee) {
        match callee {
            FunctionCallee::Function(def_id) => {
                self.tag(0);
                self.global_def(*def_id);
            }
            FunctionCallee::FunctionInstance {
                def_id,
                arg_module_id,
                self_arg,
                args,
                const_args,
            } => {
                self.tag(1);
                self.global_def(*def_id);
                self.module_id(*arg_module_id);
                self.optional_ty(*self_arg);
                self.types(args);
                self.const_args(const_args);
            }
            FunctionCallee::Method {
                def_id,
                arg_module_id,
                self_arg,
                args,
                receiver_kind,
                receiver,
            } => {
                self.tag(2);
                self.global_def(*def_id);
                self.module_id(*arg_module_id);
                self.optional_ty(*self_arg);
                self.types(args);
                self.receiver(*receiver_kind);
                self.expr(receiver);
            }
            FunctionCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver_kind,
                receiver,
            } => {
                self.tag(3);
                self.global_def(*trait_id);
                self.global_def(*method_id);
                self.symbol(*method_name);
                self.ty(*self_ty);
                self.types(trait_args);
                self.types(args);
                self.receiver(*receiver_kind);
                self.expr(receiver);
            }
            FunctionCallee::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
            } => {
                self.tag(4);
                self.global_def(*trait_id);
                self.global_def(*method_id);
                self.symbol(*method_name);
                self.ty(*self_ty);
                self.types(trait_args);
                self.types(args);
            }
            FunctionCallee::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                slot,
                params,
                return_type,
                receiver_kind,
                receiver,
            } => {
                self.tag(5);
                self.ty(*object_ty);
                self.trait_id(*trait_id);
                self.global_def(*method_id);
                self.symbol(*method_name);
                self.types(trait_args);
                self.usize(*slot);
                self.types(params);
                self.ty(*return_type);
                self.receiver(*receiver_kind);
                self.expr(receiver);
            }
            FunctionCallee::BuiltinMethod {
                method,
                self_ty,
                receiver,
            } => {
                self.tag(6);
                self.tag(*method as u8);
                self.ty(*self_ty);
                self.expr(receiver);
            }
            FunctionCallee::BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
                receiver,
            } => {
                self.tag(7);
                self.tag(*trait_id as u8);
                self.tag(*method as u8);
                self.ty(*self_ty);
                self.types(trait_args);
                self.expr(receiver);
            }
            FunctionCallee::BuiltinOperator(operator) => {
                self.tag(8);
                self.tag(operator.trait_id as u8);
                match operator.op {
                    FunctionBuiltinOperatorOp::Unary(op) => {
                        self.tag(0);
                        self.unary(op);
                    }
                    FunctionBuiltinOperatorOp::Binary(op) => {
                        self.tag(1);
                        self.binary(op);
                    }
                }
            }
            FunctionCallee::FunctionPointer(expr) => {
                self.tag(9);
                self.expr(expr);
            }
        }
    }

    fn place(&mut self, place: &FunctionPlace) {
        self.ty(place.ty);
        match &place.base {
            FunctionPlaceBase::Local(local) => {
                self.tag(0);
                self.local(*local);
            }
            FunctionPlaceBase::Global(def_id) => {
                self.tag(1);
                self.global_def(*def_id);
            }
            FunctionPlaceBase::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => {
                self.tag(2);
                self.global_def(*def_id);
                self.module_id(*arg_module_id);
                self.types(args);
                self.const_args(const_args);
            }
            FunctionPlaceBase::Deref(expr) => {
                self.tag(3);
                self.expr(expr);
            }
            FunctionPlaceBase::Error => self.tag(4),
        }
        self.len(place.elems.len());
        for elem in &place.elems {
            match elem {
                FunctionPlaceElem::Field(def_id) => {
                    self.tag(0);
                    self.global_def(*def_id);
                }
                FunctionPlaceElem::Index(expr) => {
                    self.tag(1);
                    self.expr(expr);
                }
                FunctionPlaceElem::Error => self.tag(2),
            }
        }
    }

    fn ty(&mut self, ty: InternedTyId) {
        let kind = self.index.ty_kind(ty).unwrap_or_else(|| {
            panic!("Nia ICE: codegen fingerprint references missing type {ty:?}")
        });
        self.ty_kind(kind);
        self.optional_type_layout(self.index.type_layout(ty));
    }

    fn ty_kind(&mut self, kind: &TyKind) {
        match kind {
            TyKind::Error => self.tag(0),
            TyKind::ConstOnly => self.tag(1),
            TyKind::Primitive(primitive) => {
                self.tag(2);
                self.tag(*primitive as u8);
            }
            TyKind::Pointer { is_readonly, elem } => {
                self.tag(3);
                self.bool(*is_readonly);
                self.ty(*elem);
            }
            TyKind::VolatilePointer { is_readonly, elem } => {
                self.tag(4);
                self.bool(*is_readonly);
                self.ty(*elem);
            }
            TyKind::Slice { is_readonly, elem } => {
                self.tag(5);
                self.bool(*is_readonly);
                self.ty(*elem);
            }
            TyKind::SlicePointee { elem } => {
                self.tag(6);
                self.ty(*elem);
            }
            TyKind::Array { len, elem } => {
                self.tag(7);
                self.array_len(len);
                self.ty(*elem);
            }
            TyKind::Vector { elem, lanes } => {
                self.tag(8);
                self.tag(*elem as u8);
                self.u32(*lanes);
            }
            TyKind::Range { kind, bound } => {
                self.tag(9);
                self.tag(*kind as u8);
                self.optional_ty(*bound);
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                self.tag(10);
                self.types(params);
                self.ty(*return_type);
                self.bool(*is_variadic);
            }
            TyKind::Optional { elem } => {
                self.tag(11);
                self.ty(*elem);
            }
            TyKind::ErrorUnion { error, value } => {
                self.tag(12);
                self.ty(*error);
                self.ty(*value);
            }
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => {
                self.tag(13);
                self.global_def(*def_id);
                self.types(args);
                self.const_args(const_args);
            }
            TyKind::BuiltinType(builtin) => {
                self.tag(14);
                self.tag(*builtin as u8);
            }
            TyKind::BuiltinTrait { trait_id, args } => {
                self.tag(15);
                self.tag(*trait_id as u8);
                self.types(args);
            }
            TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => {
                self.tag(16);
                self.bool(*is_readonly);
                self.trait_object(
                    *trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                );
            }
            TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => {
                self.tag(17);
                self.trait_object(
                    *trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                );
            }
            TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            } => {
                self.tag(18);
                self.ty(*self_ty);
                self.trait_id(*trait_id);
                self.types(trait_args);
                self.const_args(trait_const_args);
                self.symbol(*name);
            }
            TyKind::GenericParam(name) => {
                self.tag(19);
                self.symbol(*name);
            }
            TyKind::SelfParam => self.tag(20),
        }
    }

    fn trait_object(
        &mut self,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
        bindings: &[AssociatedTypeBindingTy],
    ) {
        self.trait_id(trait_id);
        self.types(trait_args);
        self.const_args(trait_const_args);
        self.len(bindings.len());
        for binding in bindings {
            match binding.trait_id {
                Some(trait_id) => {
                    self.tag(1);
                    self.trait_id(trait_id);
                }
                None => self.tag(0),
            }
            self.types(&binding.trait_args);
            self.const_args(&binding.trait_const_args);
            self.symbol(binding.name);
            self.ty(binding.ty);
        }
    }

    fn trait_id(&mut self, trait_id: TraitId) {
        match trait_id {
            TraitId::Source(def_id) => {
                self.tag(0);
                self.global_def(def_id);
            }
            TraitId::Builtin(builtin) => {
                self.tag(1);
                self.tag(builtin as u8);
            }
        }
    }

    fn types(&mut self, types: &[InternedTyId]) {
        self.len(types.len());
        for ty in types {
            self.ty(*ty);
        }
    }

    fn const_args(&mut self, args: &[ConstGenericArg]) {
        self.len(args.len());
        for arg in args {
            self.const_arg(arg);
        }
    }

    fn const_arg(&mut self, arg: &ConstGenericArg) {
        self.ty(arg.ty);
        match &arg.value {
            ConstGenericValue::GenericParam(name) => {
                self.tag(0);
                self.symbol(*name);
            }
            ConstGenericValue::ConstExpr(expr) => {
                self.tag(1);
                self.global_const_expr(*expr);
            }
            ConstGenericValue::Int(value) => {
                self.tag(2);
                self.int_const(*value);
            }
            ConstGenericValue::Bool(value) => {
                self.tag(3);
                self.bool(*value);
            }
            ConstGenericValue::Char(value) => {
                self.tag(4);
                self.u32(u32::from(*value));
            }
        }
    }

    fn array_len(&mut self, len: &ArrayLenTy) {
        match len {
            ArrayLenTy::Infer => self.tag(0),
            ArrayLenTy::GenericParam(name) => {
                self.tag(1);
                self.symbol(*name);
            }
            ArrayLenTy::ConstValue(value) => {
                self.tag(2);
                self.builder.write_u64(*value);
            }
            ArrayLenTy::ConstExpr(expr) => {
                self.tag(3);
                self.global_const_expr(*expr);
            }
            ArrayLenTy::Builtin { builtin, ty } => {
                self.tag(4);
                self.tag(*builtin as u8);
                self.ty(*ty);
            }
        }
    }

    fn optional_static_init(&mut self, init: Option<&StaticInit>) {
        match init {
            Some(init) => {
                self.tag(1);
                self.static_init(init);
            }
            None => self.tag(0),
        }
    }

    fn static_init(&mut self, init: &StaticInit) {
        match init {
            StaticInit::Zero => self.tag(0),
            StaticInit::Int(value) => {
                self.tag(1);
                self.int_const(*value);
            }
            StaticInit::Float(value) => {
                self.tag(2);
                self.builder.write_str(value);
            }
            StaticInit::Bool(value) => {
                self.tag(3);
                self.bool(*value);
            }
            StaticInit::Char(value) => {
                self.tag(4);
                self.u32(*value);
            }
            StaticInit::Byte(value) => {
                self.tag(5);
                self.tag(*value);
            }
            StaticInit::Chars(values) => {
                self.tag(6);
                self.u32s(values);
            }
            StaticInit::Bytes(values) => {
                self.tag(7);
                self.builder.write_bytes(values);
            }
            StaticInit::Array(values) => {
                self.tag(8);
                self.len(values.len());
                for value in values {
                    self.static_init(value);
                }
            }
            StaticInit::Repeat { value, count } => {
                self.tag(9);
                self.static_init(value);
                self.builder.write_u64(*count);
            }
            StaticInit::Struct(fields) => {
                self.tag(10);
                self.len(fields.len());
                for field in fields {
                    self.optional_global_def(field.field);
                    self.static_init(&field.value);
                }
            }
            StaticInit::NullPtr => self.tag(11),
            StaticInit::AddrOfGlobal { global, path } => {
                self.tag(12);
                self.global_def(*global);
                self.len(path.len());
                for elem in path {
                    match elem {
                        StaticAddressElem::Field(field) => {
                            self.tag(0);
                            self.global_def(*field);
                        }
                        StaticAddressElem::Index(index) => {
                            self.tag(1);
                            self.builder.write_u64(*index);
                        }
                        StaticAddressElem::Error => self.tag(2),
                    }
                }
            }
            StaticInit::AddrOfFunction { function, args } => {
                self.tag(13);
                self.global_def(*function);
                self.types(args);
            }
            StaticInit::StaticArrayPointer {
                array_ty,
                array_init,
            } => {
                self.tag(14);
                self.ty(*array_ty);
                self.static_init(array_init);
            }
        }
    }

    fn optional_struct_layout(&mut self, layout: Option<&StructLayout>) {
        match layout {
            Some(layout) => {
                self.tag(1);
                self.type_layout(&layout.layout);
                self.len(layout.fields.len());
                for field in &layout.fields {
                    self.builder.write_u64(field.def_id.0);
                    self.builder.write_u64(field.offset);
                    self.type_layout(&field.layout);
                }
            }
            None => self.tag(0),
        }
    }

    fn optional_type_layout(&mut self, layout: Option<&TypeLayout>) {
        match layout {
            Some(layout) => {
                self.tag(1);
                self.type_layout(layout);
            }
            None => self.tag(0),
        }
    }

    fn type_layout(&mut self, layout: &TypeLayout) {
        self.builder.write_u64(layout.size);
        self.builder.write_u64(layout.align);
    }

    fn int_const(&mut self, value: IntConst) {
        self.u128(value.bits());
        self.bool(value.is_signed());
    }

    fn optional_expr(&mut self, expr: Option<&FunctionExpr>) {
        match expr {
            Some(expr) => {
                self.tag(1);
                self.expr(expr);
            }
            None => self.tag(0),
        }
    }

    fn exprs(&mut self, exprs: &[FunctionExpr]) {
        self.len(exprs.len());
        for expr in exprs {
            self.expr(expr);
        }
    }

    fn symbols(&mut self, symbols: &[nia_symbol::SymbolId]) {
        self.len(symbols.len());
        for symbol in symbols {
            self.symbol(*symbol);
        }
    }

    fn optional_str(&mut self, value: Option<&str>) {
        match value {
            Some(value) => {
                self.tag(1);
                self.builder.write_str(value);
            }
            None => self.tag(0),
        }
    }

    fn optional_i128(&mut self, value: Option<i128>) {
        match value {
            Some(value) => {
                self.tag(1);
                self.u128(value as u128);
            }
            None => self.tag(0),
        }
    }

    fn optional_ty(&mut self, ty: Option<InternedTyId>) {
        match ty {
            Some(ty) => {
                self.tag(1);
                self.ty(ty);
            }
            None => self.tag(0),
        }
    }

    fn optional_global_def(&mut self, def_id: Option<GlobalDefId>) {
        match def_id {
            Some(def_id) => {
                self.tag(1);
                self.global_def(def_id);
            }
            None => self.tag(0),
        }
    }

    fn optional_local(&mut self, local: Option<nia_ids::LocalId>) {
        match local {
            Some(local) => {
                self.tag(1);
                self.local(local);
            }
            None => self.tag(0),
        }
    }

    fn optional_symbol(&mut self, symbol: Option<nia_symbol::SymbolId>) {
        match symbol {
            Some(symbol) => {
                self.tag(1);
                self.symbol(symbol);
            }
            None => self.tag(0),
        }
    }

    fn optional_receiver(&mut self, receiver: Option<ReceiverKind>) {
        match receiver {
            Some(receiver) => {
                self.tag(1);
                self.receiver(receiver);
            }
            None => self.tag(0),
        }
    }

    fn receiver(&mut self, receiver: ReceiverKind) {
        self.tag(receiver as u8);
    }

    fn local(&mut self, local: nia_ids::LocalId) {
        self.u32(local.0);
    }

    fn block_id(&mut self, block: FunctionBlockId) {
        self.u32(block.0);
    }

    fn optional_block(&mut self, block: Option<FunctionBlockId>) {
        match block {
            Some(block) => {
                self.tag(1);
                self.block_id(block);
            }
            None => self.tag(0),
        }
    }

    fn scope_id(&mut self, scope: FunctionScopeId) {
        self.u32(scope.0);
    }

    fn u32s(&mut self, values: &[u32]) {
        self.len(values.len());
        for value in values {
            self.u32(*value);
        }
    }

    fn unary(&mut self, op: UnaryOp) {
        self.tag(op as u8);
    }

    fn binary(&mut self, op: BinaryOp) {
        self.tag(op as u8);
    }

    fn assign(&mut self, op: AssignOp) {
        self.tag(op as u8);
    }
}

fn write_optimization(builder: &mut QueryFingerprintBuilder, policy: OptimizationPolicy) {
    builder.write_u8(match policy.level {
        NiaOptimizationLevel::O0 => 0,
        NiaOptimizationLevel::O1 => 1,
        NiaOptimizationLevel::O2 => 2,
        NiaOptimizationLevel::O3 => 3,
        NiaOptimizationLevel::Os => 4,
        NiaOptimizationLevel::Oz => 5,
    });
    for depth in [
        policy.simplify_cfg,
        policy.const_fold,
        policy.dead_code_elim,
        policy.local_copy_prop,
    ] {
        builder.write_u8(match depth {
            OptimizationDepth::Disabled => 0,
            OptimizationDepth::Required => 1,
            OptimizationDepth::Cheap => 2,
            OptimizationDepth::Full => 3,
            OptimizationDepth::Aggressive => 4,
        });
    }
    builder.write_u8(match policy.inline_threshold {
        InlineThreshold::Never => 0,
        InlineThreshold::Minimal => 1,
        InlineThreshold::Size => 2,
        InlineThreshold::Small => 3,
        InlineThreshold::Normal => 4,
        InlineThreshold::Aggressive => 5,
    });
    builder.write_u8(match policy.specialize_generics {
        SpecializationPolicy::RequiredOnly => 0,
        SpecializationPolicy::SizeAware => 1,
        SpecializationPolicy::Normal => 2,
        SpecializationPolicy::Aggressive => 3,
    });
    builder.write_u8(u8::from(policy.dedup_monomorphized_instances));
    builder.write_u8(u8::from(policy.prefer_size));
}

fn write_target_identity(builder: &mut QueryFingerprintBuilder, identity: &TargetMachineIdentity) {
    builder.write_str(&identity.triple);
    builder.write_str(&identity.cpu);
    builder.write_str(&identity.features);
}

const fn llvm_sys_version() -> u64 {
    nia_llvm::CODEGEN_ABI_VERSION
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nia_backend_lower::{BackendLowering, BackendOptimizationReport};
    use nia_ids::{DefId, GlobalDefId, ModuleIdAllocator};
    use nia_layout::{TargetDataLayout, TypeLayout};
    use nia_source::SourceIdentity;
    use nia_span::Span;
    use nia_symbol::SymbolId;
    use nia_ty::{PrimitiveTy, TypeStore};

    use super::*;

    struct Fixture {
        index: ProgramIndex,
        partition: CodegenPartition,
    }

    fn module_with_global(
        module_id: ModuleId,
        identity: &str,
        ty: InternedTyId,
        init: i128,
    ) -> BackendModule {
        BackendModule {
            id: module_id,
            source_identity: SourceIdentity::new(identity),
            name: identity.to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: TargetDataLayout::LP64,
                types: vec![(ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: vec![BackendGlobal {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                },
                name: SymbolId::EMPTY,
                link_name: None,
                ty,
                is_let: true,
                is_extern: false,
                init: Some(StaticInit::Int(IntConst::signed(init))),
                span: Span::default(),
            }],
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }
    }

    fn declaration_module(
        module_id: ModuleId,
        identity: &str,
        types: Vec<(InternedTyId, TypeLayout)>,
        return_type: InternedTyId,
    ) -> BackendModule {
        BackendModule {
            id: module_id,
            source_identity: SourceIdentity::new(identity),
            name: identity.to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: TargetDataLayout::LP64,
                types,
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            unions: Vec::new(),
            struct_instances: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: vec![BackendFunction {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                },
                name: SymbolId::EMPTY,
                link_name: Some("foreign_value".to_string()),
                generics: Vec::new(),
                params: Vec::new(),
                return_type,
                is_extern: true,
                is_variadic: false,
                attributes: Vec::new(),
                local_names: Default::default(),
                function_body: None,
                span: Span::default(),
            }],
            function_instances: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        }
    }

    fn fixture(program: BackendProgram, type_store: TypeStore, owner: &str) -> Fixture {
        let plan = program.codegen_partition_plan();
        let partition = plan
            .partitions()
            .iter()
            .find(|partition| {
                matches!(
                    &partition.key,
                    CodegenUnitKey::SourceModule { source_identity, .. }
                        if source_identity.normalized_path() == owner
                )
            })
            .expect("owner partition")
            .clone();
        let lowering = Arc::new(BackendLowering {
            program,
            codegen_partitions: plan,
            optimization: OptimizationPolicy::default(),
            optimization_report: BackendOptimizationReport::default(),
            diagnostics: Vec::new(),
        });
        Fixture {
            index: ProgramIndex::new(lowering, Arc::new(type_store)),
            partition,
        }
    }

    fn ir_fingerprints(
        fixture: &Fixture,
        options: LlvmCodegenOptions,
    ) -> CodegenUnitFingerprintSet {
        source_unit_fingerprint(
            &fixture.partition,
            &fixture.index,
            options,
            ArtifactTarget::LlvmIr,
        )
    }

    fn ir_fingerprint(fixture: &Fixture, options: LlvmCodegenOptions) -> CodegenUnitFingerprint {
        ir_fingerprints(fixture, options).fingerprint
    }

    #[test]
    fn source_unit_fingerprint_ignores_session_local_handle_allocation() {
        let mut first_ids = ModuleIdAllocator::new();
        let first_id = first_ids.allocate();
        let first_store = TypeStore::new();
        let first_ty = first_store
            .append_for_module(first_id)
            .primitive(PrimitiveTy::I32);
        let first = fixture(
            BackendProgram {
                modules: vec![module_with_global(first_id, "main.nia", first_ty, 1)],
            },
            first_store,
            "main.nia",
        );

        let mut second_ids = ModuleIdAllocator::new();
        let _unrelated = second_ids.allocate();
        let second_id = second_ids.allocate();
        let second_store = TypeStore::new();
        let _unrelated_ty = second_store
            .append_for_module(second_id)
            .primitive(PrimitiveTy::U8);
        let second_ty = second_store
            .append_for_module(second_id)
            .primitive(PrimitiveTy::I32);
        let second = fixture(
            BackendProgram {
                modules: vec![module_with_global(second_id, "main.nia", second_ty, 1)],
            },
            second_store,
            "main.nia",
        );

        assert_eq!(
            ir_fingerprint(&first, LlvmCodegenOptions::default()),
            ir_fingerprint(&second, LlvmCodegenOptions::default())
        );
    }

    #[test]
    fn source_unit_fingerprint_is_independent_of_module_input_order() {
        let mut ids = ModuleIdAllocator::new();
        let main_id = ids.allocate();
        let helper_id = ids.allocate();
        let store = TypeStore::new();
        let ty = store.append_for_module(main_id).primitive(PrimitiveTy::I32);
        let main = module_with_global(main_id, "main.nia", ty, 1);
        let helper = module_with_global(helper_id, "helper.nia", ty, 2);
        let first = fixture(
            BackendProgram {
                modules: vec![main, helper],
            },
            store,
            "main.nia",
        );

        let mut ids = ModuleIdAllocator::new();
        let main_id = ids.allocate();
        let helper_id = ids.allocate();
        let store = TypeStore::new();
        let ty = store.append_for_module(main_id).primitive(PrimitiveTy::I32);
        let second = fixture(
            BackendProgram {
                modules: vec![
                    module_with_global(helper_id, "helper.nia", ty, 2),
                    module_with_global(main_id, "main.nia", ty, 1),
                ],
            },
            store,
            "main.nia",
        );

        assert_eq!(
            ir_fingerprint(&first, LlvmCodegenOptions::default()),
            ir_fingerprint(&second, LlvmCodegenOptions::default())
        );
    }

    #[test]
    fn source_unit_fingerprint_tracks_definition_and_ignores_span() {
        let mut ids = ModuleIdAllocator::new();
        let module_id = ids.allocate();
        let store = TypeStore::new();
        let ty = store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);
        let baseline = fixture(
            BackendProgram {
                modules: vec![module_with_global(module_id, "main.nia", ty, 1)],
            },
            store,
            "main.nia",
        );

        let mut ids = ModuleIdAllocator::new();
        let module_id = ids.allocate();
        let store = TypeStore::new();
        let ty = store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);
        let mut span_only_module = module_with_global(module_id, "main.nia", ty, 1);
        span_only_module.globals[0].span = Span::new(100, 200);
        let span_only = fixture(
            BackendProgram {
                modules: vec![span_only_module],
            },
            store,
            "main.nia",
        );

        let mut ids = ModuleIdAllocator::new();
        let module_id = ids.allocate();
        let store = TypeStore::new();
        let ty = store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);
        let changed = fixture(
            BackendProgram {
                modules: vec![module_with_global(module_id, "main.nia", ty, 2)],
            },
            store,
            "main.nia",
        );

        let baseline = ir_fingerprints(&baseline, LlvmCodegenOptions::default());
        assert_eq!(
            baseline,
            ir_fingerprints(&span_only, LlvmCodegenOptions::default())
        );
        let changed = ir_fingerprints(&changed, LlvmCodegenOptions::default());
        assert_ne!(baseline.fingerprint, changed.fingerprint);
        assert_ne!(
            baseline.components.definition,
            changed.components.definition
        );
        assert_eq!(baseline.components.policy, changed.components.policy);
        assert_eq!(
            baseline.components.declarations,
            changed.components.declarations
        );
        assert_eq!(baseline.components.target, changed.components.target);
    }

    #[test]
    fn source_unit_fingerprint_tracks_cross_module_abi_and_optimization() {
        fn make(return_ty: PrimitiveTy) -> Fixture {
            let mut ids = ModuleIdAllocator::new();
            let main_id = ids.allocate();
            let foreign_id = ids.allocate();
            let store = TypeStore::new();
            let append = store.append_for_module(main_id);
            let i32_ty = append.primitive(PrimitiveTy::I32);
            let i64_ty = append.primitive(PrimitiveTy::I64);
            let selected = match return_ty {
                PrimitiveTy::I32 => i32_ty,
                PrimitiveTy::I64 => i64_ty,
                _ => unreachable!(),
            };
            fixture(
                BackendProgram {
                    modules: vec![
                        module_with_global(main_id, "main.nia", i32_ty, 1),
                        declaration_module(
                            foreign_id,
                            "foreign.nia",
                            vec![
                                (i32_ty, TypeLayout { size: 4, align: 4 }),
                                (i64_ty, TypeLayout { size: 8, align: 8 }),
                            ],
                            selected,
                        ),
                    ],
                },
                store,
                "main.nia",
            )
        }

        let baseline = make(PrimitiveTy::I32);
        let changed_abi = make(PrimitiveTy::I64);
        let baseline_fingerprints = ir_fingerprints(&baseline, LlvmCodegenOptions::default());
        let changed_abi = ir_fingerprints(&changed_abi, LlvmCodegenOptions::default());
        assert_ne!(baseline_fingerprints.fingerprint, changed_abi.fingerprint);
        assert_eq!(
            baseline_fingerprints.components.definition,
            changed_abi.components.definition
        );
        assert_ne!(
            baseline_fingerprints.components.declarations,
            changed_abi.components.declarations
        );
        let changed_optimization = ir_fingerprints(
            &baseline,
            LlvmCodegenOptions {
                optimization: NiaOptimizationLevel::O2.policy(),
                ..LlvmCodegenOptions::default()
            },
        );
        assert_ne!(
            baseline_fingerprints.fingerprint,
            changed_optimization.fingerprint
        );
        assert_ne!(
            baseline_fingerprints.components.policy,
            changed_optimization.components.policy
        );
        assert_eq!(
            baseline_fingerprints.components.definition,
            changed_optimization.components.definition
        );
        assert_eq!(
            baseline_fingerprints.components.declarations,
            changed_optimization.components.declarations
        );
        assert_eq!(
            baseline_fingerprints.components.target,
            changed_optimization.components.target
        );
    }

    #[test]
    fn native_object_fingerprint_tracks_exact_target_identity() {
        let mut ids = ModuleIdAllocator::new();
        let module_id = ids.allocate();
        let store = TypeStore::new();
        let ty = store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);
        let fixture = fixture(
            BackendProgram {
                modules: vec![module_with_global(module_id, "main.nia", ty, 1)],
            },
            store,
            "main.nia",
        );
        let target = TargetMachineIdentity {
            triple: "x86_64-unknown-linux-gnu".to_string(),
            cpu: "generic".to_string(),
            features: "+sse2".to_string(),
        };
        let changed_target = TargetMachineIdentity {
            features: "+sse2,+avx2".to_string(),
            ..target.clone()
        };

        let baseline = source_unit_fingerprint(
            &fixture.partition,
            &fixture.index,
            LlvmCodegenOptions::default(),
            ArtifactTarget::NativeObject(&target),
        );
        let changed = source_unit_fingerprint(
            &fixture.partition,
            &fixture.index,
            LlvmCodegenOptions::default(),
            ArtifactTarget::NativeObject(&changed_target),
        );
        assert_ne!(baseline.fingerprint, changed.fingerprint);
        assert_ne!(baseline.components.target, changed.components.target);
        assert_eq!(baseline.components.policy, changed.components.policy);
        assert_eq!(
            baseline.components.definition,
            changed.components.definition
        );
        assert_eq!(
            baseline.components.declarations,
            changed.components.declarations
        );
    }
}
