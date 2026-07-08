// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleProgramSignatureFacts {
    pub trait_defs: HashSet<GlobalDefId>,
    pub functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub globals: HashMap<GlobalDefId, ProgramGlobalSignature>,
    pub comptimes: HashMap<GlobalDefId, ProgramComptimeSignature>,
    pub structs: HashMap<GlobalDefId, ProgramStructSignature>,
    pub unions: HashMap<GlobalDefId, ProgramUnionSignature>,
    pub enums: HashMap<GlobalDefId, ProgramEnumSignature>,
    pub traits: HashMap<GlobalDefId, ProgramTraitSignature>,
    pub type_aliases: HashMap<GlobalDefId, ProgramTypeAliasSignature>,
    pub trait_impls: Vec<ProgramTraitImplSignature>,
}

pub fn collect_module_program_signature_facts(
    module: ModuleSignatureInput<'_>,
) -> ModuleProgramSignatureFacts {
    let trait_defs = module
        .defs
        .defs
        .iter()
        .filter(|(_, def)| matches!(def.kind, nia_defs::DefKind::Trait))
        .map(|(def_id, _)| GlobalDefId {
            module_id: module.module_id,
            def_id,
        })
        .collect();
    let modules = [module];
    ModuleProgramSignatureFacts {
        trait_defs,
        functions: collect_program_functions_excluding(&modules, &HashSet::new()),
        globals: collect_program_globals(&modules),
        comptimes: collect_program_comptimes(&modules),
        structs: collect_program_structs(&modules),
        unions: collect_program_unions(&modules),
        enums: collect_program_enums(&modules),
        traits: collect_program_traits(&modules),
        type_aliases: collect_program_type_aliases(&modules),
        trait_impls: collect_program_trait_impls(&modules),
    }
}

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
                    name: module
                        .defs
                        .defs
                        .get(*def_id)
                        .map(|def| def.name)
                        .unwrap_or_default(),
                    signature: signature.clone(),
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    functions
}

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
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    globals
}

pub fn collect_program_comptimes(
    modules: &[ModuleSignatureInput<'_>],
) -> HashMap<GlobalDefId, ProgramComptimeSignature> {
    let mut comptimes = HashMap::new();
    for module in modules {
        for (def_id, signature) in &module.signatures.comptimes {
            comptimes.insert(
                GlobalDefId {
                    module_id: module.module_id,
                    def_id: *def_id,
                },
                ProgramComptimeSignature {
                    signature: signature.clone(),
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    comptimes
}

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
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    structs
}

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
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    unions
}

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
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    enums
}

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
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    traits
}

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
                    interner: module.lowering.interner.clone(),
                },
            );
        }
    }
    type_aliases
}

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
                trait_id_and_args(&module.lowering.interner, trait_ty)
            else {
                continue;
            };
            trait_impls.push(ProgramTraitImplSignature {
                module_id: module.module_id,
                impl_id: impl_signature.impl_id,
                builtin: impl_signature.builtin.clone(),
                generics: impl_signature.generics.clone(),
                target_ty: impl_signature.target_ty,
                trait_id,
                trait_args,
                trait_const_args,
                where_predicates: impl_signature.where_predicates.clone(),
                associated_types: impl_signature.associated_types.clone(),
                associated_values: impl_signature.associated_values.clone(),
                interner: module.lowering.interner.clone(),
            });
        }
    }
    trait_impls
}
