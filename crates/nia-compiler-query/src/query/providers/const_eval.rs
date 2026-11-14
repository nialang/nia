// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

/// Executes a substituted semantic term without rechecking its declaration.
pub(super) fn evaluate_const_argument(
    db: &QueryDb<CompilerContext>,
    arg: &nia_ty::ConstGenericArg,
) -> QueryResult<Result<nia_ty::ConstGenericArg, Vec<Diagnostic>>> {
    let nia_ty::ConstGenericValue::Computation(term) = &arg.value else {
        return Ok(Ok(arg.clone()));
    };
    with_const_input_and_program_facts(
        db,
        term.module_id,
        None,
        |_| false,
        |input, _| {
            let evaluated = nia_const_check::evaluate_const_computation(input, arg)?;
            let mut result = evaluated.into_const_generic_arg(
                arg,
                input.type_store,
                input.target.pointer_width(),
            );
            if let Err(diagnostics) = &mut result {
                for diagnostic in diagnostics {
                    if diagnostic.source_path.is_none() {
                        diagnostic.source_path = Some(input.source_path.as_str().to_owned());
                    }
                }
            }
            Ok(result)
        },
    )
}

pub(in crate::query) fn evaluate_const_argument_capturing_failure(
    db: &QueryDb<CompilerContext>,
    failure: &RefCell<Option<QueryError>>,
    arg: &nia_ty::ConstGenericArg,
) -> nia_ice::IceResult<Result<nia_ty::ConstGenericArg, Vec<Diagnostic>>> {
    match evaluate_const_argument(db, arg) {
        Ok(value) => Ok(value),
        Err(error) => {
            *failure.borrow_mut() = Some(error);
            Err(nia_ice::Ice::new("const argument evaluation query failed"))
        }
    }
}

/// Resolves the const expressions written in a declaration type to their
/// values or terms, whichever module wrote them; layout resolves declaration
/// types with this before substituting their generic arguments. A site's
/// identity names its module, whose const analysis owns the site; `lengths`
/// gives that analysis the way the caller's mode reads it, so a type-only
/// module is never checked in full for this.
pub(in crate::query) fn resolve_declaration_const_sites(
    db: &QueryDb<CompilerContext>,
    failure: &RefCell<Option<QueryError>>,
    module_id: ModuleId,
    ty: nia_ids::InternedTyId,
    lengths: &dyn Fn(ModuleId) -> Option<Arc<nia_const_check::ConstArrayLengths>>,
) -> nia_ids::InternedTyId {
    let resolver = |id: &nia_ids::GlobalConstExprId| {
        lengths(id.module_id).and_then(|lengths| lengths.resolve(id))
    };
    let unresolved = nia_type_normalize::TypeNormalization {
        normalized: HashMap::new(),
        diagnostics: Vec::new(),
        aliases: HashMap::new(),
    };
    match unresolved.resolve_array_lengths(module_id, &db.context().type_store, &[ty], &resolver) {
        Ok(resolved) => resolved.normalize(ty),
        Err(error) => {
            *failure.borrow_mut() = Some(error.into());
            ty
        }
    }
}

/// Backend inputs can cross module-finalization worker boundaries. Keep their
/// query failure channel synchronized with the rest of the input contract.
pub(in crate::query) fn evaluate_const_argument_for_backend(
    db: &QueryDb<CompilerContext>,
    failure: &parking_lot::Mutex<Option<QueryError>>,
    arg: &nia_ty::ConstGenericArg,
) -> nia_ice::IceResult<Result<nia_ty::ConstGenericArg, Vec<Diagnostic>>> {
    match evaluate_const_argument(db, arg) {
        Ok(value) => Ok(value),
        Err(error) => {
            *failure.lock() = Some(error);
            Err(nia_ice::Ice::new("const argument evaluation query failed"))
        }
    }
}

pub(super) fn provide_const_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ConstModuleLowering> {
    let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
    let defs = full_module_defs_semantic(db, module_id)?;
    let values = value_resolution_semantic(db, module_id)?;
    let locals = local_resolution_semantic(db, module_id)?;
    let semantic_uses = db.get(SemanticUseTableQuery(module_id))?;
    let type_lowering = type_lowering_semantic(db, module_id)?;
    let signatures = item_signatures_semantic(db, module_id)?;
    let source_path = db.get(ModulePathQuery(module_id))?;
    let symbols = db.context().symbols();
    let defs_for_module = |owner| full_module_defs_semantic(db, owner).ok();
    let signatures_for_module = |owner| item_signatures_semantic(db, owner).ok();
    let visible_extensions_for_module = |owner| {
        db.get(VisibleExtensionsQuery(owner))
            .ok()
            .map(|facts| Arc::clone(&facts.methods))
    };
    Ok(nia_const_check::lower_module_const(
        nia_const_check::ConstModuleInput {
            type_store: &db.context().type_store,
            defs_for_module: Some(&defs_for_module),
            signatures_for_module: Some(&signatures_for_module),
            visible_extensions_for_module: Some(&visible_extensions_for_module),
            active_item_tree: &active_item_tree,
            defs: &defs,
            signatures: &signatures,
            values: &values,
            locals: &locals,
            semantic_uses: &semantic_uses,
            symbols: &symbols,
            const_exprs: &type_lowering.const_exprs,
            source_path: &source_path,
        },
    )?)
}

pub(super) fn provide_const(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleConstCheck> {
    let mut const_eval = time_module_provider(db, "const", module_id, || {
        let array_lengths = Arc::unwrap_or_clone(db.get(ConstArrayLengthsQuery(module_id))?);
        let enum_values = Arc::unwrap_or_clone(db.get(ConstEnumValuesQuery(module_id))?);
        let values = Arc::unwrap_or_clone(db.get(ConstValuesQuery(module_id))?);
        let typed_facts = Arc::unwrap_or_clone(db.get(ConstTypedFactsQuery(module_id))?);

        with_const_input(db, module_id, |_input, module| {
            let mut const_eval = nia_const_check::check_module_const_with_all_phases(
                array_lengths,
                enum_values,
                values,
                typed_facts,
            );
            const_eval.diagnostics.append_from(&module.diagnostics);
            Ok(const_eval)
        })
    })?;
    let declaration_check = super::body_executable::body_check_const_declarations(db, module_id)?;
    const_eval
        .diagnostics
        .extend(declaration_check.diagnostics.iter());
    // Declaration-wide `const fun` checking proves trait obligations too; the
    // implementations it needs must reach the loader like any other demand.
    if !declaration_check.provider_demands.is_empty() {
        Arc::make_mut(&mut const_eval.provider_demands)
            .extend(declaration_check.provider_demands.iter().cloned());
    }
    let diagnostics = std::mem::take(&mut const_eval.diagnostics);
    let source_path = db.get(ModulePathQuery(module_id))?;
    Ok(ModuleConstCheck {
        semantic: Arc::new(const_eval),
        diagnostics: Arc::new(diagnostics),
        rejected_const_functions: Arc::new(rejected_function_diagnostics(
            &declaration_check,
            source_path.as_str(),
        )),
    })
}

/// The functions a body check rejected, each with its errors located in
/// `path`, the file of the checked module.
fn rejected_function_diagnostics(
    check: &nia_body_check::BodyCheck,
    path: &str,
) -> HashMap<GlobalDefId, Vec<Diagnostic>> {
    let mut rejected = HashMap::<GlobalDefId, Vec<Diagnostic>>::new();
    for (index, diagnostic) in check.diagnostics.iter().enumerate() {
        let Some(function) = check.diagnostic_owners.get(index).copied().flatten() else {
            continue;
        };
        if !check.body_check_rejections.functions.contains(&function)
            || diagnostic.severity() != nia_diagnostic::Severity::Error
        {
            continue;
        }
        let mut diagnostic = diagnostic.clone();
        diagnostic
            .source_path
            .get_or_insert_with(|| path.to_owned());
        rejected.entry(function).or_default().push(diagnostic);
    }
    rejected
}

pub(super) fn provide_const_array_lengths(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<nia_const_check::ConstArrayLengths> {
    with_const_input(db, module_id, |input, module| {
        let mut array_lengths = nia_const_check::compute_module_const_array_lengths(input)?;
        array_lengths.diagnostics.append_from(&module.diagnostics);
        Ok(array_lengths)
    })
}

pub(super) fn provide_const_enum_values(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<nia_const_check::ConstEnumValues> {
    let array_lengths = Arc::unwrap_or_clone(db.get(ConstArrayLengthsQuery(module_id))?);
    with_const_input(db, module_id, |input, module| {
        let mut enum_values =
            nia_const_check::compute_module_const_enum_values(input, array_lengths)?;
        enum_values.diagnostics.append_from(&module.diagnostics);
        Ok(enum_values)
    })
}

pub(super) fn provide_const_values(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<nia_const_check::ConstValues> {
    let array_lengths = Arc::unwrap_or_clone(db.get(ConstArrayLengthsQuery(module_id))?);
    let enum_values = Arc::unwrap_or_clone(db.get(ConstEnumValuesQuery(module_id))?);
    with_const_input(db, module_id, |input, module| {
        let mut values =
            nia_const_check::compute_module_const_values(input, array_lengths, enum_values)?;
        values.diagnostics.append_from(&module.diagnostics);
        Ok(values)
    })
}

pub(super) fn provide_const_typed_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<nia_const_check::ConstTypedFacts> {
    let array_lengths = Arc::unwrap_or_clone(db.get(ConstArrayLengthsQuery(module_id))?);
    let enum_values = Arc::unwrap_or_clone(db.get(ConstEnumValuesQuery(module_id))?);
    let values = Arc::unwrap_or_clone(db.get(ConstValuesQuery(module_id))?);
    with_const_input(db, module_id, |input, _module| {
        nia_const_check::compute_module_const_typed_facts(input, array_lengths, enum_values, values)
    })
}

fn with_const_input<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    f: impl FnOnce(nia_const_check::ConstInput<'_>, &ConstModuleLowering) -> nia_ice::IceResult<T>,
) -> QueryResult<T> {
    with_const_input_and_program_facts(db, module_id, None, |_| false, f)
}

pub(super) fn with_const_input_and_program_facts<T>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
    use_signature_facts_for: impl Fn(ModuleId) -> bool,
    f: impl FnOnce(nia_const_check::ConstInput<'_>, &ConstModuleLowering) -> nia_ice::IceResult<T>,
) -> QueryResult<T> {
    let module = db.get(ConstModuleQuery(module_id))?;
    let defs = full_module_defs_semantic(db, module_id)?;
    let query_failure = RefCell::new(None);
    let program_module = |module_id| {
        if use_signature_facts_for(module_id) {
            return capture_query_failure(
                &query_failure,
                signature_const_module_lowering(db, module_id),
            )
            .map(|lowering| Arc::clone(&lowering.module));
        }
        capture_query_failure(&query_failure, db.get(ConstModuleQuery(module_id)))
            .map(|lowering| Arc::clone(&lowering.module))
    };
    let program_source_path = |module_id| {
        capture_query_failure(&query_failure, db.get(ModulePathQuery(module_id)))
            .map(|path| path.as_ref().clone())
    };
    let program_module_path = |module_id| {
        capture_query_failure(&query_failure, db.context().loader_facts().module_graph())
            .and_then(|graph| graph.module_path(module_id))
    };
    let program_source_text = |module_id| {
        capture_query_failure(
            &query_failure,
            db.context().loader_facts().module_source_text(module_id),
        )
        .flatten()
    };
    let program_defs =
        |module_id| capture_query_failure(&query_failure, full_module_defs_semantic(db, module_id));
    let program_type_normalization = |module_id| {
        if use_signature_facts_for(module_id) {
            return capture_query_failure(
                &query_failure,
                signature_type_normalization_semantic(
                    db,
                    module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            );
        }
        capture_query_failure(&query_failure, type_normalization_semantic(db, module_id))
    };
    let local_trait_impls = RefCell::new(None);
    let trait_impls_for_module = |requested_module_id| {
        if requested_module_id == module_id {
            if let Some(signatures) = non_function_signatures_override {
                return Some(signatures.trait_impls.clone());
            }
            if local_trait_impls.borrow().is_none() {
                let facts = capture_query_failure(
                    &query_failure,
                    db.get(VisibleTraitImplsQuery(module_id)),
                )?;
                *local_trait_impls.borrow_mut() = Some(facts);
            }
            return local_trait_impls
                .borrow()
                .as_ref()
                .map(|signatures| signatures.trait_impls.clone());
        }
        if let Some(signatures) = non_function_signatures_override {
            return Some(signatures.trait_impls.clone());
        }
        capture_query_failure(
            &query_failure,
            db.get(VisibleTraitImplsQuery(requested_module_id)),
        )
        .map(|signatures| signatures.trait_impls.clone())
    };
    let program_fieldless_enum = |def_id: GlobalDefId| match non_function_signatures_override
        .and_then(|signatures| signatures.enums.get(&def_id))
    {
        Some(enum_) => enum_.signature.is_fieldless(),
        None => capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            )),
        )
        .is_some_and(|signatures| {
            signatures
                .semantic
                .enums
                .get(&def_id.def_id)
                .is_some_and(nia_item_signatures::EnumSignature::is_fieldless)
        }),
    };
    let item_signatures_for_module = |module_id| {
        if use_signature_facts_for(module_id) {
            return capture_query_failure(
                &query_failure,
                signature_item_signatures_semantic(
                    db,
                    module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            );
        }
        capture_query_failure(&query_failure, item_signatures_semantic(db, module_id))
    };
    let value_signatures_for_module = |module_id| {
        capture_query_failure(
            &query_failure,
            signature_item_signatures_semantic(
                db,
                module_id,
                nia_item_tree::SignatureItemSet::Values,
            ),
        )
    };
    let function_signature = |def_id| {
        capture_query_failure(&query_failure, function_signature_semantic(db, def_id)).flatten()
    };
    let local_visible_extensions = RefCell::new(None);
    let visible_extensions_for_module = |requested_module_id| {
        if requested_module_id == module_id {
            if local_visible_extensions.borrow().is_none() {
                let facts = capture_query_failure(
                    &query_failure,
                    db.get(VisibleExtensionsQuery(module_id)),
                )?;
                *local_visible_extensions.borrow_mut() = Some(facts);
            }
            return local_visible_extensions
                .borrow()
                .as_ref()
                .map(|extensions| extensions.methods.clone());
        }
        capture_query_failure(
            &query_failure,
            db.get(VisibleExtensionsQuery(requested_module_id)),
        )
        .map(|extensions| extensions.methods.clone())
    };
    let values = value_resolution_semantic(db, module_id)?;
    let locals = local_resolution_semantic(db, module_id)?;
    let semantic_uses = db.get(SemanticUseTableQuery(module_id))?;
    let source_path = db.get(ModulePathQuery(module_id))?;
    let source_text = db
        .context()
        .loader_facts()
        .module_source_text(module_id)?
        .unwrap_or_else(|| Arc::from(""));
    let item_signatures = item_signatures_semantic(db, module_id)?;
    let type_lowering = type_lowering_semantic(db, module_id)?;
    let type_normalization = db.get(TypeNormalizationQuery(module_id))?;
    let target = db.get(CompilerTargetQuery)?;
    let symbols = db.context().symbols();
    let input = nia_const_check::ConstInput {
        type_store: &db.context().type_store,
        module: &module.module,
        defs: &defs,
        values: &values,
        locals: &locals,
        semantic_uses: &semantic_uses,
        symbols: &symbols,
        lowered: &type_lowering,
        signatures: &item_signatures,
        normalization: &type_normalization.semantic,
        target: &target,
        source_path: &source_path,
        source_text: &source_text,
        program: nia_const_check::ConstProgramContext {
            module: Some(&program_module),
            source_path: Some(&program_source_path),
            module_path: Some(&program_module_path),
            source_text: Some(&program_source_text),
            defs: Some(&program_defs),
            type_normalizations: Some(&program_type_normalization),
            signatures: Some(&item_signatures_for_module),
            function_signature: Some(&function_signature),
            value_signatures: Some(&value_signatures_for_module),
            const_values: None,
            global_initializer: None,
            program_fieldless_enum: Some(&program_fieldless_enum),
            trait_impls_for_module: Some(&trait_impls_for_module),
            visible_extensions: Some(&visible_extensions_for_module),
        },
    };
    let output = f(input, &module)?;
    match query_failure.into_inner() {
        Some(error) => Err(error),
        None => Ok(output),
    }
}
