// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug)]
pub(super) struct ProviderDemandPlanSource {
    pub(super) path: SourcePath,
    pub(super) fingerprint: SourceContentFingerprint,
    pub(super) len: usize,
}

#[derive(Debug)]
pub(super) struct DecodedProviderDemandPlan {
    pub(super) key: [u64; 2],
    pub(super) namespace: [u64; 2],
    pub(super) entry: String,
    pub(super) module_map: [u64; 2],
    pub(super) package_root_used_paths: bool,
    pub(super) sources: Vec<ProviderDemandPlanSource>,
    pub(super) symbols: Vec<(SymbolId, String)>,
    pub(super) demand_symbols: BTreeSet<SymbolId>,
    pub(super) demands: HashSet<ProviderDemand>,
}

/// Identity header every persisted provider demand plan is validated against.
///
/// A plan is reusable only for the same cache key, namespace, entry source,
/// module map, and package-root processing policy.
#[derive(Clone, Copy)]
pub(crate) struct ProviderDemandPlanIdentity<'a> {
    pub(crate) key: FrontendProviderDemandPlanCacheKey,
    pub(crate) namespace: FrontendCacheNamespace,
    pub(crate) entry: &'a SourceIdentity,
    pub(crate) module_map: FrontendModuleMapFingerprint,
    pub(crate) package_root_used_paths: bool,
}

impl ProviderDemandPlanIdentity<'_> {
    pub(super) fn matches(&self, decoded: &DecodedProviderDemandPlan) -> bool {
        decoded.key == self.key.parts()
            && decoded.namespace == self.namespace.parts()
            && decoded.entry == self.entry.normalized_path()
            && decoded.module_map == self.module_map.parts()
            && decoded.package_root_used_paths == self.package_root_used_paths
    }

    fn write(&self, payload: &mut Vec<u8>) {
        write_parts(payload, self.key.parts());
        write_parts(payload, self.namespace.parts());
        write_string(payload, self.entry.normalized_path());
        write_parts(payload, self.module_map.parts());
        payload.push(u8::from(self.package_root_used_paths));
    }
}

pub(super) fn encode_provider_demand_plan(
    identity: ProviderDemandPlanIdentity<'_>,
    source_paths: &[SourcePath],
    demands: &HashSet<ProviderDemand>,
    sources: &SourceDatabase,
    symbols: &SymbolTable,
) -> io::Result<Vec<u8>> {
    let mut source_paths = source_paths.to_vec();
    source_paths.sort_unstable_by(|left, right| {
        left.identity()
            .normalized_path()
            .cmp(right.identity().normalized_path())
    });
    source_paths.dedup_by(|left, right| left.identity() == right.identity());

    let mut payload = Vec::new();
    identity.write(&mut payload);
    payload.extend_from_slice(&(source_paths.len() as u64).to_le_bytes());
    for path in source_paths {
        let file = sources.read_source(&path)?;
        write_string(&mut payload, path.identity().normalized_path());
        write_parts(&mut payload, source_content_fingerprint(&file.text).parts());
        payload.extend_from_slice(&(file.text.len() as u64).to_le_bytes());
    }

    let mut demand_symbols = BTreeSet::new();
    for demand in demands {
        match demand.request {
            ProviderRequest::Method {
                target_type_name,
                target_package,
                ref target_module_segments,
                method_name,
            } => {
                demand_symbols.extend(target_type_name);
                demand_symbols.insert(target_package);
                demand_symbols.extend(target_module_segments.iter().copied());
                demand_symbols.insert(method_name);
            }
            ProviderRequest::TraitImpl {
                target_type_name,
                target_package,
                ref target_module_segments,
                trait_name,
                trait_package,
                ref trait_type_argument_names,
            } => {
                demand_symbols.extend(target_type_name);
                demand_symbols.extend(target_package);
                demand_symbols.extend(target_module_segments.iter().copied());
                demand_symbols.insert(trait_name);
                demand_symbols.extend(trait_package);
                demand_symbols.extend(trait_type_argument_names.iter().flatten().copied());
            }
            ProviderRequest::ModuleSemantic { .. } | ProviderRequest::ModuleBody { .. } => {}
        }
    }
    write_symbol_dictionary(&mut payload, demand_symbols, symbols)?;

    let mut demands = demands.iter().collect::<Vec<_>>();
    demands.sort_unstable_by(|left, right| compare_provider_demands(left, right));
    payload.extend_from_slice(&(demands.len() as u64).to_le_bytes());
    for demand in demands {
        write_string(
            &mut payload,
            demand.source_path.identity().normalized_path(),
        );
        match &demand.request {
            ProviderRequest::Method {
                target_type_name,
                target_package,
                target_module_segments,
                method_name,
            } => {
                payload.push(0);
                write_optional_symbol(&mut payload, *target_type_name);
                payload.extend_from_slice(&target_package.raw().to_le_bytes());
                payload.extend_from_slice(&(target_module_segments.len() as u64).to_le_bytes());
                for segment in target_module_segments {
                    payload.extend_from_slice(&segment.raw().to_le_bytes());
                }
                payload.extend_from_slice(&method_name.raw().to_le_bytes());
            }
            ProviderRequest::TraitImpl {
                target_type_name,
                target_package,
                target_module_segments,
                trait_name,
                trait_package,
                trait_type_argument_names,
            } => {
                payload.push(1);
                write_optional_symbol(&mut payload, *target_type_name);
                write_optional_symbol(&mut payload, *target_package);
                payload.extend_from_slice(&(target_module_segments.len() as u64).to_le_bytes());
                for segment in target_module_segments {
                    payload.extend_from_slice(&segment.raw().to_le_bytes());
                }
                payload.extend_from_slice(&trait_name.raw().to_le_bytes());
                write_optional_symbol(&mut payload, *trait_package);
                payload.extend_from_slice(&(trait_type_argument_names.len() as u64).to_le_bytes());
                for argument in trait_type_argument_names {
                    write_optional_symbol(&mut payload, *argument);
                }
            }
            ProviderRequest::ModuleSemantic { module_path } => {
                payload.push(2);
                write_string(&mut payload, module_path.identity().normalized_path());
            }
            ProviderRequest::ModuleBody { module_path } => {
                payload.push(3);
                write_string(&mut payload, module_path.identity().normalized_path());
            }
        }
    }

    let mut encoded = Vec::with_capacity(48 + payload.len());
    encoded.extend_from_slice(FRONTEND_PROVIDER_DEMAND_PLAN.magic);
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    write_parts(
        &mut encoded,
        provider_demand_plan_checksum(&payload).parts(),
    );
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

pub(super) fn decode_provider_demand_plan(encoded: &[u8]) -> Option<DecodedProviderDemandPlan> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *FRONTEND_PROVIDER_DEMAND_PLAN.magic).then_some(())?;
    let payload_len = read_len(&mut cursor, MAX_CACHE_PAYLOAD_BYTES)?;
    let checksum = QueryFingerprint::from_parts(read_parts(&mut cursor)?);
    let mut payload = vec![0; payload_len];
    cursor.read_exact(&mut payload).ok()?;
    (usize::try_from(cursor.position()).ok()? == encoded.len()).then_some(())?;
    (provider_demand_plan_checksum(&payload) == checksum).then_some(())?;

    let mut cursor = Cursor::new(payload.as_slice());
    let key = read_parts(&mut cursor)?;
    let namespace = read_parts(&mut cursor)?;
    let entry = read_canonical_source_path(&mut cursor, payload_len)?;
    let module_map = read_parts(&mut cursor)?;
    let package_root_used_paths = read_bool(&mut cursor)?;
    let source_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut manifest_sources = Vec::with_capacity(source_len);
    for _ in 0..source_len {
        let path = SourcePath::from_normalized_unchecked(read_canonical_source_path(
            &mut cursor,
            payload_len,
        )?);
        let fingerprint = SourceContentFingerprint::from_parts(read_parts(&mut cursor)?);
        let len = usize::try_from(read_u64(&mut cursor)?).ok()?;
        manifest_sources.push(ProviderDemandPlanSource {
            path,
            fingerprint,
            len,
        });
    }
    manifest_sources
        .windows(2)
        .all(|pair| pair[0].path.as_str() < pair[1].path.as_str())
        .then_some(())?;

    let symbols = read_symbol_dictionary(&mut cursor, payload_len)?;
    let demand_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut demands = Vec::with_capacity(demand_len);
    let mut demand_symbols = BTreeSet::new();
    for _ in 0..demand_len {
        let source_path = SourcePath::from_normalized_unchecked(read_canonical_source_path(
            &mut cursor,
            payload_len,
        )?);
        let request = match read_u8(&mut cursor)? {
            0 => {
                let target_type_name = read_optional_symbol(&mut cursor)?;
                let target_package = read_symbol(&mut cursor)?;
                let target_module_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
                let mut target_module_segments = Vec::with_capacity(target_module_len);
                for _ in 0..target_module_len {
                    target_module_segments.push(read_symbol(&mut cursor)?);
                }
                let method_name = read_symbol(&mut cursor)?;
                demand_symbols.extend(target_type_name);
                demand_symbols.insert(target_package);
                demand_symbols.extend(target_module_segments.iter().copied());
                demand_symbols.insert(method_name);
                ProviderRequest::Method {
                    target_type_name,
                    target_package,
                    target_module_segments,
                    method_name,
                }
            }
            1 => {
                let target_type_name = read_optional_symbol(&mut cursor)?;
                let target_package = read_optional_symbol(&mut cursor)?;
                let target_module_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
                let mut target_module_segments = Vec::with_capacity(target_module_len);
                for _ in 0..target_module_len {
                    target_module_segments.push(read_symbol(&mut cursor)?);
                }
                let trait_name = read_symbol(&mut cursor)?;
                let trait_package = read_optional_symbol(&mut cursor)?;
                let argument_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
                let mut trait_type_argument_names = Vec::with_capacity(argument_len);
                for _ in 0..argument_len {
                    trait_type_argument_names.push(read_optional_symbol(&mut cursor)?);
                }
                demand_symbols.extend(target_type_name);
                demand_symbols.extend(target_package);
                demand_symbols.extend(target_module_segments.iter().copied());
                demand_symbols.insert(trait_name);
                demand_symbols.extend(trait_package);
                demand_symbols.extend(trait_type_argument_names.iter().flatten().copied());
                ProviderRequest::TraitImpl {
                    target_type_name,
                    target_package,
                    target_module_segments,
                    trait_name,
                    trait_package,
                    trait_type_argument_names,
                }
            }
            2 => ProviderRequest::ModuleSemantic {
                module_path: SourcePath::from_normalized_unchecked(read_canonical_source_path(
                    &mut cursor,
                    payload_len,
                )?),
            },
            3 => ProviderRequest::ModuleBody {
                module_path: SourcePath::from_normalized_unchecked(read_canonical_source_path(
                    &mut cursor,
                    payload_len,
                )?),
            },
            _ => return None,
        };
        demands.push(ProviderDemand {
            source_path,
            request,
        });
    }
    demands
        .windows(2)
        .all(|pair| compare_provider_demands(&pair[0], &pair[1]).is_lt())
        .then_some(())?;
    (usize::try_from(cursor.position()).ok()? == payload_len).then_some(())?;
    Some(DecodedProviderDemandPlan {
        key,
        namespace,
        entry,
        module_map,
        package_root_used_paths,
        sources: manifest_sources,
        symbols,
        demand_symbols,
        demands: demands.into_iter().collect(),
    })
}

pub(super) fn provider_demand_plan_paths_are_closed(plan: &DecodedProviderDemandPlan) -> bool {
    let manifest = plan
        .sources
        .iter()
        .map(|source| source.path.as_str())
        .collect::<HashSet<_>>();
    // Every path carried by a demand must be covered by the fingerprinted
    // source manifest; otherwise a cache hit could depend on unchecked input.
    manifest.contains(plan.entry.as_str())
        && plan.demands.iter().all(|demand| {
            manifest.contains(demand.source_path.as_str())
                && match &demand.request {
                    ProviderRequest::Method { .. } | ProviderRequest::TraitImpl { .. } => true,
                    ProviderRequest::ModuleSemantic { module_path }
                    | ProviderRequest::ModuleBody { module_path } => {
                        manifest.contains(module_path.as_str())
                    }
                }
        })
}

pub(super) fn remap_provider_demands(
    demands: HashSet<ProviderDemand>,
    source_roots: &[SourcePath],
) -> Option<HashSet<ProviderDemand>> {
    demands
        .into_iter()
        .map(|demand| {
            let source_path =
                resolve_cached_source_path(&demand.source_path.identity(), source_roots)?;
            let request = match demand.request {
                request @ ProviderRequest::Method { .. } => request,
                ProviderRequest::TraitImpl {
                    target_type_name,
                    trait_name,
                    trait_type_argument_names,
                    target_package,
                    target_module_segments,
                    trait_package,
                } => ProviderRequest::TraitImpl {
                    target_type_name,
                    target_package,
                    target_module_segments,
                    trait_name,
                    trait_package,
                    trait_type_argument_names,
                },
                ProviderRequest::ModuleSemantic { module_path } => {
                    ProviderRequest::ModuleSemantic {
                        module_path: resolve_cached_source_path(
                            &module_path.identity(),
                            source_roots,
                        )?,
                    }
                }
                ProviderRequest::ModuleBody { module_path } => ProviderRequest::ModuleBody {
                    module_path: resolve_cached_source_path(&module_path.identity(), source_roots)?,
                },
            };
            Some(ProviderDemand {
                source_path,
                request,
            })
        })
        .collect()
}

pub(super) fn resolve_cached_source_path(
    identity: &SourceIdentity,
    source_roots: &[SourcePath],
) -> Option<SourcePath> {
    let logical = identity.normalized_path();
    if let Some(root) = source_roots
        .iter()
        .find(|root| root.identity().normalized_path() == logical)
    {
        return Some(root.clone());
    }

    // Cache identities are logical and stable, while source roots may move.
    // A tree root's modules live in the root file's directory, so rebuild a
    // physical path below the longest matching root directory.
    let remapped = source_roots
        .iter()
        .filter_map(|root| {
            let root_identity = root.identity();
            let logical_root = root_identity
                .normalized_path()
                .rsplit_once('/')
                .map_or("", |(directory, _)| directory);
            let physical_root = root
                .as_str()
                .rsplit_once('/')
                .map_or("", |(directory, _)| directory);
            let suffix = if logical_root.is_empty() {
                logical
            } else {
                logical.strip_prefix(logical_root)?.strip_prefix('/')?
            };
            let physical = if physical_root.is_empty() {
                suffix.to_owned()
            } else {
                format!("{physical_root}/{suffix}")
            };
            Some((
                logical_root.len(),
                SourcePath::with_identity(physical, logical),
            ))
        })
        .max_by_key(|(prefix_len, _)| *prefix_len)
        .map(|(_, path)| path);
    if remapped.is_some() {
        return remapped;
    }

    (!logical.contains(":/")).then(|| SourcePath::new(logical))
}

fn read_canonical_source_path(cursor: &mut Cursor<&[u8]>, limit: usize) -> Option<String> {
    let path = read_string(cursor, limit)?;
    (SourcePath::new(&path).as_str() == path).then_some(path)
}

fn compare_provider_demands(left: &ProviderDemand, right: &ProviderDemand) -> std::cmp::Ordering {
    left.source_path
        .identity()
        .normalized_path()
        .cmp(right.source_path.identity().normalized_path())
        .then_with(|| compare_provider_requests(&left.request, &right.request))
}

fn compare_provider_requests(
    left: &ProviderRequest,
    right: &ProviderRequest,
) -> std::cmp::Ordering {
    let tag = |request: &ProviderRequest| match request {
        ProviderRequest::Method { .. } => 0_u8,
        ProviderRequest::TraitImpl { .. } => 1,
        ProviderRequest::ModuleSemantic { .. } => 2,
        ProviderRequest::ModuleBody { .. } => 3,
    };
    tag(left)
        .cmp(&tag(right))
        .then_with(|| match (left, right) {
            (
                ProviderRequest::Method {
                    target_type_name: left_target,
                    target_package: left_package,
                    target_module_segments: left_segments,
                    method_name: left_method,
                },
                ProviderRequest::Method {
                    target_type_name: right_target,
                    target_package: right_package,
                    target_module_segments: right_segments,
                    method_name: right_method,
                },
            ) => left_target
                .map(SymbolId::raw)
                .cmp(&right_target.map(SymbolId::raw))
                .then_with(|| left_package.raw().cmp(&right_package.raw()))
                .then_with(|| {
                    left_segments
                        .iter()
                        .map(|symbol| symbol.raw())
                        .cmp(right_segments.iter().map(|symbol| symbol.raw()))
                })
                .then_with(|| left_method.raw().cmp(&right_method.raw())),
            (
                ProviderRequest::TraitImpl {
                    target_type_name: left_target,
                    target_package: left_target_package,
                    target_module_segments: left_module_segments,
                    trait_name: left_trait,
                    trait_package: left_trait_package,
                    trait_type_argument_names: left_arguments,
                    ..
                },
                ProviderRequest::TraitImpl {
                    target_type_name: right_target,
                    target_package: right_target_package,
                    target_module_segments: right_module_segments,
                    trait_name: right_trait,
                    trait_package: right_trait_package,
                    trait_type_argument_names: right_arguments,
                    ..
                },
            ) => left_target
                .map(SymbolId::raw)
                .cmp(&right_target.map(SymbolId::raw))
                .then_with(|| {
                    left_target_package
                        .map(SymbolId::raw)
                        .cmp(&right_target_package.map(SymbolId::raw))
                })
                .then_with(|| {
                    left_module_segments
                        .iter()
                        .map(|symbol| symbol.raw())
                        .cmp(right_module_segments.iter().map(|symbol| symbol.raw()))
                })
                .then_with(|| left_trait.raw().cmp(&right_trait.raw()))
                .then_with(|| {
                    left_trait_package
                        .map(SymbolId::raw)
                        .cmp(&right_trait_package.map(SymbolId::raw))
                })
                .then_with(|| {
                    left_arguments
                        .iter()
                        .map(|argument| argument.map(SymbolId::raw))
                        .cmp(
                            right_arguments
                                .iter()
                                .map(|argument| argument.map(SymbolId::raw)),
                        )
                }),
            (
                ProviderRequest::ModuleSemantic {
                    module_path: left_path,
                },
                ProviderRequest::ModuleSemantic {
                    module_path: right_path,
                },
            )
            | (
                ProviderRequest::ModuleBody {
                    module_path: left_path,
                },
                ProviderRequest::ModuleBody {
                    module_path: right_path,
                },
            ) => left_path
                .identity()
                .normalized_path()
                .cmp(right_path.identity().normalized_path()),
            _ => std::cmp::Ordering::Equal,
        })
}
