// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable fingerprints shared by compiler query keys and cache protocols.

use super::*;

pub(super) const PROVIDER_FACT_WORKLIST_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-fact-worklist");
pub(super) const PROVIDER_FACT_REVISION_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-fact-revision");
pub(super) const PROVIDER_DEMAND_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-demand");
pub(super) const CHECK_CERTIFICATE_INPUT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.check-certificate-input");
pub(super) const MODULE_GRAPH_PATH_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-path");
pub(super) const MODULE_GRAPH_ENTRY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-entry");
pub(super) const MODULE_GRAPH_PARENT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-parent");
pub(super) const MODULE_GRAPH_CHILD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-child");
pub(super) const MODULE_GRAPH_PROVIDER_DEPENDENCIES_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-graph-provider-dependencies");
pub(super) const MODULE_PACKAGE_ROOT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-package-root");
pub(super) const LOADED_MODULES_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.loaded-modules");
pub(super) const PARSE_OK_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.parse-ok-module-ids");
pub(super) const SEMANTIC_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.semantic-module-ids");
pub(super) const MODULE_SOURCE_PATH_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-source-path");
pub(super) const MODULE_SOURCE_VERSION_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.module-source-version");
pub(super) const PUBLIC_SURFACE_MODULE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.public-surface-module");
pub(super) const USING_SCOPE_MODULE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.using-scope-module");
pub(super) const PROGRAM_SIGNATURE_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.program-signature-module-ids");
pub(super) const PROGRAM_SIGNATURE_MODULE_ELIGIBILITY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.program-signature-module-eligibility");
pub(super) const EXTENSION_PROVIDER_MODULE_IDS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.extension-provider-module-ids");
pub(super) const EXTENSION_PROVIDER_MODULE_ELIGIBILITY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.extension-provider-module-eligibility");
pub(super) const PROVIDER_SUMMARY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.compiler.provider-summary");

pub(super) fn provider_fact_worklist_fingerprint(
    worklist: &crate::ProviderFactSnapshot,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(PROVIDER_FACT_WORKLIST_DOMAIN);
    builder.write_fingerprint(provider_fact_revision_fingerprint(worklist.revision()));
    builder.write_fingerprint(provider_fact_revision_fingerprint(
        worklist.reset_revision(),
    ));
    let mut changes = worklist
        .demands()
        .iter()
        .map(provider_demand_fingerprint)
        .collect::<Vec<_>>();
    changes.sort_unstable();
    builder.write_u64(changes.len() as u64);
    for change in changes {
        builder.write_fingerprint(change);
    }
    builder.finish()
}

pub(super) fn provider_fact_revision_fingerprint(
    revision: crate::ProviderFactRevision,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(PROVIDER_FACT_REVISION_DOMAIN);
    for part in revision.fingerprint_parts() {
        builder.write_u64(part);
    }
    builder.finish()
}

pub(super) fn provider_demand_fingerprint(demand: &crate::ProviderDemand) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(PROVIDER_DEMAND_DOMAIN);
    builder.write_str(demand.source_path.identity().normalized_path());
    match &demand.request {
        crate::ProviderRequest::Method {
            target_type_name,
            method_name,
        } => {
            builder.write_u8(0);
            if let Some(target_type_name) = target_type_name {
                builder.write_u8(1);
                builder.write_u64(target_type_name.raw());
            } else {
                builder.write_u8(0);
            }
            builder.write_u64(method_name.raw());
        }
        crate::ProviderRequest::TraitImpl {
            target_type_name,
            trait_name,
            trait_type_argument_names,
        } => {
            builder.write_u8(1);
            if let Some(target_type_name) = target_type_name {
                builder.write_u8(1);
                builder.write_u64(target_type_name.raw());
            } else {
                builder.write_u8(0);
            }
            builder.write_u64(trait_name.raw());
            builder.write_u64(trait_type_argument_names.len() as u64);
            for argument in trait_type_argument_names {
                if let Some(argument) = argument {
                    builder.write_u8(1);
                    builder.write_u64(argument.raw());
                } else {
                    builder.write_u8(0);
                }
            }
        }
        crate::ProviderRequest::ModuleSemantic { module_path } => {
            builder.write_u8(2);
            builder.write_str(module_path.identity().normalized_path());
        }
        crate::ProviderRequest::ModuleBody { module_path } => {
            builder.write_u8(3);
            builder.write_str(module_path.identity().normalized_path());
        }
    }
    builder.finish()
}

pub(super) fn check_certificate_input_fingerprint(
    program_sources: crate::FrontendProgramSourceFingerprint,
    graph: &nia_imports::ModuleGraphSnapshot,
    provider_facts: &crate::ProviderFactSnapshot,
) -> nia_ice::IceResult<FrontendCheckInputFingerprint> {
    let mut builder = QueryFingerprintBuilder::new(CHECK_CERTIFICATE_INPUT_DOMAIN);
    builder.write_fingerprint(QueryFingerprint::from_parts(program_sources.parts()));
    let mut modules = graph.modules().collect::<Vec<_>>();
    modules.sort_unstable_by(|left, right| {
        left.stable_key
            .source_identity()
            .normalized_path()
            .cmp(right.stable_key.source_identity().normalized_path())
    });
    builder.write_u64(modules.len() as u64);
    for module in modules {
        builder.write_str(module.stable_key.source_identity().normalized_path());
        builder.write_u64(module.module_path.package.raw());
        builder.write_u64(module.module_path.segments.len() as u64);
        for segment in &module.module_path.segments {
            builder.write_u64(segment.raw());
        }
        if let Some(parent) = module.parent {
            builder.write_u8(1);
            let parent = graph.stable_key(parent).ok_or_else(|| {
                nia_ice::Ice::new(format!(
                    "module graph parent {parent:?} has no stable identity"
                ))
            })?;
            builder.write_str(parent.source_identity().normalized_path());
        } else {
            builder.write_u8(0);
        }
        builder.write_u8(u8::from(graph.is_executable_root_module(module.id)));
        let mut declarations = module
            .declarations
            .iter()
            .map(|declaration| {
                let target = graph.stable_key(declaration.target).ok_or_else(|| {
                    nia_ice::Ice::new(format!(
                        "module declaration target {:?} has no stable identity",
                        declaration.target
                    ))
                })?;
                Ok((
                    declaration.name.raw(),
                    visibility_tag(declaration.visibility),
                    target.source_identity().normalized_path().to_owned(),
                ))
            })
            .collect::<nia_ice::IceResult<Vec<_>>>()?;
        declarations.sort_unstable();
        builder.write_u64(declarations.len() as u64);
        for (name, visibility, target) in declarations {
            builder.write_u64(name);
            builder.write_u8(visibility);
            builder.write_str(&target);
        }
    }
    let mut demands = provider_facts
        .demands()
        .iter()
        .map(provider_demand_fingerprint)
        .collect::<Vec<_>>();
    demands.sort_unstable();
    builder.write_u64(demands.len() as u64);
    for demand in demands {
        builder.write_fingerprint(demand);
    }
    Ok(FrontendCheckInputFingerprint::from_parts(
        builder.finish().parts(),
    ))
}

pub(super) fn visibility_tag(visibility: nia_ids::Visibility) -> u8 {
    match visibility {
        nia_ids::Visibility::Private => 0,
        nia_ids::Visibility::PublicSuper => 1,
        nia_ids::Visibility::PublicPkg => 2,
        nia_ids::Visibility::Public => 3,
    }
}

pub(super) fn module_graph_path_fingerprint(
    path: &Option<nia_imports::ModulePath>,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(MODULE_GRAPH_PATH_DOMAIN);
    let Some(path) = path else {
        builder.write_u8(0);
        return builder.finish();
    };
    builder.write_u8(1);
    builder.write_u64(path.package.raw());
    builder.write_u64(path.segments.len() as u64);
    for segment in &path.segments {
        builder.write_u64(segment.raw());
    }
    builder.finish()
}

pub(super) fn stable_module_key_fingerprint(
    domain: FingerprintDomain,
    key: &StableModuleKey,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    write_stable_module_key(&mut builder, key);
    builder.finish()
}

pub(super) fn optional_stable_module_key_fingerprint(
    domain: FingerprintDomain,
    key: Option<&StableModuleKey>,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    if let Some(key) = key {
        builder.write_u8(1);
        write_stable_module_key(&mut builder, key);
    } else {
        builder.write_u8(0);
    }
    builder.finish()
}

pub(super) fn module_graph_child_fingerprint(
    child: &Option<(StableModuleKey, nia_ids::Visibility)>,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(MODULE_GRAPH_CHILD_DOMAIN);
    let Some((key, visibility)) = child else {
        builder.write_u8(0);
        return builder.finish();
    };
    builder.write_u8(1);
    write_stable_module_key(&mut builder, key);
    builder.write_u8(match visibility {
        nia_ids::Visibility::Private => 0,
        nia_ids::Visibility::PublicSuper => 1,
        nia_ids::Visibility::PublicPkg => 2,
        nia_ids::Visibility::Public => 3,
    });
    builder.finish()
}

pub(super) fn write_stable_module_key(
    builder: &mut QueryFingerprintBuilder,
    key: &StableModuleKey,
) {
    builder.write_str(key.source_identity().normalized_path());
}

pub(super) fn stable_module_sequence_fingerprint(
    domain: FingerprintDomain,
    sequence: &StableModuleSequence,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u64(sequence.keys.len() as u64);
    for key in &sequence.keys {
        write_stable_module_key(&mut builder, key);
    }
    builder.finish()
}

pub(super) fn source_path_fingerprint(
    domain: FingerprintDomain,
    path: &SourcePath,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_str(path.as_str());
    builder.finish()
}

pub(super) fn source_version_fingerprint(
    domain: FingerprintDomain,
    version: SourceVersion,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u64(u64::from(version.id.store_index()));
    builder.write_u64(u64::from(version.id.local_index()));
    builder.write_u64(version.revision.0);
    builder.finish()
}

pub(super) fn provider_summary_fingerprint(
    summary: &nia_provider_summary::ProviderSummary,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(PROVIDER_SUMMARY_DOMAIN);
    builder.write_u64(summary.providers().len() as u64);
    for provider in summary.providers() {
        provider_type_ref_fingerprint(&mut builder, &provider.target.ty);
        if let Some(trait_ref) = &provider.trait_ref {
            builder.write_u8(1);
            provider_type_ref_fingerprint(&mut builder, trait_ref);
        } else {
            builder.write_u8(0);
        }
        builder.write_u64(provider.associated_methods.len() as u64);
        for method in &provider.associated_methods {
            builder.write_u64(method.raw());
        }
        builder.write_u64(provider.associated_values.len() as u64);
        for value in &provider.associated_values {
            builder.write_u64(value.raw());
        }
    }
    builder.finish()
}

pub(super) fn provider_type_ref_fingerprint(
    builder: &mut QueryFingerprintBuilder,
    type_ref: &nia_provider_summary::ProviderTypeRef,
) {
    if let Some(last_name) = type_ref.last_name {
        builder.write_u8(1);
        builder.write_u64(last_name.raw());
    } else {
        builder.write_u8(0);
    }
    builder.write_u8(u8::from(type_ref.is_generic_or_structural_target));
    builder.write_u8(u8::from(type_ref.semantic_is_conservative));
}

pub(super) fn bool_query_fingerprint(domain: FingerprintDomain, value: bool) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u8(u8::from(value));
    builder.finish()
}
