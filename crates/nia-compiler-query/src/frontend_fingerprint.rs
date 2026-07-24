use nia_ast::FunctionItem;
use nia_item_tree::{ItemTreeNodeKind, ModuleItemTree};
use nia_query::{QueryFingerprint, QueryFingerprintBuilder};
use nia_span::Span;
use nia_syntax::SyntaxTree;

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

    fn signature_fingerprint(source: &str) -> ItemSignatureFingerprint {
        let (module, errors) = nia_parser::parse_module(source);
        assert!(errors.is_empty(), "{errors:?}");
        item_signature_fingerprint(
            &SyntaxTree::parse(source, None),
            &ModuleItemTree::from_module(&module),
        )
    }
}
