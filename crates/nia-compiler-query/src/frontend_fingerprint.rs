use nia_ast::FunctionItem;
use nia_imports::StableModuleKey;
use nia_item_tree::{ItemTreeNodeKind, ModuleItemTree};
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
    use nia_source::{SourceId, SourceRevision, SourceVersion};

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
        assert_ne!(
            signature_key,
            FrontendItemSignatureCacheKey::new(namespace, &other_module, after_signature)
        );
        assert_ne!(source_key.parts(), syntax_key.parts());
        assert_ne!(syntax_key.parts(), signature_key.parts());
        assert_ne!(signature_key.parts(), provider_key.parts());
        assert_eq!(
            provider_key,
            FrontendProviderSummaryCacheKey::from_parts(provider_key.parts())
        );
        assert_eq!(std::mem::size_of::<FrontendCacheNamespace>(), 16);
        assert_eq!(std::mem::size_of::<FrontendSourceCacheKey>(), 16);
        assert_eq!(std::mem::size_of::<FrontendSyntaxCacheKey>(), 16);
        assert_eq!(std::mem::size_of::<FrontendItemSignatureCacheKey>(), 16);
        assert_eq!(std::mem::size_of::<FrontendProviderSummaryCacheKey>(), 16);
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
