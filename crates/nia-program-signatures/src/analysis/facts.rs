// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq)]
/// Program-qualified declaration facts collected from one module.
pub struct ModuleProgramSignatureFacts {
    /// Trait definition ids declared by the module.
    pub trait_defs: HashSet<GlobalDefId>,
    /// Function signatures keyed by global definition id.
    pub functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    /// Global signatures keyed by global definition id.
    pub globals: HashMap<GlobalDefId, ProgramGlobalSignature>,
    /// Constant signatures keyed by global definition id.
    pub consts: HashMap<GlobalDefId, ProgramConstSignature>,
    /// Struct signatures keyed by global definition id.
    pub structs: HashMap<GlobalDefId, ProgramStructSignature>,
    /// Union signatures keyed by global definition id.
    pub unions: HashMap<GlobalDefId, ProgramUnionSignature>,
    /// Enum signatures keyed by global definition id.
    pub enums: HashMap<GlobalDefId, ProgramEnumSignature>,
    /// Trait signatures keyed by global definition id.
    pub traits: HashMap<GlobalDefId, ProgramTraitSignature>,
    /// Type-alias signatures keyed by global definition id.
    pub type_aliases: HashMap<GlobalDefId, ProgramTypeAliasSignature>,
    /// Program-qualified trait implementations.
    pub trait_impls: Vec<ProgramTraitImplSignature>,
}

/// Collects all program-qualified signature facts for one module.
pub fn collect_module_program_signature_facts(
    module: ModuleSignatureInput<'_>,
) -> ModuleProgramSignatureFacts {
    let trait_defs = module
        .signatures
        .traits
        .keys()
        .map(|def_id| GlobalDefId {
            module_id: module.module_id,
            def_id: *def_id,
        })
        .collect();
    let modules = [module];
    ModuleProgramSignatureFacts {
        trait_defs,
        functions: collect_program_functions_excluding(&modules, &HashSet::new()),
        globals: collect_program_globals(&modules),
        consts: collect_program_consts(&modules),
        structs: collect_program_structs(&modules),
        unions: collect_program_unions(&modules),
        enums: collect_program_enums(&modules),
        traits: collect_program_traits(&modules),
        type_aliases: collect_program_type_aliases(&modules),
        trait_impls: collect_program_trait_impls(&modules),
    }
}

/// Tests whether an active item tree contains facts in the requested set.
pub fn signature_tree_has_program_signature_facts(
    tree: &nia_item_tree::ActiveModuleItemTree,
    set: nia_item_tree::SignatureItemSet,
) -> bool {
    tree.items
        .iter()
        .any(|item| signature_tree_item_has_program_signature_facts(&item.kind, set))
}

fn signature_tree_item_has_program_signature_facts(
    kind: &nia_item_tree::ItemTreeNodeKind,
    set: nia_item_tree::SignatureItemSet,
) -> bool {
    match (kind, set) {
        (
            nia_item_tree::ItemTreeNodeKind::Function(_),
            nia_item_tree::SignatureItemSet::Functions,
        ) => true,
        (
            nia_item_tree::ItemTreeNodeKind::Trait(item),
            nia_item_tree::SignatureItemSet::Functions,
        ) => !item.methods.is_empty(),
        (
            nia_item_tree::ItemTreeNodeKind::Extend(item),
            nia_item_tree::SignatureItemSet::Functions,
        ) => !item.methods.is_empty(),
        (nia_item_tree::ItemTreeNodeKind::Binding(_), nia_item_tree::SignatureItemSet::Values) => {
            true
        }
        (
            nia_item_tree::ItemTreeNodeKind::Extend(item),
            nia_item_tree::SignatureItemSet::Values,
        ) => !item.associated_values.is_empty(),
        (
            nia_item_tree::ItemTreeNodeKind::Struct(_)
            | nia_item_tree::ItemTreeNodeKind::Union(_)
            | nia_item_tree::ItemTreeNodeKind::Enum(_)
            | nia_item_tree::ItemTreeNodeKind::TypeAlias(_),
            nia_item_tree::SignatureItemSet::Types,
        ) => true,
        (
            nia_item_tree::ItemTreeNodeKind::Trait(_) | nia_item_tree::ItemTreeNodeKind::Extend(_),
            nia_item_tree::SignatureItemSet::Traits,
        ) => true,
        (
            nia_item_tree::ItemTreeNodeKind::Trait(item),
            nia_item_tree::SignatureItemSet::ExtensionFunctions,
        ) => !item.methods.is_empty(),
        (
            nia_item_tree::ItemTreeNodeKind::Extend(extend),
            nia_item_tree::SignatureItemSet::ExtensionFunctions,
        ) => !extend.methods.is_empty(),
        _ => false,
    }
}

/// Collects function signatures, omitting explicitly excluded ids.
pub fn collect_program_functions_excluding(
    modules: &[ModuleSignatureInput<'_>],
    excluded: &HashSet<GlobalDefId>,
) -> HashMap<GlobalDefId, ProgramFunctionSignature> {
    let mut functions = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.functions {
            let global_def_id = GlobalDefId {
                module_id: module.module_id,
                def_id: *def_id,
            };
            if excluded.contains(&global_def_id) {
                continue;
            }
            functions.insert(
                global_def_id,
                ProgramFunctionSignature {
                    name: signature.name,
                    signature: signature.clone(),
                },
            );
        }
    }
    functions
}

/// Collects global signatures from the supplied modules.
pub fn collect_program_globals(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramGlobalSignature> {
    let mut globals = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.globals {
            globals.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramGlobalSignature {
                    signature: signature.clone(),
                },
            );
        }
    }
    globals
}

/// Collects constant signatures from the supplied modules.
pub fn collect_program_consts(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramConstSignature> {
    let mut consts = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.consts {
            consts.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramConstSignature {
                    signature: signature.clone(),
                },
            );
        }
    }
    consts
}

/// Collects struct signatures from the supplied modules.
pub fn collect_program_structs(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramStructSignature> {
    let mut structs = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.structs {
            structs.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramStructSignature {
                    signature: signature.clone(),
                },
            );
        }
    }
    structs
}

/// Collects union signatures from the supplied modules.
pub fn collect_program_unions(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramUnionSignature> {
    let mut unions = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.unions {
            unions.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramUnionSignature {
                    signature: signature.clone(),
                },
            );
        }
    }
    unions
}

/// Collects enum signatures from the supplied modules.
pub fn collect_program_enums(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramEnumSignature> {
    let mut enums = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.enums {
            enums.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramEnumSignature {
                    signature: signature.clone(),
                },
            );
        }
    }
    enums
}

/// Collects trait signatures from the supplied modules.
pub fn collect_program_traits(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramTraitSignature> {
    let mut traits = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.traits {
            traits.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramTraitSignature {
                    signature: signature.clone(),
                },
            );
        }
    }
    traits
}

/// Collects type-alias signatures from the supplied modules.
pub fn collect_program_type_aliases(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramTypeAliasSignature> {
    let mut type_aliases = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.type_aliases {
            type_aliases.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramTypeAliasSignature {
                    signature: signature.clone(),
                },
            );
        }
    }
    type_aliases
}

/// Collects non-builtin trait implementations from the supplied modules.
pub fn collect_program_trait_impls(
    modules: &[ModuleSignatureInput<'_>],
) -> Vec<ProgramTraitImplSignature> {
    let mut trait_impls = Vec::new();
    for module in modules {
        for impl_signature in &module.signatures.trait_impls {
            if impl_signature.builtin.is_some() {
                continue;
            }
            let Some(trait_ty) = impl_signature.trait_ty else {
                continue;
            };
            let Some((trait_id, trait_args, trait_const_args)) =
                trait_id_and_args(module.type_store, trait_ty)
            else {
                continue;
            };
            trait_impls.push(ProgramTraitImplSignature {
                module_id: module.module_id,
                impl_id: impl_signature.impl_id,
                builtin: impl_signature.builtin.clone(),
                generics: impl_signature.generics.clone(),
                generic_params: impl_signature.generic_params.clone(),
                target_ty: impl_signature.target_ty,
                trait_id,
                trait_args,
                trait_const_args,
                where_predicates: impl_signature.where_predicates.clone(),
                associated_types: impl_signature.associated_types.clone(),
                associated_values: impl_signature.associated_values.clone(),
            });
        }
    }
    trait_impls
}
