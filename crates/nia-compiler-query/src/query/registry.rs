// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

// These keys either name a module directly or name one of its definitions.
// Register and retire them from the same lists.
macro_rules! module_queries {
    ($visit:ident) => {
        $visit!(
            AbiCheckQuery,
            ActiveModuleItemTreeInputQuery,
            ActiveModuleItemTreeQuery,
            BackendModuleItemPlanQuery,
            BackendModuleFunctionInstancePlanQuery,
            BackendModuleSourceItemPlanQuery,
            BodyCheckQuery,
            CheckedModuleQuery,
            ConstArrayLengthsQuery,
            ConstDeclarationCheckQuery,
            ConstEnumValuesQuery,
            ConstModuleQuery,
            ConstQuery,
            ConstTypedFactsQuery,
            ConstValuesQuery,
            DeclarationActiveModuleItemTreeInputQuery,
            DeclarationActiveModuleItemTreeQuery,
            DeclarationModuleItemTreeInputQuery,
            DeclarationModuleItemTreeQuery,
            DeclarationTypeLoweringQuery,
            DeclarationTypeResolutionQuery,
            ExecutableValueRefItemIndexQuery,
            ExtensionProviderModuleEligibilityQuery,
            ExtensionProviderModuleFactsQuery,
            ExtensionProviderSummaryQuery,
            ExtensionProviderValidationFactsQuery,
            ExtensionSignatureModuleInputQuery,
            ExtensionTraitSolvingModuleFactsQuery,
            FlowCheckQuery,
            InferredModuleValueTypesQuery,
            FullActiveModuleItemTreeInputQuery,
            FullActiveModuleItemTreeQuery,
            FullModuleDefsQuery,
            FullModuleItemTreeInputQuery,
            FullModuleItemTreeQuery,
            ItemSignaturesQuery,
            LayoutTypeNormalizationQuery,
            LayoutsQuery,
            LocalResolutionQuery,
            ModuleLexicalUsingScopesQuery,
            SignatureConstUsingScopesQuery,
            ModuleAbiSignatureFactsQuery,
            ModuleDefsQuery,
            ModuleGraphChildQuery,
            ModuleGraphParentQuery,
            ModuleGraphPathQuery,
            ModuleGraphProviderDependenciesQuery,
            ModuleItemTreeInputQuery,
            ModuleItemTreeQuery,
            ModuleOriginsQuery,
            ModuleNamedRootQuery,
            ModuleParseErrorsQuery,
            ModuleUsingImportsQuery,
            ModulePathQuery,
            ModuleProgramSignatureFactsQuery,
            ModulePublicSurfaceQuery,
            PublicSurfaceModuleFactsQuery,
            ModuleSourceVersionQuery,
            ModuleUsingScopeQuery,
            ProgramSignatureModuleEligibilityQuery,
            PublicSurfaceScopeQuery,
            PublicSurfaceModuleQuery,
            PublicSurfaceTypeQuery,
            PublicSurfaceValueQuery,
            SemanticUseTableQuery,
            FullBodyResolutionInputsQuery,
            ConstructorCheckQuery,
            ConcreteTypeLayoutQuery,
            SignatureConstItemSignaturesQuery,
            SignatureConstItemTreeQuery,
            SignatureConstModuleQuery,
            SignatureConstTypeLoweringQuery,
            SignatureConstTypeNormalizationQuery,
            SignatureConstTypeResolutionQuery,
            SignatureItemSignaturesQuery,
            SignatureItemSignaturesSemanticQuery,
            SignatureItemTreeQuery,
            SignatureLayoutsQuery,
            SignatureTypeLoweringQuery,
            SignatureTypeLoweringSemanticQuery,
            SignatureTypeNormalizationQuery,
            SignatureTypeResolutionQuery,
            SignatureTypeResolutionSemanticQuery,
            StaticCheckQuery,
            TypeLoweringQuery,
            TypeNormalizationQuery,
            TypeResolutionQuery,
            UsingScopeModuleQuery,
            UsingScopeTypeQuery,
            UsingScopeUnresolvedQuery,
            UsingScopeValueQuery,
            ValueResolutionQuery,
            VisibleExtensionsQuery,
            VisibleTraitImplModulesQuery,
            VisibleTraitImplsQuery,
        );
    };
}

macro_rules! definition_queries {
    ($visit:ident) => {
        $visit!(
            ExecutableFunctionBodyQuery,
            ExecutableStaticInitQuery,
            ExecutableValueRefEdgesQuery,
            ExecutableValueRefItemQuery,
            ExtensionMethodByIdQuery,
            LoweredFunctionBodyQuery,
            ProgramTypeAliasSignatureQuery,
        );
    };
}

pub(super) fn compiler_query_registry() -> QueryResult<nia_query::QueryRegistry> {
    let mut registry = nia_query::QueryRegistry::new();
    macro_rules! register {
        ($($key:ty),+ $(,)?) => {
            $(registry.register::<CompilerContext, $key>()?;)+
        };
    }
    module_queries!(register);
    definition_queries!(register);
    register!(
        BodyActivationWorklistQuery,
        BackendFinalizationTaskContextQuery,
        BackendItemPlanQuery,
        BackendLoweringInputsQuery,
        BackendModuleFinalizationQuery,
        BackendLoweringQuery,
        CheckedModuleIdsQuery,
        CheckedProgramQuery,
        CodegenPreparationQuery,
        CodegenProgramQuery,
        CompilerRootScopeQuery,
        CompilerOptimizationQuery,
        CompilerRuntimeQuery,
        CompilerTargetQuery,
        CompilerCpuFeaturesQuery,
        CompilerBuildProfileQuery,
        CurrentModuleIdQuery,
        EntryCheckedProgramQuery,
        ExecutableCheckedModuleFactsQuery,
        ExecutableCheckedModulesQuery,
        ExecutableFactEpochQuery,
        ExecutableProviderDemandsQuery,
        ExecutableRootModulesQuery,
        ExecutableCheckRootsQuery,
        ExtensionMethodIndexQuery,
        ExtensionMethodsNamedQuery,
        ExtensionProviderDiscoveryIndexQuery,
        ExtensionProviderModuleIdsQuery,
        ExtensionTraitImplsForTraitQuery,
        ExtensionTraitSignatureIndexQuery,
        FrontendProgramSourcesQuery,
        LoadedModulesQuery,
        ModuleGraphEntryQuery,
        ModuleGraphQuery,
        ModulePackageRootQuery,
        MonomorphizationQuery,
        ParseOkModuleIdsQuery,
        ProgramAbiSignaturesQuery,
        ProgramLoadDiagnosticsQuery,
        ProgramSignatureModuleIdsQuery,
        ProgramTraitMethodIndexQuery,
        ProviderFactRevisionQuery,
        ProviderFactWorklistQuery,
        PublicSurfacesQuery,
        PublicUsingScopesQuery,
        SemanticModuleIdsQuery,
        TypeExposureIndexQuery,
    );
    Ok(registry)
}

pub(super) fn retire_module_queries(
    db: &QueryDb<CompilerContext>,
    removed: &HashSet<ModuleId>,
) -> QueryResult<usize> {
    db.retire_selected(|selection| {
        macro_rules! module {
            ($($key:ty),+ $(,)?) => {
                $(selection.matching::<$key>(|key| removed.contains(&key.0))?;)+
            };
        }
        macro_rules! definition {
            ($($key:ty),+ $(,)?) => {
                $(selection.matching::<$key>(|key| removed.contains(&key.0.module_id))?;)+
            };
        }
        module_queries!(module);
        definition_queries!(definition);
        selection
            .matching::<BackendModuleFinalizationQuery>(|key| removed.contains(&key.module_id))?;
        selection.matching::<ExtensionTraitImplsForTraitQuery>(
            |key| matches!(key.0, nia_ty::TraitId::Source(id) if removed.contains(&id.module_id)),
        )?;
        Ok(())
    })
}
