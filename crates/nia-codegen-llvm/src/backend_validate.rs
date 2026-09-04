// SPDX-License-Identifier: GPL-3.0-or-later
mod aggregates;
mod function_ir;
mod layouts;
mod refs;
mod static_init;
mod types;

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
};

use crate::{
    compiler_builtins::CompilerBuiltinSymbols,
    declaration_membership::CodegenDeclarationMembership, program_index::ProgramIndex,
};
use nia_backend_ir::{
    BackendClosureEntry, BackendClosureEntryKey, BackendClosureEntryOwner, BackendFunction,
    BackendFunctionInstance, BackendGlobal, BackendGlobalInstance, BackendModule, BackendParam,
    BackendStruct, BackendStructInstance, BackendTraitObjectVtable,
    BackendTraitObjectVtableFunction, BackendUnion, BackendUnionInstance, CodegenPartition,
};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{FunctionBody, FunctionInstanceKey, FunctionLocalKind};
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_layout::{TargetDataLayout, TypeLayout};
use nia_mangle::{
    MangleModuleId, MangleResolvers, mangle_base_symbol_id, mangle_closure_entry_symbol,
    mangle_instance_symbol_id, mangle_symbol_id, mangle_type_with,
};
use nia_symbol::SymbolId;
use nia_ty::{ArrayLenTy, ConstGenericArg, PrimitiveTy, TyKind};

#[derive(Clone, Copy)]
struct FunctionInstanceRef<'a> {
    def_id: GlobalDefId,
    arg_module_id: ModuleId,
    self_arg: Option<InternedTyId>,
    args: &'a [InternedTyId],
    const_args: &'a [ConstGenericArg],
}
type AggregateInstanceKey = (GlobalDefId, Vec<InternedTyId>, Vec<ConstGenericArg>);
type AggregateFieldsLookup = RefCell<HashMap<AggregateInstanceKey, Option<Vec<InternedTyId>>>>;

#[derive(Clone, Copy)]
struct ExternalSymbolState {
    kind: &'static str,
    has_definition: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExternAbiTypeContext {
    StructField,
    FunctionParameter,
    FunctionReturn,
    FunctionPointerParameter,
    FunctionPointerReturn,
    Global,
}

impl ExternAbiTypeContext {
    fn description(self) -> &'static str {
        match self {
            Self::StructField => "extern struct field",
            Self::FunctionParameter => "extern parameter",
            Self::FunctionReturn => "extern return type",
            Self::FunctionPointerParameter => "extern function pointer parameter",
            Self::FunctionPointerReturn => "extern function pointer return type",
            Self::Global => "extern global",
        }
    }

    fn permits_unit(self) -> bool {
        matches!(self, Self::FunctionReturn | Self::FunctionPointerReturn)
    }
}

pub(super) fn backend_symbol_debug_name(name: SymbolId) -> String {
    mangle_symbol_id(name)
}

pub(super) fn validate_backend_program(index: &ProgramIndex) -> Vec<Diagnostic> {
    let targets = index.module_target_layouts().collect::<Vec<_>>();
    let mut diagnostics = Vec::new();
    if targets
        .iter()
        .any(|target| !layouts::target_layout_supported(*target))
    {
        diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            "backend IR target data layout has unsupported pointer size or alignment",
        ));
    }
    if targets
        .first()
        .is_some_and(|first| targets.iter().any(|target| target != first))
    {
        diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            "backend IR modules disagree on the artifact target data layout",
        ));
    }
    validate_generated_symbols(index, &mut diagnostics);
    diagnostics
}

pub(super) fn validate_native_backend_program(
    index: &ProgramIndex,
    builtin_symbols: CompilerBuiltinSymbols,
) -> Vec<Diagnostic> {
    let mut diagnostics = validate_backend_program(index);
    let reserved = builtin_symbols
        .external_definitions()
        .collect::<HashSet<_>>();
    if reserved.is_empty() {
        return diagnostics;
    }
    for module_id in index.module_ids() {
        let Some(module) = index.module(*module_id) else {
            continue;
        };
        for function in &module.functions {
            if function.is_extern
                && let Some(symbol) = function.link_name.as_deref()
                && reserved.contains(symbol)
            {
                diagnostics.push(compiler_builtin_collision_diagnostic(
                    "extern function",
                    symbol,
                    function.span,
                ));
            }
        }
        for global in &module.globals {
            if global.is_extern
                && let Some(symbol) = global.link_name.as_deref()
                && reserved.contains(symbol)
            {
                diagnostics.push(compiler_builtin_collision_diagnostic(
                    "extern global",
                    symbol,
                    global.span,
                ));
            }
        }
    }
    diagnostics
}

fn compiler_builtin_collision_diagnostic(
    kind: &'static str,
    symbol: &str,
    span: nia_span::Span,
) -> Diagnostic {
    Diagnostic::internal_error_at(
        nia_diagnostic::codes::INVALID_BACKEND_IR,
        span,
        format!(
            "backend IR external symbol collision: {kind} reuses `{symbol}` already owned by compiler builtin"
        ),
    )
}

fn validate_generated_symbols(index: &ProgramIndex, diagnostics: &mut Vec<Diagnostic>) {
    let mut values = HashMap::<String, &'static str>::new();
    for module_id in index.module_ids() {
        let Some(module) = index.module(*module_id) else {
            continue;
        };
        let module_mangle =
            MangleModuleId::from_normalized_source_path(module.source_identity.normalized_path());
        for function in &module.functions {
            if function.is_extern || !function.generics.is_empty() {
                continue;
            }
            record_generated_symbol(
                &mut values,
                diagnostics,
                mangle_base_symbol_id(function.def_id, module_mangle, function.name),
                "function",
                function.span,
            );
        }
        for global in &module.globals {
            if !global.is_extern {
                record_generated_symbol(
                    &mut values,
                    diagnostics,
                    mangle_base_symbol_id(global.def_id, module_mangle, global.name),
                    "global",
                    global.span,
                );
            }
        }
        for function in &module.function_instances {
            record_generated_symbol(
                &mut values,
                diagnostics,
                function.symbol.clone(),
                "function instance",
                function.span,
            );
        }
        for global in &module.global_instances {
            record_generated_symbol(
                &mut values,
                diagnostics,
                global.symbol.clone(),
                "global instance",
                global.span,
            );
        }
        for entry in &module.closure_entries {
            values
                .entry(entry.symbol.clone())
                .or_insert("closure entry");
        }
        let mut types = HashMap::<String, &'static str>::new();
        for item in &module.structs {
            if item.generics.is_empty() {
                record_generated_symbol(
                    &mut types,
                    diagnostics,
                    mangle_base_symbol_id(item.def_id, module_mangle, item.name),
                    "struct",
                    item.span,
                );
            }
        }
        for item in &module.unions {
            if item.generics.is_empty() {
                record_generated_symbol(
                    &mut types,
                    diagnostics,
                    mangle_base_symbol_id(item.def_id, module_mangle, item.name),
                    "union",
                    item.span,
                );
            }
        }
        for item in &module.struct_instances {
            record_generated_symbol(
                &mut types,
                diagnostics,
                item.symbol.clone(),
                "struct instance",
                item.span,
            );
        }
        for item in &module.union_instances {
            record_generated_symbol(
                &mut types,
                diagnostics,
                item.symbol.clone(),
                "union instance",
                item.span,
            );
        }
    }
    record_vtable_symbols(index, &mut values, diagnostics);
    validate_external_symbol_collisions(index, &values, diagnostics);
}

fn validate_external_symbol_collisions(
    index: &ProgramIndex,
    generated: &HashMap<String, &'static str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut external = HashMap::<String, ExternalSymbolState>::new();
    for module_id in index.module_ids() {
        let Some(module) = index.module(*module_id) else {
            continue;
        };
        for function in &module.functions {
            if function.is_extern
                && let Some(symbol) = function.link_name.as_deref()
            {
                record_external_symbol(
                    generated,
                    &mut external,
                    diagnostics,
                    symbol,
                    "extern function",
                    function.function_body.is_some(),
                    function.span,
                );
            }
        }
        for global in &module.globals {
            if global.is_extern
                && let Some(symbol) = global.link_name.as_deref()
            {
                record_external_symbol(
                    generated,
                    &mut external,
                    diagnostics,
                    symbol,
                    "extern global",
                    false,
                    global.span,
                );
            }
        }
    }
}

fn record_external_symbol(
    generated: &HashMap<String, &'static str>,
    external: &mut HashMap<String, ExternalSymbolState>,
    diagnostics: &mut Vec<Diagnostic>,
    symbol: &str,
    kind: &'static str,
    is_definition: bool,
    span: nia_span::Span,
) {
    if symbol.is_empty() || symbol.contains('\0') {
        return;
    }
    if let Some(generated_kind) = generated.get(symbol) {
        diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!(
                "backend IR external symbol collision: {kind} reuses `{symbol}` already owned by {generated_kind}"
            ),
        ));
    }
    if let Some(previous) = external.get_mut(symbol) {
        if previous.kind != kind {
            diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!(
                    "backend IR external symbol collision: {kind} reuses `{symbol}` already declared by {}",
                    previous.kind
                ),
            ));
        } else if is_definition && previous.has_definition {
            diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!(
                    "backend IR external symbol collision: {kind} defines `{symbol}` more than once"
                ),
            ));
        } else {
            previous.has_definition |= is_definition;
        }
    } else {
        external.insert(
            symbol.to_string(),
            ExternalSymbolState {
                kind,
                has_definition: is_definition,
            },
        );
    }
}

/// Reproduces `ModuleCodegen::trait_object_vtable_symbol` for program preflight.
///
/// Vtable globals are externally visible, so their names belong to the same
/// linker namespace as ordinary functions, globals, instances, and closure
/// entries. This helper must stay byte-identical to the emitter, including its
/// const-expression resolver: the emitted name is what the linker sees, not
/// the semantically richer instance mangling. Returns `None` when a
/// mangle input is absent from Backend IR so preflight defers instead of
/// reserving a guessed name.
fn expected_trait_object_vtable_symbol(
    index: &ProgramIndex,
    key: &nia_backend_ir::BackendTraitObjectVtableKey,
) -> Option<String> {
    let missing = Cell::new(false);
    let mangle_ty = |ty: InternedTyId| {
        if index.type_store().get(ty).is_none() {
            missing.set(true);
            return String::new();
        }
        mangle_type_with(
            index.type_store(),
            ty,
            MangleResolvers::new(
                |module_id| {
                    index
                        .module(module_id)
                        .map(|module| {
                            MangleModuleId::from_normalized_source_path(
                                module.source_identity.normalized_path(),
                            )
                        })
                        .unwrap_or_else(|| {
                            missing.set(true);
                            MangleModuleId::from_normalized_source_path("")
                        })
                },
                |def_id| {
                    index
                        .struct_item(def_id)
                        .map(|item| mangle_symbol_id(item.name))
                        .or_else(|| {
                            index
                                .union_item(def_id)
                                .map(|item| mangle_symbol_id(item.name))
                        })
                        .or_else(|| {
                            index
                                .enum_item(def_id)
                                .map(|item| mangle_symbol_id(item.name))
                        })
                        .or_else(|| {
                            index
                                .function(def_id)
                                .map(|item| mangle_symbol_id(item.name))
                        })
                        .unwrap_or_else(|| format!("def{}", def_id.def_id.0))
                },
                |const_expr: nia_ids::GlobalConstExprId| {
                    index.module(const_expr.module_id).and_then(|module| {
                        module.const_eval.array_lengths.get(&const_expr).copied()
                    })
                },
            ),
        )
    };
    let self_part = mangle_ty(key.self_ty);
    let object_part = mangle_ty(key.object_ty);
    (!missing.get()).then(|| format!("nia__vtable__{self_part}__as__{object_part}"))
}

/// Reserves compiler-generated trait-object vtable symbols in the program value
/// namespace.
///
/// One vtable key may be published by several modules, because non-defining
/// partitions emit an external declaration of the same table. Those repeats
/// share one reservation. Two *distinct* keys resolving to one symbol is an
/// aliasing bug: LLVM would rename the second global and silently change the
/// linker identity that dispatch sites already reference.
fn record_vtable_symbols(
    index: &ProgramIndex,
    values: &mut HashMap<String, &'static str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut owners = HashMap::<String, nia_backend_ir::BackendTraitObjectVtableKey>::new();
    for module_id in index.module_ids() {
        let Some(module) = index.module(*module_id) else {
            continue;
        };
        for vtable in &module.trait_object_vtables {
            let Some(symbol) = expected_trait_object_vtable_symbol(index, &vtable.key) else {
                continue;
            };
            if let Some(previous) = owners.get(&symbol) {
                if *previous != vtable.key {
                    diagnostics.push(Diagnostic::internal_error_at(
                        nia_diagnostic::codes::INVALID_BACKEND_IR,
                        vtable.span,
                        format!(
                            "backend IR generated symbol collision: trait-object vtable reuses `{symbol}` already used by a different vtable identity"
                        ),
                    ));
                }
                continue;
            }
            if let Some(previous_kind) = values.get(&symbol) {
                diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    vtable.span,
                    format!(
                        "backend IR generated symbol collision: trait-object vtable reuses `{symbol}` already used by {previous_kind}"
                    ),
                ));
                continue;
            }
            owners.insert(symbol.clone(), vtable.key.clone());
            values.insert(symbol, "trait-object vtable");
        }
    }
}

fn record_generated_symbol(
    symbols: &mut HashMap<String, &'static str>,
    diagnostics: &mut Vec<Diagnostic>,
    symbol: String,
    kind: &'static str,
    span: nia_span::Span,
) {
    if let Some(previous_kind) = symbols.insert(symbol.clone(), kind) {
        diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!(
                "backend IR generated symbol collision: {kind} reuses `{symbol}` already used by {previous_kind}"
            ),
        ));
    }
}

pub(super) fn validate_backend_partition_definitions(
    partition: &CodegenPartition,
    index: &ProgramIndex,
) -> Vec<Diagnostic> {
    let module = index.module_for_partition(partition);
    let mut validator = BackendValidator::new(index, module.layouts.target);
    validator.validate_layout_owners(module);
    validator.validate_type_layout_products(module);
    validator.validate_aggregate_layout_products(module);
    validator.validate_enum_layout_products(module);
    for &position in partition.function_definitions() {
        let function = &module.functions[position];
        validator.validate_definition_owner(module.id, function.def_id, function.span, "function");
        validator.validate_function(&module.name, function, true);
    }
    for &position in partition.function_instance_definitions() {
        validator.validate_function_instance(
            &module.name,
            &module.function_instances[position],
            true,
        );
    }
    for &position in partition.closure_entry_definitions() {
        validator.validate_closure_entry(
            module.id,
            &module.name,
            &module.closure_entries[position],
            true,
        );
    }
    for &position in partition.global_definitions() {
        let global = &module.globals[position];
        validator.validate_definition_owner(module.id, global.def_id, global.span, "global");
        validator.validate_global(global, true);
    }
    for &position in partition.global_instance_definitions() {
        validator.validate_global_instance(&module.global_instances[position], true);
    }
    for &position in partition.vtable_definitions() {
        validator.validate_vtable(&module.trait_object_vtables[position], true);
    }
    validator.diagnostics
}

pub(super) fn validate_backend_partition_declarations(
    declarations: &CodegenDeclarationMembership,
    index: &ProgramIndex,
    target: TargetDataLayout,
) -> Vec<Diagnostic> {
    let mut validator = BackendValidator::new(index, target);
    for &def_id in &declarations.functions {
        let Some(item) = index.function(def_id) else {
            validator
                .diagnostics
                .push(missing_declaration_diagnostic("function", def_id));
            continue;
        };
        let Some(owner_id) = index.function_owner(def_id) else {
            validator
                .diagnostics
                .push(missing_declaration_diagnostic("function owner", def_id));
            continue;
        };
        let Some(owner) = index.module(owner_id) else {
            validator.diagnostics.push(missing_declaration_diagnostic(
                "function owner module",
                def_id,
            ));
            continue;
        };
        validator.validate_definition_owner(owner.id, item.def_id, item.span, "function");
        validator.validate_function(&owner.name, item, false);
    }
    for key in &declarations.function_instances {
        let Some(item) = index.function_instance(
            key.def_id,
            key.arg_module_id,
            key.self_arg,
            &key.args,
            &key.const_args,
        ) else {
            validator.diagnostics.push(missing_membership_diagnostic(
                "function instance",
                format!("{key:?}"),
            ));
            continue;
        };
        let Some(owner) = index
            .function_instance_owner(
                key.def_id,
                key.arg_module_id,
                key.self_arg,
                &key.args,
                &key.const_args,
            )
            .and_then(|owner| index.module(owner))
        else {
            validator.diagnostics.push(missing_membership_diagnostic(
                "function instance owner",
                format!("{key:?}"),
            ));
            continue;
        };
        validator.validate_function_instance(&owner.name, item, false);
    }
    for &def_id in &declarations.globals {
        let Some(global) = index.global(def_id) else {
            validator
                .diagnostics
                .push(missing_declaration_diagnostic("global", def_id));
            continue;
        };
        let Some(owner_id) = index.global_owner(def_id) else {
            validator
                .diagnostics
                .push(missing_declaration_diagnostic("global owner", def_id));
            continue;
        };
        validator.validate_definition_owner(owner_id, global.def_id, global.span, "global");
        validator.validate_global(global, false);
    }
    for key in &declarations.global_instances {
        let Some(instance) =
            index.global_instance(key.def_id, key.arg_module_id, &key.args, &key.const_args)
        else {
            validator.diagnostics.push(missing_membership_diagnostic(
                "global instance",
                format!("{key:?}"),
            ));
            continue;
        };
        validator.validate_global_instance(instance, false);
    }
    for &def_id in &declarations.structs {
        let Some(item) = index.struct_item(def_id) else {
            validator
                .diagnostics
                .push(missing_declaration_diagnostic("struct", def_id));
            continue;
        };
        let Some(owner_id) = index.struct_owner(def_id) else {
            validator
                .diagnostics
                .push(missing_declaration_diagnostic("struct owner", def_id));
            continue;
        };
        validator.validate_definition_owner(owner_id, item.def_id, item.span, "struct");
        validator.validate_struct(item);
    }
    for key in &declarations.struct_instances {
        let Some(instance) = index.struct_instance(key.def_id, &key.args, &key.const_args) else {
            validator.diagnostics.push(missing_membership_diagnostic(
                "struct instance",
                format!("{key:?}"),
            ));
            continue;
        };
        validator.validate_struct_instance(instance);
    }
    for &def_id in &declarations.unions {
        let Some(item) = index.union_item(def_id) else {
            validator
                .diagnostics
                .push(missing_declaration_diagnostic("union", def_id));
            continue;
        };
        let Some(owner_id) = index.union_owner(def_id) else {
            validator
                .diagnostics
                .push(missing_declaration_diagnostic("union owner", def_id));
            continue;
        };
        validator.validate_definition_owner(owner_id, item.def_id, item.span, "union");
        validator.validate_union(item);
    }
    for key in &declarations.union_instances {
        let Some(instance) = index.union_instance(key.def_id, &key.args, &key.const_args) else {
            validator.diagnostics.push(missing_membership_diagnostic(
                "union instance",
                format!("{key:?}"),
            ));
            continue;
        };
        validator.validate_union_instance(instance);
    }
    for key in &declarations.vtables {
        let Some(vtable) = index.trait_object_vtable(key) else {
            validator.diagnostics.push(missing_membership_diagnostic(
                "trait-object vtable",
                format!("{key:?}"),
            ));
            continue;
        };
        validator.validate_vtable(vtable, false);
    }
    validator.diagnostics
}

fn missing_declaration_diagnostic(kind: &str, def_id: GlobalDefId) -> Diagnostic {
    missing_membership_diagnostic(
        kind,
        format!("{def_id:?} without a matching published owner"),
    )
}

fn missing_membership_diagnostic(kind: &str, detail: String) -> Diagnostic {
    Diagnostic::internal_error_at(
        nia_diagnostic::codes::INVALID_BACKEND_IR,
        nia_span::Span::default(),
        format!("backend declaration membership references {kind} {detail}"),
    )
}

pub(super) fn validate_backend_declaration_module(
    module: &BackendModule,
    index: &ProgramIndex,
) -> Vec<Diagnostic> {
    let mut validator = BackendValidator::new(index, module.layouts.target);
    validator.validate_layout_owners(module);
    validator.validate_type_layout_products(module);
    validator.validate_aggregate_layout_products(module);
    validator.validate_enum_layout_products(module);
    for function in &module.functions {
        validator.validate_definition_owner(module.id, function.def_id, function.span, "function");
        validator.validate_function(&module.name, function, false);
    }
    for function in &module.function_instances {
        validator.validate_function_instance(&module.name, function, false);
    }
    for entry in &module.closure_entries {
        validator.validate_closure_entry(module.id, &module.name, entry, false);
    }
    for global in &module.globals {
        validator.validate_definition_owner(module.id, global.def_id, global.span, "global");
        validator.validate_global(global, false);
    }
    for global in &module.global_instances {
        validator.validate_global_instance(global, false);
    }
    for item in &module.structs {
        validator.validate_definition_owner(module.id, item.def_id, item.span, "struct");
        validator.validate_struct(item);
    }
    for item in &module.struct_instances {
        validator.validate_struct_instance(item);
    }
    for item in &module.unions {
        validator.validate_definition_owner(module.id, item.def_id, item.span, "union");
        validator.validate_union(item);
    }
    for item in &module.union_instances {
        validator.validate_union_instance(item);
    }
    for item in &module.enums {
        validator.validate_definition_owner(module.id, item.def_id, item.span, "enum");
        validator.current_item = Some(format!("enum {}", backend_symbol_debug_name(item.name)));
        validator.validate_runtime_type(item.backing_type, item.span);
        for variant in &item.variants {
            if !member_owner_matches(item.def_id.module_id, variant.def_id) {
                validator.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    variant.span,
                    format!(
                        "backend IR enum variant {:?} does not belong to its enum module",
                        variant.def_id
                    ),
                ));
            }
            match &variant.payload {
                nia_backend_ir::BackendEnumVariantPayload::Unit => {}
                nia_backend_ir::BackendEnumVariantPayload::Tuple(fields) => {
                    for field in fields {
                        validator.validate_runtime_type(*field, variant.span);
                    }
                }
                nia_backend_ir::BackendEnumVariantPayload::Named(fields) => {
                    validator.validate_field_owners(item.def_id.module_id, fields);
                    for field in fields {
                        validator.validate_runtime_type(field.ty, field.span);
                    }
                }
            }
        }
        validator.current_item = None;
    }
    for vtable in &module.trait_object_vtables {
        validator.validate_vtable(vtable, false);
    }
    validator.diagnostics
}

fn member_owner_matches(aggregate_module: ModuleId, member: GlobalDefId) -> bool {
    member.module_id == aggregate_module
}

fn instance_fields_match_template(
    instance: &[nia_backend_ir::BackendField],
    template: &[nia_backend_ir::BackendField],
) -> bool {
    instance.len() == template.len()
        && instance.iter().zip(template).all(|(instance, template)| {
            instance.def_id == template.def_id && instance.name == template.name
        })
}

fn backend_definition_name(index: &ProgramIndex, def_id: GlobalDefId) -> Option<SymbolId> {
    index
        .function(def_id)
        .map(|item| item.name)
        .or_else(|| index.global(def_id).map(|item| item.name))
        .or_else(|| index.struct_item(def_id).map(|item| item.name))
        .or_else(|| index.union_item(def_id).map(|item| item.name))
        .or_else(|| index.enum_item(def_id).map(|item| item.name))
}

impl<'a> BackendValidator<'a> {
    fn new(index: &'a ProgramIndex, target: TargetDataLayout) -> Self {
        let mut validator = Self {
            index,
            target,
            diagnostics: Vec::new(),
            seen_types: HashSet::new(),
            seen_closure_entries: HashSet::new(),
            layout_cache: RefCell::new(HashMap::new()),
            same_type_cache: RefCell::new(HashMap::new()),
            function_instance_ref_cache: RefCell::new(HashMap::new()),
            struct_fields_lookup_cache: RefCell::new(HashMap::new()),
            union_fields_lookup_cache: RefCell::new(HashMap::new()),
            local_tys: Vec::new(),
            local_kinds: Vec::new(),
            body_tys: Vec::new(),
            current_closure_owner: None,
            current_item: None,
            current_subject: None,
        };
        validator.validate_target_layout();
        validator
    }
}

pub(super) struct BackendValidator<'a> {
    index: &'a ProgramIndex,
    target: TargetDataLayout,
    diagnostics: Vec<Diagnostic>,
    seen_types: HashSet<InternedTyId>,
    seen_closure_entries: HashSet<BackendClosureEntryKey>,
    layout_cache: RefCell<HashMap<InternedTyId, Option<TypeLayout>>>,
    same_type_cache: RefCell<HashMap<(InternedTyId, InternedTyId), bool>>,
    function_instance_ref_cache: RefCell<HashMap<FunctionInstanceKey, bool>>,
    struct_fields_lookup_cache: AggregateFieldsLookup,
    union_fields_lookup_cache: AggregateFieldsLookup,
    local_tys: Vec<HashMap<LocalId, InternedTyId>>,
    local_kinds: Vec<HashMap<LocalId, FunctionLocalKind>>,
    body_tys: Vec<InternedTyId>,
    current_closure_owner: Option<BackendClosureEntryOwner>,
    current_item: Option<String>,
    current_subject: Option<&'static str>,
}

impl BackendValidator<'_> {
    fn validate_function(&mut self, module_name: &str, function: &BackendFunction, body: bool) {
        self.validate_link_name(
            "function",
            function.is_extern,
            function.link_name.as_deref(),
            function.span,
        );
        if function.is_extern && !function.generics.is_empty() {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                function.span,
                "backend IR extern function cannot have generic parameters",
            ));
        }
        if !function.generics.is_empty() {
            return;
        }
        self.current_item = Some(format!(
            "function {} in {}::{:?}",
            backend_symbol_debug_name(function.name),
            module_name,
            function.def_id
        ));
        self.current_closure_owner = Some(BackendClosureEntryOwner::Source(function.def_id));
        self.validate_function_attributes(
            &function.attributes,
            function.is_extern,
            function.function_body.is_some(),
            function.span,
        );
        if function.is_extern {
            self.validate_extern_function_abi(
                &function.params,
                function.return_type,
                function.span,
            );
        }
        self.validate_function_declaration_contract(
            &function.params,
            function.is_extern,
            function.is_variadic,
            function.function_body.is_some(),
            function.span,
        );
        self.validate_function_signature(&function.params, function.return_type, function.span);
        if body && let Some(body) = &function.function_body {
            self.validate_function_param_locals(&function.params, body);
            self.validate_function_body(body, function.return_type);
        }
        self.current_closure_owner = None;
        self.current_item = None;
    }

    fn validate_function_instance(
        &mut self,
        module_name: &str,
        function: &BackendFunctionInstance,
        body: bool,
    ) {
        self.validate_generated_symbol("function instance", &function.symbol, function.span);
        self.validate_instance_symbol(
            "function instance",
            &function.symbol,
            function.def_id,
            Some(function.arg_module_id),
            function.self_arg,
            &function.args,
            &function.const_args,
            function.span,
        );
        self.current_item = Some(format!(
            "function instance {} in {}::{:?}::{:?}",
            backend_symbol_debug_name(function.name),
            module_name,
            function.def_id,
            function.args
        ));
        self.current_closure_owner = Some(BackendClosureEntryOwner::FunctionInstance(
            FunctionInstanceKey {
                def_id: function.def_id,
                arg_module_id: function.arg_module_id,
                self_arg: function.self_arg,
                args: function.args.clone(),
                const_args: function.const_args.clone(),
            },
        ));
        self.validate_instance_arguments(
            function.self_arg,
            &function.args,
            &function.const_args,
            function.span,
        );
        self.validate_function_attributes(
            &function.attributes,
            function.is_extern,
            function.function_body.is_some(),
            function.span,
        );
        self.validate_function_instance_metadata(function);
        if function.is_extern {
            self.validate_extern_function_abi(
                &function.params,
                function.return_type,
                function.span,
            );
        }
        self.validate_function_declaration_contract(
            &function.params,
            function.is_extern,
            function.is_variadic,
            function.function_body.is_some(),
            function.span,
        );
        self.validate_function_signature(&function.params, function.return_type, function.span);
        if body && let Some(body) = &function.function_body {
            self.validate_function_param_locals(&function.params, body);
            self.validate_function_body(body, function.return_type);
        }
        self.current_closure_owner = None;
        self.current_item = None;
    }

    fn validate_closure_entry(
        &mut self,
        module_id: ModuleId,
        module_name: &str,
        entry: &BackendClosureEntry,
        body: bool,
    ) {
        self.validate_generated_symbol("closure entry", &entry.symbol, entry.span);
        self.current_item = Some(format!(
            "closure entry {} in {}::{:?}#{}",
            entry.symbol, module_name, entry.key.closure_id.owner, entry.key.closure_id.ordinal
        ));
        if !self.seen_closure_entries.insert(entry.key.clone()) {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                entry.span,
                "backend module contains a duplicate closure entry identity",
            ));
        }
        self.current_closure_owner = Some(entry.key.owner.clone());
        let owner_identity_matches = match &entry.key.owner {
            BackendClosureEntryOwner::Source(owner) => *owner == entry.key.closure_id.owner,
            BackendClosureEntryOwner::FunctionInstance(owner) => {
                owner.def_id == entry.key.closure_id.owner
            }
        };
        if !owner_identity_matches {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                entry.span,
                "closure entry owner does not match its source closure identity",
            ));
        }
        let owner_exists = match &entry.key.owner {
            BackendClosureEntryOwner::Source(owner) => self.index.function(*owner).is_some(),
            BackendClosureEntryOwner::FunctionInstance(owner) => self
                .index
                .function_instance(
                    owner.def_id,
                    owner.arg_module_id,
                    owner.self_arg,
                    &owner.args,
                    &owner.const_args,
                )
                .is_some(),
        };
        if !owner_exists {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                entry.span,
                "closure entry owner does not resolve to a backend function",
            ));
        }
        let owner_module = match &entry.key.owner {
            BackendClosureEntryOwner::Source(owner) => Some(owner.module_id),
            BackendClosureEntryOwner::FunctionInstance(owner) => {
                self.index.function_instance_owner(
                    owner.def_id,
                    owner.arg_module_id,
                    owner.self_arg,
                    &owner.args,
                    &owner.const_args,
                )
            }
        };
        if owner_module != Some(module_id) {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                entry.span,
                "closure entry is not published with its owning backend function",
            ));
        }
        let owner_symbol = match &entry.key.owner {
            BackendClosureEntryOwner::Source(owner) => self
                .index
                .function(*owner)
                .zip(self.index.module(owner.module_id))
                .map(|(function, module)| {
                    mangle_base_symbol_id(
                        *owner,
                        MangleModuleId::from_normalized_source_path(
                            module.source_identity.normalized_path(),
                        ),
                        function.name,
                    )
                }),
            BackendClosureEntryOwner::FunctionInstance(owner) => self
                .index
                .function_instance(
                    owner.def_id,
                    owner.arg_module_id,
                    owner.self_arg,
                    &owner.args,
                    &owner.const_args,
                )
                .map(|instance| instance.symbol.clone()),
        };
        if owner_symbol
            .as_deref()
            .map(|symbol| mangle_closure_entry_symbol(symbol, entry.key.closure_id) != entry.symbol)
            == Some(true)
        {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                entry.span,
                "closure entry symbol does not match its owner-derived identity",
            ));
        }
        self.validate_runtime_type(entry.abi.state_type, entry.span);
        self.validate_runtime_type(entry.abi.state_pointer_type, entry.span);
        for param in &entry.abi.params {
            self.validate_runtime_type(*param, entry.span);
        }
        self.current_subject = Some("closure return type");
        self.validate_runtime_type(entry.abi.return_type, entry.span);
        self.current_subject = None;

        let state_contract_matches =
            match self.index.type_store().get(entry.abi.state_type).cloned() {
                Some(TyKind::ClosureState {
                    closure_id,
                    params,
                    return_type,
                    ..
                }) => {
                    closure_id == entry.key.closure_id
                        && self.same_type_args(&params, &entry.abi.params)
                        && self.same_type(return_type, entry.abi.return_type)
                }
                _ => false,
            };
        if !state_contract_matches {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                entry.span,
                "closure entry state type does not match its identity and ABI signature",
            ));
        }
        let state_pointer_matches = matches!(
            self.index.type_store().get(entry.abi.state_pointer_type),
            Some(TyKind::Pointer {
                is_readonly: true,
                elem,
            }) if *elem == entry.abi.state_type
        );
        if !state_pointer_matches {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                entry.span,
                "closure entry ABI state pointer must be a readonly pointer to its state type",
            ));
        }
        if entry.abi.params.len() != entry.params.len() {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                entry.span,
                "closure entry ABI parameter list does not match its body parameters",
            ));
        }
        if body {
            self.validate_closure_entry_param_locals(entry);
            self.validate_function_body(&entry.function_body, entry.abi.return_type);
            if !self.same_type(entry.function_body.ty, entry.abi.return_type) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    entry.function_body.span,
                    "closure entry body type does not match its ABI return type",
                ));
            }
        }
        self.current_closure_owner = None;
        self.current_item = None;
    }

    fn validate_function_signature(
        &mut self,
        params: &[BackendParam],
        return_type: InternedTyId,
        span: nia_span::Span,
    ) {
        // Returns cross the same LLVM ABI boundary as parameters. Keep the
        // runtime-layout check symmetric so opaque/compile-time-only return
        // types are rejected before LLVM type classification.
        self.current_subject = Some("return type");
        self.validate_runtime_type(return_type, span);
        self.current_subject = None;
        for param in params {
            self.current_subject = Some("param passing_ty");
            self.validate_runtime_type(param.passing_ty, param.span);
            self.current_subject = Some("param local_ty");
            self.validate_runtime_type(param.local_ty, param.span);
            self.current_subject = None;
        }
    }

    fn validate_function_declaration_contract(
        &mut self,
        params: &[BackendParam],
        is_extern: bool,
        is_variadic: bool,
        has_body: bool,
        span: nia_span::Span,
    ) {
        // Recheck the source-level foreign-variadic declaration invariants so
        // malformed Backend IR cannot reach LLVM's variadic type construction.
        if !is_extern || !is_variadic {
            return;
        }
        if params.is_empty() {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                "extern variadic function requires at least one fixed parameter",
            ));
        }
        if has_body {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                "extern variadic function definition is not supported",
            ));
        }
    }

    fn validate_function_attributes(
        &mut self,
        attributes: &[nia_backend_ir::BackendFunctionAttribute],
        is_extern: bool,
        has_body: bool,
        span: nia_span::Span,
    ) {
        for attribute in attributes {
            if matches!(attribute, nia_backend_ir::BackendFunctionAttribute::Naked)
                && (!is_extern || !has_body)
            {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    "backend IR `naked` attribute is only valid on extern function definitions",
                ));
            }
            if matches!(
                attribute,
                nia_backend_ir::BackendFunctionAttribute::TrackCaller
            ) && is_extern
            {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    span,
                    "backend IR `trackCaller` attribute is not valid on extern functions",
                ));
            }
        }
    }

    fn validate_function_instance_metadata(&mut self, function: &BackendFunctionInstance) {
        let Some(template) = self.index.function(function.def_id) else {
            return;
        };
        if function.args.len() + function.const_args.len() != template.generics.len() {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                function.span,
                "backend IR function instance generic argument arity does not match its source template",
            ));
        }
        if function.name != template.name {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                function.span,
                "backend IR function instance name does not match its source template",
            ));
        }
        if function.params.len() != template.params.len()
            || function
                .params
                .iter()
                .zip(&template.params)
                .any(|(instance, template)| {
                    instance.local_id != template.local_id
                        || instance.name != template.name
                        || instance.receiver != template.receiver
                })
        {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                function.span,
                "backend IR function instance parameter metadata does not match its source template",
            ));
        }
        if function.is_extern != template.is_extern {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                function.span,
                "backend IR function instance extern flag does not match its source template",
            ));
        }
        if function.is_variadic != template.is_variadic {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                function.span,
                "backend IR function instance variadic flag does not match its source template",
            ));
        }
        if function.attributes != template.attributes {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                function.span,
                "backend IR function instance attributes do not match its source template",
            ));
        }
    }

    fn validate_extern_function_abi(
        &mut self,
        params: &[BackendParam],
        return_type: InternedTyId,
        span: nia_span::Span,
    ) {
        for param in params {
            self.validate_extern_abi_type(
                param.passing_ty,
                ExternAbiTypeContext::FunctionParameter,
                param.span,
                &mut Vec::new(),
            );
        }
        self.validate_extern_abi_type(
            return_type,
            ExternAbiTypeContext::FunctionReturn,
            span,
            &mut Vec::new(),
        );
    }

    fn validate_extern_abi_type(
        &mut self,
        ty: InternedTyId,
        context: ExternAbiTypeContext,
        span: nia_span::Span,
        nominal_stack: &mut Vec<GlobalDefId>,
    ) {
        let description = context.description();
        let Some(kind) = self.ty_kind(ty).cloned() else {
            self.extern_abi_error(
                span,
                format!("{description} type is missing from the type store"),
            );
            return;
        };
        if context.permits_unit() && kind.is_unit() {
            return;
        }
        match kind {
            TyKind::Primitive(PrimitiveTy::Bool) => {
                self.extern_abi_error(span, format!("{description} cannot use `bool` directly"));
            }
            TyKind::Primitive(PrimitiveTy::Char) => {
                self.extern_abi_error(span, format!("{description} cannot use `char` directly"));
            }
            TyKind::Primitive(PrimitiveTy::Never) => {
                self.extern_abi_error(span, format!("{description} cannot use `never` directly"));
            }
            TyKind::Primitive(_) | TyKind::Pointer { .. } | TyKind::VolatilePointer { .. } => {}
            TyKind::Opaque => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use incomplete `opaque` directly"),
                );
            }
            TyKind::Tuple(_) => {
                self.extern_abi_error(span, format!("{description} cannot use tuple by value"));
            }
            TyKind::Vector { .. } => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use SIMD vector by value"),
                );
            }
            TyKind::Slice { .. } => {
                self.extern_abi_error(span, format!("{description} cannot use nia slice directly"));
            }
            TyKind::SlicePointee { .. } => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use unsized slice pointee directly"),
                );
            }
            TyKind::TraitObject { .. } => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use nia trait object directly"),
                );
            }
            TyKind::TraitObjectPointee { .. } => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use unsized trait object pointee directly"),
                );
            }
            TyKind::Callable { .. } => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use nia callable view directly"),
                );
            }
            TyKind::CallablePointee { .. } => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use unsized callable interface directly"),
                );
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                if is_variadic {
                    self.extern_abi_error(
                        span,
                        format!("{description} cannot use variadic function pointer"),
                    );
                }
                for param in params {
                    self.validate_extern_abi_type(
                        param,
                        ExternAbiTypeContext::FunctionPointerParameter,
                        span,
                        nominal_stack,
                    );
                }
                self.validate_extern_abi_type(
                    return_type,
                    ExternAbiTypeContext::FunctionPointerReturn,
                    span,
                    nominal_stack,
                );
            }
            TyKind::Array { len, elem } => {
                if context != ExternAbiTypeContext::StructField {
                    self.extern_abi_error(span, format!("{description} cannot use array by value"));
                } else {
                    if matches!(len, ArrayLenTy::Infer | ArrayLenTy::GenericParam(_)) {
                        self.extern_abi_error(
                            span,
                            "extern struct field cannot use an unresolved array length",
                        );
                    }
                    self.validate_extern_abi_type(
                        elem,
                        ExternAbiTypeContext::StructField,
                        span,
                        nominal_stack,
                    );
                }
            }
            TyKind::Range { .. } => {
                self.extern_abi_error(span, format!("{description} cannot use range by value"));
            }
            TyKind::Optional { .. } => {
                self.extern_abi_error(span, format!("{description} cannot use optional by value"));
            }
            TyKind::ErrorUnion { .. } => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use error union by value"),
                );
            }
            TyKind::ClosureState { .. } => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use closure state directly"),
                );
            }
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => self.validate_extern_nominal_abi(
                def_id,
                &args,
                &const_args,
                context,
                span,
                nominal_stack,
            ),
            TyKind::BuiltinTrait { .. } => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use trait type directly"),
                );
            }
            TyKind::BuiltinType(builtin) => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use builtin type `{builtin:?}` directly"),
                );
            }
            TyKind::GenericParam(_) | TyKind::SelfParam => {
                self.extern_abi_error(span, format!("{description} cannot use generic parameter"));
            }
            TyKind::Projection { .. } => {
                self.extern_abi_error(
                    span,
                    format!("{description} cannot use unresolved associated type projection"),
                );
            }
            TyKind::ConstOnly => {
                self.extern_abi_error(span, format!("{description} cannot use const-only value"));
            }
            TyKind::Error => {}
        }
    }

    fn validate_extern_nominal_abi(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
        context: ExternAbiTypeContext,
        span: nia_span::Span,
        nominal_stack: &mut Vec<GlobalDefId>,
    ) {
        let description = context.description();
        if nominal_stack.contains(&def_id) {
            self.extern_abi_error(
                span,
                format!("recursive nominal type cannot be used as {description}"),
            );
            return;
        }
        if self.index.enum_item(def_id).is_some() {
            self.extern_abi_error(
                span,
                format!("{description} cannot use enum directly; use its backing integer type"),
            );
            return;
        }
        if self.index.union_item(def_id).is_some() {
            self.extern_abi_error(span, format!("{description} cannot use union by value"));
            return;
        }
        let instance = self
            .index
            .struct_instance(def_id, args, const_args)
            .or_else(|| {
                self.index.struct_instances_for(def_id).find(|instance| {
                    self.same_type_args(&instance.args, args)
                        && self.same_const_args(&instance.const_args, const_args)
                })
            });
        let (is_extern, fields) = if let Some(instance) = instance {
            (instance.is_extern, Some(instance.fields.clone()))
        } else if args.is_empty() && const_args.is_empty() {
            let Some(item) = self.index.struct_item(def_id) else {
                self.extern_abi_error(
                    span,
                    format!("{description} nominal type has no ABI classification"),
                );
                return;
            };
            (item.is_extern, Some(item.fields.clone()))
        } else {
            let Some(item) = self.index.struct_item(def_id) else {
                self.extern_abi_error(
                    span,
                    format!("{description} nominal type has no ABI classification"),
                );
                return;
            };
            (item.is_extern, None)
        };
        if !is_extern {
            self.extern_abi_error(
                span,
                format!("{description} cannot use normal Nia struct by value"),
            );
            return;
        }
        let Some(fields) = fields else {
            self.extern_abi_error(
                span,
                format!("{description} nominal type has no materialized ABI fields"),
            );
            return;
        };
        if fields.is_empty() {
            self.extern_abi_error(
                span,
                format!("{description} cannot use empty struct by value"),
            );
            return;
        }
        nominal_stack.push(def_id);
        for field in &fields {
            self.validate_extern_abi_type(
                field.ty,
                ExternAbiTypeContext::StructField,
                field.span,
                nominal_stack,
            );
        }
        nominal_stack.pop();
    }

    fn extern_abi_error(&mut self, span: nia_span::Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            message,
        ));
    }

    fn validate_global(&mut self, global: &BackendGlobal, init: bool) {
        self.current_item = Some(format!("global {}", backend_symbol_debug_name(global.name)));
        self.validate_link_name(
            "global",
            global.is_extern,
            global.link_name.as_deref(),
            global.span,
        );
        self.validate_runtime_type(global.ty, global.span);
        if global.is_extern {
            self.validate_extern_abi_type(
                global.ty,
                ExternAbiTypeContext::Global,
                global.span,
                &mut Vec::new(),
            );
        }
        if init && let Some(value) = &global.init {
            self.validate_static_init(global.ty, value, global.span);
        }
        self.current_item = None;
    }

    fn validate_link_name(
        &mut self,
        kind: &'static str,
        is_extern: bool,
        link_name: Option<&str>,
        span: nia_span::Span,
    ) {
        match (is_extern, link_name) {
            (true, None) => self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR extern {kind} requires an external link name"),
            )),
            (false, Some(_)) => self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR non-extern {kind} cannot publish an external link name"),
            )),
            _ => {}
        }
        if link_name.is_some_and(|name| name.is_empty() || name.contains('\0')) {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR {kind} link name must not be empty or contain NUL"),
            ));
        }
    }

    fn validate_generated_symbol(
        &mut self,
        kind: &'static str,
        symbol: &str,
        span: nia_span::Span,
    ) {
        if symbol.is_empty() || symbol.contains('\0') {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR {kind} symbol must not be empty or contain NUL"),
            ));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_instance_symbol(
        &mut self,
        kind: &'static str,
        symbol: &str,
        def_id: GlobalDefId,
        arg_module_id: Option<ModuleId>,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
        span: nia_span::Span,
    ) {
        let Some(expected) =
            self.expected_instance_symbol(def_id, arg_module_id, self_arg, args, const_args)
        else {
            return;
        };
        if symbol != expected {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR {kind} symbol does not match its instance identity"),
            ));
        }
    }

    fn expected_instance_symbol(
        &self,
        def_id: GlobalDefId,
        arg_module_id: Option<ModuleId>,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<String> {
        if self_arg
            .into_iter()
            .chain(args.iter().copied())
            .chain(const_args.iter().map(|arg| arg.ty))
            .any(|ty| self.index.type_store().get(ty).is_none())
        {
            return None;
        }
        let name = backend_definition_name(self.index, def_id)?;
        let missing_module = Cell::new(false);
        let mut mangled_args = args.to_vec();
        if let Some(self_arg) = self_arg {
            mangled_args.insert(0, self_arg);
        }
        let mut symbol = mangle_instance_symbol_id(
            def_id,
            name,
            &mangled_args,
            const_args,
            self.index.type_store(),
            MangleResolvers::new(
                |module_id| {
                    self.index
                        .module(module_id)
                        .map(|module| {
                            MangleModuleId::from_normalized_source_path(
                                module.source_identity.normalized_path(),
                            )
                        })
                        .unwrap_or_else(|| {
                            missing_module.set(true);
                            MangleModuleId::from_normalized_source_path("")
                        })
                },
                |def_id| {
                    backend_definition_name(self.index, def_id)
                        .map(backend_symbol_debug_name)
                        .unwrap_or_else(|| {
                            missing_module.set(true);
                            format!("def{}", def_id.def_id.0)
                        })
                },
                |const_expr: nia_ids::GlobalConstExprId| {
                    self.index.module(const_expr.module_id).and_then(|module| {
                        module.const_eval.array_lengths.get(&const_expr).copied()
                    })
                },
            ),
        );
        if self_arg.is_some() {
            symbol = symbol.replacen("__inst__t_", "__inst__t_self_", 1);
        }
        let Some(arg_module_id) = arg_module_id else {
            return (!missing_module.get()).then_some(symbol);
        };
        let context = self
            .index
            .module(arg_module_id)
            .map(|module| nia_symbol::stable_hash(module.source_identity.normalized_path()));
        context
            .filter(|_| !missing_module.get())
            .map(|context| format!("{symbol}__ctx_s{context:016x}"))
    }

    fn validate_global_instance(&mut self, global: &BackendGlobalInstance, init: bool) {
        self.validate_generated_symbol("global instance", &global.symbol, global.span);
        self.validate_instance_symbol(
            "global instance",
            &global.symbol,
            global.def_id,
            Some(global.arg_module_id),
            None,
            &global.args,
            &global.const_args,
            global.span,
        );
        self.current_item = Some(format!(
            "global instance {}::{:?}",
            backend_symbol_debug_name(global.name),
            global.args
        ));
        self.validate_instance_arguments(None, &global.args, &global.const_args, global.span);
        if let Some(template) = self.index.global(global.def_id) {
            if template.is_extern {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    global.span,
                    "backend IR global instance cannot materialize an extern source global",
                ));
            }
            if global.name != template.name {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    global.span,
                    "backend IR global instance name does not match its source template",
                ));
            }
            if global.is_let != template.is_let {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    global.span,
                    "backend IR global instance mutability does not match its source template",
                ));
            }
            if global.init.is_some() != template.init.is_some() {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    global.span,
                    "backend IR global instance initializer presence does not match its source template",
                ));
            }
        }
        self.validate_runtime_type(global.ty, global.span);
        if init && let Some(value) = &global.init {
            self.validate_static_init(global.ty, value, global.span);
        }
        self.current_item = None;
    }

    fn validate_struct(&mut self, item: &BackendStruct) {
        self.validate_field_owners(item.def_id.module_id, &item.fields);
        if item.is_extern {
            self.validate_extern_struct_fields(&item.fields);
        }
        if item.generics.is_empty() {
            self.current_item = Some(format!("struct {}", backend_symbol_debug_name(item.name)));
            self.validate_fields(&item.fields);
            self.current_item = None;
        }
    }

    fn validate_definition_owner(
        &mut self,
        module_id: ModuleId,
        def_id: GlobalDefId,
        span: nia_span::Span,
        kind: &str,
    ) {
        if !definition_owner_matches(module_id, def_id) {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!(
                    "backend IR {kind} definition {def_id:?} does not belong to its backend module"
                ),
            ));
        }
    }

    fn validate_layout_owners(&mut self, module: &BackendModule) {
        for (def_id, _) in &module.layouts.structs {
            self.validate_definition_owner(
                module.id,
                *def_id,
                nia_span::Span::default(),
                "struct layout",
            );
        }
        for (def_id, _) in &module.layouts.unions {
            self.validate_definition_owner(
                module.id,
                *def_id,
                nia_span::Span::default(),
                "union layout",
            );
        }
        for (def_id, _) in &module.layouts.enums {
            self.validate_definition_owner(
                module.id,
                *def_id,
                nia_span::Span::default(),
                "enum layout",
            );
        }
    }

    fn validate_struct_instance(&mut self, item: &BackendStructInstance) {
        self.validate_generated_symbol("struct instance", &item.symbol, item.span);
        self.validate_instance_symbol(
            "struct instance",
            &item.symbol,
            item.def_id,
            None,
            None,
            &item.args,
            &item.const_args,
            item.span,
        );
        self.current_item = Some(format!(
            "struct instance {}::{:?}",
            backend_symbol_debug_name(item.name),
            item.args
        ));
        self.validate_instance_arguments(None, &item.args, &item.const_args, item.span);
        if let Some(template) = self.index.struct_item(item.def_id) {
            if item.args.len() + item.const_args.len() != template.generics.len() {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    item.span,
                    "backend IR struct instance generic argument arity does not match its source template",
                ));
            }
            if template.is_extern != item.is_extern {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    item.span,
                    "backend IR struct instance extern flag does not match its source template",
                ));
            }
            if template.name != item.name {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    item.span,
                    "backend IR struct instance name does not match its source template",
                ));
            }
            if !instance_fields_match_template(&item.fields, &template.fields) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    item.span,
                    "backend IR struct instance field metadata does not match its source template",
                ));
            }
        }
        self.validate_field_owners(item.def_id.module_id, &item.fields);
        self.validate_fields(&item.fields);
        if item.is_extern {
            self.validate_extern_struct_fields(&item.fields);
        }
        self.current_item = None;
    }

    fn validate_union(&mut self, item: &BackendUnion) {
        self.validate_field_owners(item.def_id.module_id, &item.fields);
        if item.generics.is_empty() {
            self.current_item = Some(format!("union {}", backend_symbol_debug_name(item.name)));
            self.validate_fields(&item.fields);
            self.current_item = None;
        }
    }

    fn validate_union_instance(&mut self, item: &BackendUnionInstance) {
        self.validate_generated_symbol("union instance", &item.symbol, item.span);
        self.validate_instance_symbol(
            "union instance",
            &item.symbol,
            item.def_id,
            None,
            None,
            &item.args,
            &item.const_args,
            item.span,
        );
        self.current_item = Some(format!(
            "union instance {}::{:?}",
            backend_symbol_debug_name(item.name),
            item.args
        ));
        self.validate_instance_arguments(None, &item.args, &item.const_args, item.span);
        if let Some(template) = self.index.union_item(item.def_id) {
            if item.args.len() + item.const_args.len() != template.generics.len() {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    item.span,
                    "backend IR union instance generic argument arity does not match its source template",
                ));
            }
            if template.is_extern != item.is_extern {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    item.span,
                    "backend IR union instance extern flag does not match its source template",
                ));
            }
            if template.name != item.name {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    item.span,
                    "backend IR union instance name does not match its source template",
                ));
            }
            if !instance_fields_match_template(&item.fields, &template.fields) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    item.span,
                    "backend IR union instance field metadata does not match its source template",
                ));
            }
        }
        self.validate_field_owners(item.def_id.module_id, &item.fields);
        self.validate_fields(&item.fields);
        self.current_item = None;
    }

    fn validate_fields(&mut self, fields: &[nia_backend_ir::BackendField]) {
        for field in fields {
            self.validate_runtime_type(field.ty, field.span);
        }
    }

    fn validate_instance_arguments(
        &mut self,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
        span: nia_span::Span,
    ) {
        if let Some(self_arg) = self_arg {
            self.validate_type(self_arg, span);
        }
        for arg in args {
            self.validate_type(*arg, span);
        }
        for arg in const_args {
            self.validate_const_arg(arg, span);
        }
    }

    fn validate_extern_struct_fields(&mut self, fields: &[nia_backend_ir::BackendField]) {
        for field in fields {
            self.validate_extern_abi_type(
                field.ty,
                ExternAbiTypeContext::StructField,
                field.span,
                &mut Vec::new(),
            );
        }
    }

    fn validate_field_owners(
        &mut self,
        aggregate_module: ModuleId,
        fields: &[nia_backend_ir::BackendField],
    ) {
        for field in fields {
            if !member_owner_matches(aggregate_module, field.def_id) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    field.span,
                    format!(
                        "backend IR aggregate field {:?} does not belong to its aggregate module",
                        field.def_id
                    ),
                ));
            }
        }
    }

    fn validate_enum_discriminants(&mut self, item: &nia_backend_ir::BackendEnum) {
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(item.backing_type).cloned() else {
            self.invalid_enum_declaration(item.span, "backing type is not primitive");
            return;
        };
        if !primitive.is_integer() {
            self.invalid_enum_declaration(item.span, "backing type is not an integer");
            return;
        }
        let Some(pointer_bits) = self
            .target
            .pointer_size
            .checked_mul(8)
            .and_then(|bits| u32::try_from(bits).ok())
        else {
            self.invalid_enum_declaration(item.span, "target pointer width is invalid");
            return;
        };
        for (index, variant) in item.variants.iter().enumerate() {
            let value = variant.value.unwrap_or(index as i128);
            if !nia_ty::IntConst::from_i128(value).fits_primitive_int(primitive, pointer_bits) {
                self.invalid_enum_declaration(
                    variant.span,
                    "variant discriminant is out of range for its backing type",
                );
            }
        }
    }

    fn invalid_enum_declaration(&mut self, span: nia_span::Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR enum declaration has an invalid contract: {message}"),
        ));
    }

    fn validate_vtable(&mut self, vtable: &BackendTraitObjectVtable, entries: bool) {
        self.current_item = Some(format!("trait object vtable {:?}", vtable.key));
        self.validate_trait_object_self_type(vtable.key.self_ty, vtable.span);
        self.validate_runtime_type(vtable.key.object_ty, vtable.span);
        let payload_matches_object = matches!(
            self.ty_kind(vtable.key.object_ty).cloned(),
            Some(TyKind::TraitObject {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            }) if trait_id == vtable.trait_id
                && self.same_type_args(&trait_args, &vtable.trait_args)
                && self.same_const_args(&trait_const_args, &vtable.trait_const_args)
        );
        if !payload_matches_object {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                vtable.span,
                "backend IR vtable trait arguments do not match its object type",
            ));
        }
        // LLVM emits entries in vector order, while dynamic calls address the
        // explicit slot field. Requiring the two coordinates to agree prevents
        // a well-typed but unreferenced malformed table from reaching codegen.
        for (expected_slot, entry) in vtable.entries.iter().enumerate() {
            if entry.slot != expected_slot {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    vtable.span,
                    "backend IR vtable entry slot does not match its table position",
                ));
            }
            for arg in &entry.trait_args {
                self.validate_type(*arg, vtable.span);
            }
            for arg in &entry.trait_const_args {
                self.validate_const_arg(arg, vtable.span);
            }
        }
        if entries {
            for entry in &vtable.entries {
                match &entry.function {
                    BackendTraitObjectVtableFunction::Function(def_id) => {
                        self.validate_function_ref(
                            *def_id,
                            vtable.span,
                            "backend IR vtable references missing function",
                        );
                    }
                    BackendTraitObjectVtableFunction::FunctionInstance {
                        def_id,
                        arg_module_id,
                        self_arg,
                        args,
                        const_args,
                    } => {
                        self.validate_function_instance_ref(
                            FunctionInstanceRef {
                                def_id: *def_id,
                                arg_module_id: *arg_module_id,
                                self_arg: *self_arg,
                                args,
                                const_args,
                            },
                            vtable.span,
                            "backend IR vtable references missing function instance",
                        );
                    }
                }
            }
        }
        self.current_item = None;
    }

    fn validate_function_param_locals(&mut self, params: &[BackendParam], body: &FunctionBody) {
        let param_locals = body
            .locals
            .iter()
            .filter(|local| local.kind == FunctionLocalKind::Param)
            .map(|local| (local.id, local.ty))
            .collect::<HashMap<_, _>>();
        let mut mapped_locals = HashSet::with_capacity(params.len());
        for param in params {
            let Some(local_id) = param.local_id else {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    param.span,
                    "backend IR function parameter with a body is missing its local binding",
                ));
                continue;
            };
            if !mapped_locals.insert(local_id) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    param.span,
                    format!(
                        "backend IR function parameters reference duplicate body local {local_id:?}"
                    ),
                ));
            }
            let Some(local_ty) = param_locals.get(&local_id).copied() else {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    param.span,
                    format!("backend IR function parameter references missing local {local_id:?}"),
                ));
                continue;
            };
            if !self.same_type(param.local_ty, local_ty) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    param.span,
                    "backend IR function parameter local type does not match its body local",
                ));
            }
        }
        // FunctionBody.locals is intentionally flat and includes locals from
        // nested closure bodies. We therefore cannot reject every unmapped
        // `Param` local here without mistaking a nested ABI parameter for an
        // outer function parameter; each nested closure is validated separately.
    }

    fn validate_closure_entry_param_locals(&mut self, entry: &BackendClosureEntry) {
        let param_locals = entry
            .function_body
            .locals
            .iter()
            .filter(|local| local.kind == FunctionLocalKind::Param)
            .map(|local| (local.id, local.ty))
            .collect::<HashMap<_, _>>();
        let mut mapped_locals = HashSet::with_capacity(entry.params.len() + 1);
        let expected = std::iter::once((entry.state_param, Some(entry.abi.state_pointer_type)))
            .chain(
                entry
                    .params
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(index, local_id)| (local_id, entry.abi.params.get(index).copied())),
            );
        for (local_id, expected_ty) in expected {
            if !mapped_locals.insert(local_id) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    entry.span,
                    format!(
                        "closure entry ABI parameters reference duplicate body local {local_id:?}"
                    ),
                ));
            }
            let Some(local_ty) = param_locals.get(&local_id).copied() else {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    entry.span,
                    format!(
                        "closure entry ABI parameter references missing body local {local_id:?}"
                    ),
                ));
                continue;
            };
            if expected_ty.is_some_and(|expected_ty| !self.same_type(expected_ty, local_ty)) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    entry.span,
                    format!("closure entry ABI parameter local {local_id:?} has a mismatched type"),
                ));
            }
        }
        for local_id in param_locals.keys() {
            if !mapped_locals.contains(local_id) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    entry.span,
                    format!("closure entry body contains unmapped parameter local {local_id:?}"),
                ));
            }
        }
    }
}

fn definition_owner_matches(module_id: ModuleId, def_id: GlobalDefId) -> bool {
    def_id.module_id == module_id
}

#[cfg(test)]
mod owner_tests {
    use std::sync::Arc;

    use super::{
        definition_owner_matches, expected_trait_object_vtable_symbol, member_owner_matches,
        missing_declaration_diagnostic, validate_backend_partition_declarations,
    };
    use crate::declaration_membership::CodegenDeclarationMembership;
    use crate::program_index::ProgramIndex;
    use nia_backend_ir::{
        BackendConstFacts, BackendLayouts, BackendModule, BackendModuleStore,
        CodegenUnitDependencies, CodegenUnitId,
    };
    use nia_function_ir::FunctionInstanceKey;
    use nia_ids::{ConstExprId, DefId, GlobalConstExprId, GlobalDefId, ModuleIdAllocator};
    use nia_layout::{TargetDataLayout, TypeLayout};
    use nia_source::SourceIdentity;
    use nia_ty::{ArrayLenTy, TyKind, TypeStore};

    #[test]
    fn aggregate_members_require_the_nominal_module_owner() {
        let mut modules = ModuleIdAllocator::new();
        let owner = modules.allocate();
        let foreign = modules.allocate();
        let field = DefId(4);

        assert!(member_owner_matches(
            owner,
            GlobalDefId {
                module_id: owner,
                def_id: field,
            }
        ));
        assert!(!member_owner_matches(
            owner,
            GlobalDefId {
                module_id: foreign,
                def_id: field,
            }
        ));
    }

    #[test]
    fn ordinary_definitions_require_the_backend_module_owner() {
        let mut modules = ModuleIdAllocator::new();
        let owner = modules.allocate();
        let foreign = modules.allocate();
        assert!(definition_owner_matches(
            owner,
            GlobalDefId {
                module_id: owner,
                def_id: DefId(1),
            }
        ));
        assert!(!definition_owner_matches(
            owner,
            GlobalDefId {
                module_id: foreign,
                def_id: DefId(1),
            }
        ));
    }

    #[test]
    fn stale_declaration_membership_is_an_invalid_backend_ir_diagnostic() {
        let module_id = ModuleIdAllocator::new().allocate();
        let def_id = GlobalDefId {
            module_id,
            def_id: DefId(7),
        };

        let diagnostic = missing_declaration_diagnostic("function", def_id);

        assert!(
            diagnostic
                .summary
                .contains("backend declaration membership")
        );
        assert!(
            diagnostic
                .summary
                .contains("without a matching published owner")
        );
    }

    #[test]
    fn missing_instance_in_declaration_membership_is_a_diagnostic() {
        let module_id = ModuleIdAllocator::new().allocate();
        let module = BackendModule {
            id: module_id,
            source_identity: SourceIdentity::new("stale-membership.nia"),
            name: "stale-membership".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: TargetDataLayout::LP64,
                types: Vec::new(),
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
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
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        };
        let store = Arc::new(BackendModuleStore::new([module_id]));
        store.publish(module);
        let (index, mut publisher) = ProgramIndex::new(store, Arc::new(TypeStore::new()));
        publisher.publish(module_id);
        let unit = CodegenUnitId::SourceModule {
            module_id,
            ordinal: 0,
        };
        let missing = FunctionInstanceKey {
            def_id: GlobalDefId {
                module_id,
                def_id: DefId(9),
            },
            arg_module_id: module_id,
            self_arg: None,
            args: Vec::new(),
            const_args: Vec::new(),
        };
        let declarations = CodegenDeclarationMembership {
            dependencies: CodegenUnitDependencies::new(unit, [module_id]),
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: vec![missing],
            globals: Vec::new(),
            global_instances: Vec::new(),
            vtables: Vec::new(),
        };

        let diagnostics =
            validate_backend_partition_declarations(&declarations, &index, TargetDataLayout::LP64);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].summary.contains("function instance"));
    }

    #[test]
    fn vtable_symbol_uses_evaluated_array_lengths() {
        let mut modules = ModuleIdAllocator::new();
        let module_id = modules.allocate();
        let type_store = TypeStore::new();
        let interner = type_store.append_for_module(module_id);
        let i32_ty = interner.primitive(nia_ty::PrimitiveTy::I32);
        let left_expr = GlobalConstExprId {
            module_id,
            const_expr_id: ConstExprId(0),
        };
        let right_expr = GlobalConstExprId {
            module_id,
            const_expr_id: ConstExprId(1),
        };
        let left = interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstExpr(left_expr),
            elem: i32_ty,
        });
        let right = interner.intern(TyKind::Array {
            len: ArrayLenTy::ConstExpr(right_expr),
            elem: i32_ty,
        });
        drop(interner);

        let mut module = BackendModule {
            id: module_id,
            source_identity: SourceIdentity::new("vtable-symbols.nia"),
            name: "vtable-symbols".to_string(),
            const_eval: BackendConstFacts::default(),
            layouts: BackendLayouts {
                target: TargetDataLayout::LP64,
                types: vec![(i32_ty, TypeLayout { size: 4, align: 4 })],
                structs: Vec::new(),
                unions: Vec::new(),
                enums: Vec::new(),
                struct_instances: Vec::new(),
                union_instances: Vec::new(),
            },
            structs: Vec::new(),
            struct_instances: Vec::new(),
            unions: Vec::new(),
            union_instances: Vec::new(),
            enums: Vec::new(),
            globals: Vec::new(),
            global_instances: Vec::new(),
            functions: Vec::new(),
            function_instances: Vec::new(),
            closure_entries: Vec::new(),
            trait_object_vtables: Vec::new(),
            generic_instantiations: Vec::new(),
        };
        module.const_eval.array_lengths.insert(left_expr, 4);
        module.const_eval.array_lengths.insert(right_expr, 8);
        let store = Arc::new(BackendModuleStore::new([module_id]));
        store.publish(module);
        let (index, mut publisher) = ProgramIndex::new(store, Arc::new(type_store));
        publisher.publish(module_id);

        let left_symbol = expected_trait_object_vtable_symbol(
            &index,
            &nia_backend_ir::BackendTraitObjectVtableKey {
                self_ty: left,
                object_ty: i32_ty,
            },
        );
        let right_symbol = expected_trait_object_vtable_symbol(
            &index,
            &nia_backend_ir::BackendTraitObjectVtableKey {
                self_ty: right,
                object_ty: i32_ty,
            },
        );
        assert_ne!(left_symbol, right_symbol);
    }
}
