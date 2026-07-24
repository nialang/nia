use nia_ast::FunctionItem;
use nia_imports::{ModuleMap, StableModuleKey};
use nia_item_tree::{ItemTreeNodeKind, ModuleItemTree, SignatureItemSet};
use nia_query::{QueryFingerprint, QueryFingerprintBuilder};
use nia_span::Span;
use nia_syntax::SyntaxTree;
use nia_target_config::TargetConfig;

use crate::RuntimeModel;

const FRONTEND_CACHE_SCHEMA_VERSION: u64 = 1;

macro_rules! frontend_fingerprint {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(QueryFingerprint);

        impl $name {
            pub const fn from_parts(parts: [u64; 2]) -> Self {
                Self(QueryFingerprint::from_parts(parts))
            }

            pub const fn parts(self) -> [u64; 2] {
                self.0.parts()
            }
        }
    };
}

frontend_fingerprint!(SourceContentFingerprint);
frontend_fingerprint!(SyntaxFingerprint);
frontend_fingerprint!(ItemSignatureFingerprint);
frontend_fingerprint!(FrontendCacheNamespace);
frontend_fingerprint!(FrontendModuleMapFingerprint);
frontend_fingerprint!(FrontendProgramSourceFingerprint);

macro_rules! frontend_cache_key {
    ($name:ident, $fingerprint:ident, $domain:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(QueryFingerprint);

        impl $name {
            pub fn new(
                namespace: FrontendCacheNamespace,
                module: &StableModuleKey,
                fingerprint: $fingerprint,
            ) -> Self {
                let mut builder = QueryFingerprintBuilder::new($domain);
                write_frontend_cache_key(&mut builder, namespace, module, fingerprint.parts());
                Self(builder.finish())
            }

            pub const fn from_parts(parts: [u64; 2]) -> Self {
                Self(QueryFingerprint::from_parts(parts))
            }

            pub const fn parts(self) -> [u64; 2] {
                self.0.parts()
            }
        }
    };
}

frontend_cache_key!(
    FrontendSourceCacheKey,
    SourceContentFingerprint,
    "nia.frontend.cache-key.source.v1"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrontendSignatureTypeResolutionCacheKey(QueryFingerprint);

impl FrontendSignatureTypeResolutionCacheKey {
    pub fn new(
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        set: SignatureItemSet,
        program_sources: FrontendProgramSourceFingerprint,
    ) -> Self {
        let mut builder =
            QueryFingerprintBuilder::new("nia.frontend.cache-key.signature-type-resolution.v1");
        write_frontend_cache_key(&mut builder, namespace, module, program_sources.parts());
        builder.write_u8(signature_item_set_tag(set));
        Self(builder.finish())
    }

    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(QueryFingerprint::from_parts(parts))
    }

    pub const fn parts(self) -> [u64; 2] {
        self.0.parts()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrontendSignatureTypeLoweringCacheKey(QueryFingerprint);

impl FrontendSignatureTypeLoweringCacheKey {
    pub fn new(
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        set: SignatureItemSet,
        program_sources: FrontendProgramSourceFingerprint,
    ) -> Self {
        let mut builder =
            QueryFingerprintBuilder::new("nia.frontend.cache-key.signature-type-lowering.v1");
        write_frontend_cache_key(&mut builder, namespace, module, program_sources.parts());
        builder.write_u8(signature_item_set_tag(set));
        Self(builder.finish())
    }

    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(QueryFingerprint::from_parts(parts))
    }

    pub const fn parts(self) -> [u64; 2] {
        self.0.parts()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrontendSignatureItemSignaturesCacheKey(QueryFingerprint);

impl FrontendSignatureItemSignaturesCacheKey {
    pub fn new(
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        set: SignatureItemSet,
        program_sources: FrontendProgramSourceFingerprint,
    ) -> Self {
        let mut builder =
            QueryFingerprintBuilder::new("nia.frontend.cache-key.signature-item-signatures.v1");
        write_frontend_cache_key(&mut builder, namespace, module, program_sources.parts());
        builder.write_u8(signature_item_set_tag(set));
        Self(builder.finish())
    }

    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(QueryFingerprint::from_parts(parts))
    }

    pub const fn parts(self) -> [u64; 2] {
        self.0.parts()
    }
}
frontend_cache_key!(
    FrontendSyntaxCacheKey,
    SyntaxFingerprint,
    "nia.frontend.cache-key.syntax.v1"
);
frontend_cache_key!(
    FrontendItemSignatureCacheKey,
    ItemSignatureFingerprint,
    "nia.frontend.cache-key.item-signature.v1"
);
frontend_cache_key!(
    FrontendProviderSummaryCacheKey,
    ItemSignatureFingerprint,
    "nia.frontend.cache-key.provider-summary.v2"
);
frontend_cache_key!(
    FrontendPublicSurfaceFactsCacheKey,
    SourceContentFingerprint,
    "nia.frontend.cache-key.public-surface-facts.v1"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrontendFacadeFactsCacheKey(QueryFingerprint);

impl FrontendFacadeFactsCacheKey {
    pub fn new(
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        item_signature: ItemSignatureFingerprint,
        module_map: FrontendModuleMapFingerprint,
    ) -> Self {
        let mut builder = QueryFingerprintBuilder::new("nia.frontend.cache-key.facade-facts.v1");
        write_frontend_cache_key(&mut builder, namespace, module, item_signature.parts());
        builder.write_fingerprint(QueryFingerprint::from_parts(module_map.parts()));
        Self(builder.finish())
    }

    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(QueryFingerprint::from_parts(parts))
    }

    pub const fn parts(self) -> [u64; 2] {
        self.0.parts()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrontendModuleDependenciesCacheKey(QueryFingerprint);

impl FrontendModuleDependenciesCacheKey {
    pub fn new(
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        source: SourceContentFingerprint,
        module_map: FrontendModuleMapFingerprint,
    ) -> Self {
        let mut builder =
            QueryFingerprintBuilder::new("nia.frontend.cache-key.module-dependencies.v1");
        write_frontend_cache_key(&mut builder, namespace, module, source.parts());
        builder.write_fingerprint(QueryFingerprint::from_parts(module_map.parts()));
        Self(builder.finish())
    }

    pub const fn from_parts(parts: [u64; 2]) -> Self {
        Self(QueryFingerprint::from_parts(parts))
    }

    pub const fn parts(self) -> [u64; 2] {
        self.0.parts()
    }
}

impl FrontendCacheNamespace {
    pub fn new(target: &TargetConfig, runtime: RuntimeModel) -> Self {
        let mut builder = QueryFingerprintBuilder::new("nia.frontend.cache-namespace.v1");
        builder.write_u64(FRONTEND_CACHE_SCHEMA_VERSION);
        builder.write_str(env!("CARGO_PKG_VERSION"));
        for field in [
            target.arch.as_str(),
            target.vendor.as_str(),
            target.os.as_str(),
            target.env.as_str(),
            target.abi.as_str(),
            target.endian.as_str(),
        ] {
            builder.write_str(field);
        }
        builder.write_u64(u64::from(target.pointer_width));
        builder.write_u8(match runtime {
            RuntimeModel::Bare => 0,
            RuntimeModel::FreestandingExecutable => 1,
        });
        Self(builder.finish())
    }
}

fn write_frontend_cache_key(
    builder: &mut QueryFingerprintBuilder,
    namespace: FrontendCacheNamespace,
    module: &StableModuleKey,
    fingerprint: [u64; 2],
) {
    builder.write_fingerprint(QueryFingerprint::from_parts(namespace.parts()));
    builder.write_str(module.source_identity().normalized_path());
    builder.write_fingerprint(QueryFingerprint::from_parts(fingerprint));
}

pub fn source_content_fingerprint(source: &str) -> SourceContentFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.frontend.source-content.v1");
    builder.write_bytes(source.as_bytes());
    SourceContentFingerprint(builder.finish())
}

pub fn frontend_program_source_fingerprint<'a>(
    sources: impl IntoIterator<Item = (&'a StableModuleKey, SourceContentFingerprint, usize)>,
) -> FrontendProgramSourceFingerprint {
    let mut sources = sources.into_iter().collect::<Vec<_>>();
    sources.sort_unstable_by(|left, right| {
        left.0
            .source_identity()
            .normalized_path()
            .cmp(right.0.source_identity().normalized_path())
    });
    let mut builder = QueryFingerprintBuilder::new("nia.frontend.program-sources.v1");
    builder.write_u64(sources.len() as u64);
    for (module, source, len) in sources {
        builder.write_str(module.source_identity().normalized_path());
        builder.write_fingerprint(QueryFingerprint::from_parts(source.parts()));
        builder.write_u64(len as u64);
    }
    FrontendProgramSourceFingerprint(builder.finish())
}

fn signature_item_set_tag(set: SignatureItemSet) -> u8 {
    match set {
        SignatureItemSet::Functions => 0,
        SignatureItemSet::ExtensionFunctions => 1,
        SignatureItemSet::Values => 2,
        SignatureItemSet::Types => 3,
        SignatureItemSet::Traits => 4,
    }
}

pub fn frontend_module_map_fingerprint(module_map: &ModuleMap) -> FrontendModuleMapFingerprint {
    let mut entries = module_map
        .entries()
        .map(|(name, path)| (name.raw(), path.identity()))
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.normalized_path().cmp(right.1.normalized_path()))
    });
    let mut builder = QueryFingerprintBuilder::new("nia.frontend.module-map.v1");
    builder.write_u64(entries.len() as u64);
    for (name, path) in entries {
        builder.write_u64(name);
        builder.write_str(path.normalized_path());
    }
    FrontendModuleMapFingerprint(builder.finish())
}

pub fn syntax_fingerprint(syntax: &SyntaxTree) -> SyntaxFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.frontend.lossless-syntax.v1");
    builder.write_bytes(syntax.source().as_bytes());
    SyntaxFingerprint(builder.finish())
}

pub fn item_signature_fingerprint(
    syntax: &SyntaxTree,
    item_tree: &ModuleItemTree,
) -> ItemSignatureFingerprint {
    let source = syntax.source();
    let mut body_spans = Vec::new();
    for item in &item_tree.items {
        match &item.kind {
            ItemTreeNodeKind::Function(function) => push_body_span(function, &mut body_spans),
            ItemTreeNodeKind::Trait(item_trait) => {
                for method in &item_trait.methods {
                    push_body_span(&method.function, &mut body_spans);
                }
            }
            ItemTreeNodeKind::Extend(extend) => {
                for method in &extend.methods {
                    push_body_span(&method.function, &mut body_spans);
                }
            }
            ItemTreeNodeKind::Module(_)
            | ItemTreeNodeKind::Using(_)
            | ItemTreeNodeKind::Struct(_)
            | ItemTreeNodeKind::Union(_)
            | ItemTreeNodeKind::Enum(_)
            | ItemTreeNodeKind::TypeAlias(_)
            | ItemTreeNodeKind::Binding(_) => {}
        }
    }
    body_spans.sort_unstable_by_key(|span| (span.start, span.end));

    let mut builder = QueryFingerprintBuilder::new("nia.frontend.item-signature.v1");
    let mut cursor = 0;
    for span in body_spans {
        assert!(
            cursor <= span.start && span.start <= span.end && span.end <= source.len(),
            "Nia ICE: function body spans must be ordered within source bounds"
        );
        let prefix = source
            .get(cursor..span.start)
            .expect("Nia ICE: function body span must lie on UTF-8 boundaries");
        builder.write_bytes(prefix.as_bytes());
        builder.write_u8(1);
        cursor = span.end;
    }
    let suffix = source
        .get(cursor..)
        .expect("Nia ICE: function body span must end on a UTF-8 boundary");
    builder.write_bytes(suffix.as_bytes());
    ItemSignatureFingerprint(builder.finish())
}

fn push_body_span(function: &FunctionItem, spans: &mut Vec<Span>) {
    if let Some(body) = &function.body {
        spans.push(body.span);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_imports::StableModuleKey;
    use nia_source::SourceIdentity;
    use nia_source::{SourceId, SourcePath, SourceRevision, SourceVersion};

    #[test]
    fn source_and_syntax_fingerprints_are_version_independent_and_domain_separated() {
        let source = "fn main() i32 { 0 }";
        let first = SyntaxTree::parse(
            source,
            Some(SourceVersion {
                id: SourceId(1),
                revision: SourceRevision(2),
            }),
        );
        let second = SyntaxTree::parse(
            source,
            Some(SourceVersion {
                id: SourceId(9),
                revision: SourceRevision(7),
            }),
        );

        assert_eq!(syntax_fingerprint(&first), syntax_fingerprint(&second));
        assert_ne!(
            source_content_fingerprint(source).parts(),
            syntax_fingerprint(&first).parts()
        );
        assert_eq!(std::mem::size_of::<SourceContentFingerprint>(), 16);
        assert_eq!(std::mem::size_of::<SyntaxFingerprint>(), 16);
        assert_eq!(std::mem::size_of::<ItemSignatureFingerprint>(), 16);
    }

    #[test]
    fn item_signature_fingerprint_excludes_function_and_method_bodies() {
        let before = signature_fingerprint(
            r#"
fn main(value: i32) i32 { value + 1 }
trait Value {
    fn get(self) i32 { 1 }
}
extend Value {
    fn twice(self) i32 { self.get() * 2 }
}
"#,
        );
        let body_edit = signature_fingerprint(
            r#"
fn main(value: i32) i32 { value + 200 }
trait Value {
    fn get(self) i32 { 99 }
}
extend Value {
    fn twice(self) i32 { self.get() * 400 }
}
"#,
        );
        let signature_edit = signature_fingerprint(
            r#"
fn main(value: i64) i32 { value + 200 }
trait Value {
    fn get(self) i32 { 99 }
}
extend Value {
    fn twice(self) i32 { self.get() * 400 }
}
"#,
        );

        assert_eq!(before, body_edit);
        assert_ne!(before, signature_edit);
    }

    #[test]
    fn exact_source_and_syntax_fingerprints_track_body_edits() {
        let before = "fn main() i32 { 1 }";
        let after = "fn main() i32 { 2 }";

        assert_ne!(
            source_content_fingerprint(before),
            source_content_fingerprint(after)
        );
        assert_ne!(
            syntax_fingerprint(&SyntaxTree::parse(before, None)),
            syntax_fingerprint(&SyntaxTree::parse(after, None))
        );
    }

    #[test]
    fn frontend_cache_namespace_covers_target_and_runtime() {
        let target = TargetConfig {
            arch: "x86_64".to_string(),
            vendor: "unknown".to_string(),
            os: "linux".to_string(),
            env: "gnu".to_string(),
            abi: "elf".to_string(),
            endian: "little".to_string(),
            pointer_width: 64,
        };
        let baseline = FrontendCacheNamespace::new(&target, RuntimeModel::Bare);
        let mut variants = Vec::new();
        for field in 0..7 {
            let mut changed = target.clone();
            match field {
                0 => changed.arch.push_str("-changed"),
                1 => changed.vendor.push_str("-changed"),
                2 => changed.os.push_str("-changed"),
                3 => changed.env.push_str("-changed"),
                4 => changed.abi.push_str("-changed"),
                5 => changed.endian.push_str("-changed"),
                6 => changed.pointer_width = 32,
                _ => unreachable!(),
            }
            variants.push(FrontendCacheNamespace::new(&changed, RuntimeModel::Bare));
        }

        assert!(variants.iter().all(|variant| *variant != baseline));
        assert_ne!(
            baseline,
            FrontendCacheNamespace::new(&target, RuntimeModel::FreestandingExecutable)
        );
        assert_eq!(
            baseline,
            FrontendCacheNamespace::from_parts(baseline.parts())
        );
    }

    #[test]
    fn frontend_module_map_fingerprint_is_order_independent_and_path_sensitive() {
        let mut first = ModuleMap::new();
        first.insert("beta", SourcePath::new("deps/beta.nia"));
        first.insert("alpha", SourcePath::new("deps/alpha.nia"));
        let mut reordered = ModuleMap::new();
        reordered.insert("alpha", SourcePath::new("deps/alpha.nia"));
        reordered.insert("beta", SourcePath::new("deps/beta.nia"));
        let mut changed_path = reordered.clone();
        changed_path.insert("beta", SourcePath::new("vendor/beta.nia"));
        let mut changed_name = ModuleMap::new();
        changed_name.insert("alpha", SourcePath::new("deps/alpha.nia"));
        changed_name.insert("gamma", SourcePath::new("deps/beta.nia"));

        let fingerprint = frontend_module_map_fingerprint(&first);
        assert_eq!(fingerprint, frontend_module_map_fingerprint(&reordered));
        assert_ne!(fingerprint, frontend_module_map_fingerprint(&changed_path));
        assert_ne!(fingerprint, frontend_module_map_fingerprint(&changed_name));
        assert_eq!(
            fingerprint,
            FrontendModuleMapFingerprint::from_parts(fingerprint.parts())
        );
        assert_eq!(std::mem::size_of::<FrontendModuleMapFingerprint>(), 16);
    }

    #[test]
    fn signature_resolution_keys_cover_program_sources_module_and_item_set() {
        let namespace = FrontendCacheNamespace::new(&TargetConfig::host(), RuntimeModel::Bare);
        let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
        let dependency =
            StableModuleKey::from_source_identity(SourceIdentity::new("src/dependency.nia"));
        let source = source_content_fingerprint("fn main() i32 { 0 }");
        let dependency_source = source_content_fingerprint("pub struct Value {}");
        let program = frontend_program_source_fingerprint([
            (&module, source, 19),
            (&dependency, dependency_source, 19),
        ]);
        let reordered = frontend_program_source_fingerprint([
            (&dependency, dependency_source, 19),
            (&module, source, 19),
        ]);
        let changed = frontend_program_source_fingerprint([
            (
                &module,
                source_content_fingerprint("fn main() i32 { 1 }"),
                19,
            ),
            (&dependency, dependency_source, 19),
        ]);
        let key = FrontendSignatureTypeResolutionCacheKey::new(
            namespace,
            &module,
            SignatureItemSet::Functions,
            program,
        );
        let lowering_key = FrontendSignatureTypeLoweringCacheKey::new(
            namespace,
            &module,
            SignatureItemSet::Functions,
            program,
        );
        let signatures_key = FrontendSignatureItemSignaturesCacheKey::new(
            namespace,
            &module,
            SignatureItemSet::Functions,
            program,
        );

        assert_eq!(program, reordered);
        assert_ne!(program, changed);
        assert_ne!(
            key,
            FrontendSignatureTypeResolutionCacheKey::new(
                namespace,
                &module,
                SignatureItemSet::Types,
                program,
            )
        );
        assert_ne!(
            key,
            FrontendSignatureTypeResolutionCacheKey::new(
                namespace,
                &dependency,
                SignatureItemSet::Functions,
                program,
            )
        );
        assert_ne!(
            key,
            FrontendSignatureTypeResolutionCacheKey::new(
                namespace,
                &module,
                SignatureItemSet::Functions,
                changed,
            )
        );
        assert_eq!(
            key,
            FrontendSignatureTypeResolutionCacheKey::from_parts(key.parts())
        );
        assert_eq!(
            lowering_key,
            FrontendSignatureTypeLoweringCacheKey::from_parts(lowering_key.parts())
        );
        assert_ne!(key.parts(), lowering_key.parts());
        assert_ne!(key.parts(), signatures_key.parts());
        assert_ne!(lowering_key.parts(), signatures_key.parts());
        assert_eq!(std::mem::size_of::<FrontendProgramSourceFingerprint>(), 16);
        assert_eq!(
            std::mem::size_of::<FrontendSignatureTypeResolutionCacheKey>(),
            16
        );
        assert_eq!(
            std::mem::size_of::<FrontendSignatureTypeLoweringCacheKey>(),
            16
        );
        assert_eq!(
            std::mem::size_of::<FrontendSignatureItemSignaturesCacheKey>(),
            16
        );
    }

    #[test]
    fn frontend_product_keys_separate_domains_modules_and_body_edits() {
        let target = TargetConfig::host();
        let namespace = FrontendCacheNamespace::new(&target, RuntimeModel::Bare);
        let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
        let other_module =
            StableModuleKey::from_source_identity(SourceIdentity::new("src/other.nia"));
        let before_source = "fn main() i32 { 1 }";
        let after_source = "fn main() i32 { 2 }";
        let before_syntax = SyntaxTree::parse(before_source, None);
        let after_syntax = SyntaxTree::parse(after_source, None);
        let before_signature = parsed_signature_fingerprint(before_source);
        let after_signature = parsed_signature_fingerprint(after_source);

        let source_key = FrontendSourceCacheKey::new(
            namespace,
            &module,
            source_content_fingerprint(before_source),
        );
        let syntax_key =
            FrontendSyntaxCacheKey::new(namespace, &module, syntax_fingerprint(&before_syntax));
        let signature_key =
            FrontendItemSignatureCacheKey::new(namespace, &module, before_signature);
        let provider_key =
            FrontendProviderSummaryCacheKey::new(namespace, &module, before_signature);
        let public_surface_facts_key = FrontendPublicSurfaceFactsCacheKey::new(
            namespace,
            &module,
            source_content_fingerprint(before_source),
        );
        let mut module_map = ModuleMap::new();
        module_map.insert("dep", SourcePath::new("deps/dep.nia"));
        let module_map = frontend_module_map_fingerprint(&module_map);
        let facade_key =
            FrontendFacadeFactsCacheKey::new(namespace, &module, before_signature, module_map);
        let dependencies_key = FrontendModuleDependenciesCacheKey::new(
            namespace,
            &module,
            source_content_fingerprint(before_source),
            module_map,
        );

        assert_ne!(
            source_key,
            FrontendSourceCacheKey::new(
                namespace,
                &module,
                source_content_fingerprint(after_source)
            )
        );
        assert_ne!(
            syntax_key,
            FrontendSyntaxCacheKey::new(namespace, &module, syntax_fingerprint(&after_syntax))
        );
        assert_eq!(before_signature, after_signature);
        assert_eq!(
            signature_key,
            FrontendItemSignatureCacheKey::new(namespace, &module, after_signature)
        );
        assert_eq!(
            provider_key,
            FrontendProviderSummaryCacheKey::new(namespace, &module, after_signature)
        );
        assert_eq!(
            facade_key,
            FrontendFacadeFactsCacheKey::new(namespace, &module, after_signature, module_map)
        );
        assert_ne!(
            facade_key,
            FrontendFacadeFactsCacheKey::new(
                namespace,
                &module,
                after_signature,
                FrontendModuleMapFingerprint::from_parts([1, 2])
            )
        );
        assert_ne!(
            dependencies_key,
            FrontendModuleDependenciesCacheKey::new(
                namespace,
                &module,
                source_content_fingerprint(after_source),
                module_map
            )
        );
        assert_ne!(
            public_surface_facts_key,
            FrontendPublicSurfaceFactsCacheKey::new(
                namespace,
                &module,
                source_content_fingerprint(after_source)
            )
        );
        assert_ne!(
            dependencies_key,
            FrontendModuleDependenciesCacheKey::new(
                namespace,
                &module,
                source_content_fingerprint(before_source),
                FrontendModuleMapFingerprint::from_parts([1, 2])
            )
        );
        assert_ne!(
            signature_key,
            FrontendItemSignatureCacheKey::new(namespace, &other_module, after_signature)
        );
        assert_ne!(source_key.parts(), syntax_key.parts());
        assert_ne!(syntax_key.parts(), signature_key.parts());
        assert_ne!(signature_key.parts(), provider_key.parts());
        assert_ne!(provider_key.parts(), facade_key.parts());
        assert_ne!(source_key.parts(), dependencies_key.parts());
        assert_ne!(source_key.parts(), public_surface_facts_key.parts());
        assert_ne!(dependencies_key.parts(), public_surface_facts_key.parts());
        assert_eq!(
            provider_key,
            FrontendProviderSummaryCacheKey::from_parts(provider_key.parts())
        );
        assert_eq!(std::mem::size_of::<FrontendCacheNamespace>(), 16);
        assert_eq!(std::mem::size_of::<FrontendSourceCacheKey>(), 16);
        assert_eq!(std::mem::size_of::<FrontendSyntaxCacheKey>(), 16);
        assert_eq!(std::mem::size_of::<FrontendItemSignatureCacheKey>(), 16);
        assert_eq!(std::mem::size_of::<FrontendProviderSummaryCacheKey>(), 16);
        assert_eq!(std::mem::size_of::<FrontendFacadeFactsCacheKey>(), 16);
        assert_eq!(
            public_surface_facts_key,
            FrontendPublicSurfaceFactsCacheKey::from_parts(public_surface_facts_key.parts())
        );
        assert_eq!(
            std::mem::size_of::<FrontendPublicSurfaceFactsCacheKey>(),
            16
        );
        assert_eq!(
            dependencies_key,
            FrontendModuleDependenciesCacheKey::from_parts(dependencies_key.parts())
        );
        assert_eq!(
            std::mem::size_of::<FrontendModuleDependenciesCacheKey>(),
            16
        );
    }

    fn signature_fingerprint(source: &str) -> ItemSignatureFingerprint {
        parsed_signature_fingerprint(source)
    }

    fn parsed_signature_fingerprint(source: &str) -> ItemSignatureFingerprint {
        let (module, errors) = nia_parser::parse_module(source);
        assert!(errors.is_empty(), "{errors:?}");
        item_signature_fingerprint(
            &SyntaxTree::parse(source, None),
            &ModuleItemTree::from_module(&module),
        )
    }
}
