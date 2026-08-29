// SPDX-License-Identifier: GPL-3.0-or-later
//! Semantic lookups, symbol mangling, and backend type/layout helpers.

use super::*;

impl ModuleLowerer<'_> {
    pub(crate) fn expr_ty(&self, expr: &Expr) -> Option<InternedTyId> {
        self.input.semantic_facts.node_expr_type(&expr.node_key)
    }

    pub(crate) fn receiver_kind_for_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<nia_ids::ReceiverKind> {
        if method_id.module_id == self.input.module_id
            && let Some(signature) = self.input.signatures.functions.get(&method_id.def_id)
        {
            return signature.params.first().and_then(|param| param.receiver);
        }
        self.input
            .program
            .functions()
            .get(&method_id)
            .and_then(|signature| signature.signature.params.first())
            .and_then(|param| param.receiver)
    }

    pub(crate) fn receiver_kind_for_method_or_diagnose(
        &mut self,
        method_id: GlobalDefId,
        span: nia_span::Span,
    ) -> nia_ids::ReceiverKind {
        self.receiver_kind_for_method(method_id).unwrap_or_else(|| {
            self.diagnostics.push(
                nia_diagnostic::Diagnostic::internal_error(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    "resolved method is missing receiver metadata",
                )
                .primary(span, "resolved method is missing receiver metadata")
                .debug("method_id", method_id)
                .finish(),
            );
            nia_ids::ReceiverKind::Value
        })
    }

    pub(crate) fn def_id_for_node(
        &mut self,
        node_key: &VersionedNodeKey,
        expected: DefKind,
    ) -> Option<DefId> {
        let def_id = self.input.defs.def_nodes.get(node_key)?;
        let def = self.input.defs.defs.get(def_id)?;
        if def.kind == expected {
            Some(def_id)
        } else {
            None
        }
    }

    pub(crate) fn def_id_for_node_any_function(
        &mut self,
        node_key: &VersionedNodeKey,
    ) -> Option<DefId> {
        let def_id = self.input.defs.def_nodes.get(node_key)?;
        let def = self.input.defs.defs.get(def_id)?;
        matches!(
            def.kind,
            DefKind::Function | DefKind::Method | DefKind::TraitMethod
        )
        .then_some(def_id)
    }

    pub(crate) fn global_def_id(&self, def_id: DefId) -> GlobalDefId {
        GlobalDefId {
            module_id: self.input.module_id,
            def_id,
        }
    }

    pub(crate) fn mangle_instance_symbol(
        &mut self,
        def_id: GlobalDefId,
        name: SymbolId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> String {
        let defs = &self.input.defs.defs;
        let input = self.input;
        let const_expr_summaries = &self.input.type_lowering.const_expr_summaries;
        let const_array_lengths = self.input.const_array_lengths;
        let source_identities = &self.shared.source_identities;
        let self_arg = self_arg.map(|ty| self.normalize_instance_arg_type(ty));
        let missing_array_len_diagnostics = &mut self.missing_array_len_diagnostics;
        let mut missing_source_identities = HashSet::new();
        let diagnostics = &mut self.diagnostics;
        let def_names = &mut self.def_names;
        let mut args = args.to_vec();
        if let Some(self_arg) = self_arg {
            args.insert(0, self_arg);
        }
        let mut symbol = mangle_instance_symbol_id(
            def_id,
            name,
            &args,
            const_args,
            self.type_store,
            MangleResolvers::new(
                |module_id| {
                    mangle_module_id_or_diagnose(
                        source_identities,
                        module_id,
                        &mut missing_source_identities,
                    )
                },
                |def_id| {
                    if let Some(name) = def_names.get(&def_id) {
                        return name.clone();
                    }
                    let name = program_def(input, def_id)
                        .or_else(|| defs.get(def_id.def_id).cloned())
                        .map(|def| mangle_symbol_id(def.name))
                        .unwrap_or_else(|| format!("def{}", def_id.def_id.0));
                    def_names.insert(def_id, name.clone());
                    name
                },
                |id| {
                    let value = const_array_lengths.get(&id).copied();
                    if value.is_none() && missing_array_len_diagnostics.insert(id) {
                        let span = const_expr_summaries
                            .get(&id)
                            .map(|summary| summary.span)
                            .unwrap_or_default();
                        diagnostics.push(Diagnostic::user_error_at(
                            nia_diagnostic::codes::LLVM_CODEGEN,
                            span,
                            format!(
                                "array length {id:?} was not evaluated before backend symbol generation"
                            ),
                        ));
                    }
                    value
                },
            ),
        );
        for module_id in missing_source_identities {
            record_missing_source_identity(
                module_id,
                &mut self.diagnostics,
                &mut self.missing_source_identity_diagnostics,
            );
        }
        if self_arg.is_some() {
            symbol = symbol.replacen("__inst__t_", "__inst__t_self_", 1);
        }
        symbol
    }

    pub(crate) fn mangle_contextual_instance_symbol(
        &mut self,
        def_id: GlobalDefId,
        name: SymbolId,
        arg_module_id: ModuleId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> String {
        let source_identity = self.shared.source_identities.get(&arg_module_id);
        if source_identity.is_none() {
            record_missing_source_identity(
                arg_module_id,
                &mut self.diagnostics,
                &mut self.missing_source_identity_diagnostics,
            );
        }
        let source_path = source_identity
            .map(|identity| identity.normalized_path().to_string())
            .unwrap_or_else(|| format!("<missing-module-{}>", arg_module_id.local_index()));
        format!(
            "{}__ctx_s{:016x}",
            self.mangle_instance_symbol(def_id, name, self_arg, args, const_args),
            stable_hash(&source_path)
        )
    }

    pub(crate) fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_or_unresolved(self.input.symbols, symbol)
    }

    pub(crate) fn local_name(&self, name: nia_function_ir::LocalName) -> String {
        match name {
            nia_function_ir::LocalName::SelfValue => "self".to_string(),
            nia_function_ir::LocalName::Named(symbol) => self.symbol_name(symbol),
            nia_function_ir::LocalName::Generated(
                nia_function_ir::GeneratedLocalName::ForIterable,
            ) => "__for_iterable".to_string(),
            nia_function_ir::LocalName::Generated(
                nia_function_ir::GeneratedLocalName::ForIterator,
            ) => "__for_iter".to_string(),
            nia_function_ir::LocalName::Generated(nia_function_ir::GeneratedLocalName::ForNext) => {
                "__for_next".to_string()
            }
            nia_function_ir::LocalName::Temporary(id) => format!("fir.tmp.{id}"),
            nia_function_ir::LocalName::Anonymous => "_".to_string(),
        }
    }

    pub(crate) fn function_local_names(
        &self,
        body: &nia_function_ir::FunctionBody,
    ) -> HashMap<nia_ids::LocalId, String> {
        body.locals
            .iter()
            .map(|local| (local.id, self.local_name(local.name)))
            .collect()
    }

    pub(crate) fn def_symbol_name(&self, def_id: GlobalDefId) -> Option<SymbolId> {
        program_def(self.input, def_id)
            .or_else(|| self.input.defs.defs.get(def_id.def_id).cloned())
            .map(|def| def.name)
    }

    pub(crate) fn layout_of(&self, ty: InternedTyId) -> Option<nia_layout::TypeLayout> {
        self.type_context.layout_of(ty)
    }

    pub(crate) fn field_offset(
        &self,
        ty: InternedTyId,
        field: nia_ids::GlobalDefId,
    ) -> Option<u64> {
        self.type_context.field_offset(ty, field)
    }

    pub(crate) fn error_ty(&self) -> InternedTyId {
        self.type_context.append.intern(TyKind::Error)
    }

    pub(crate) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.type_context.ty_kind(ty)
    }
}

fn mangle_module_id_or_diagnose(
    source_identities: &HashMap<ModuleId, nia_source::SourceIdentity>,
    module_id: ModuleId,
    missing_source_identities: &mut HashSet<ModuleId>,
) -> MangleModuleId {
    if let Some(source_identity) = source_identities.get(&module_id) {
        return MangleModuleId::from_normalized_source_path(source_identity.normalized_path());
    }
    missing_source_identities.insert(module_id);
    MangleModuleId::from_normalized_source_path(&format!(
        "<missing-module-{}>",
        module_id.local_index()
    ))
}

fn record_missing_source_identity(
    module_id: ModuleId,
    diagnostics: &mut Vec<Diagnostic>,
    reported: &mut HashSet<ModuleId>,
) {
    if reported.insert(module_id) {
        diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            nia_span::Span::default(),
            format!("missing source identity for mangled module {module_id:?}"),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::ModuleIdAllocator;

    #[test]
    fn missing_mangle_module_identity_is_deterministic_and_tracked() {
        let mut modules = ModuleIdAllocator::new();
        let missing = modules.allocate();
        let mut diagnostics = Vec::new();
        let mut mangle_reported = HashSet::new();
        let identities = HashMap::new();

        let first = mangle_module_id_or_diagnose(&identities, missing, &mut mangle_reported);
        let second = mangle_module_id_or_diagnose(&identities, missing, &mut mangle_reported);

        assert_eq!(first, second);
        assert!(mangle_reported.contains(&missing));
        assert_eq!(mangle_reported.len(), 1);

        let mut reported = HashSet::new();
        record_missing_source_identity(missing, &mut diagnostics, &mut reported);
        record_missing_source_identity(missing, &mut diagnostics, &mut reported);
        assert!(reported.contains(&missing));
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].summary.contains("missing source identity"));
    }
}
