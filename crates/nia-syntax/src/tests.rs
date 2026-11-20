// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{AlternateKind, GreenElement, GreenNode, SyntaxKind, parse};

#[test]
fn delimiter_limit_matches_across_edits_and_recovers() {
    let expression = |levels| format!("{}1{}", "(".repeat(levels), ")".repeat(levels));
    let source = |expr| format!("fun sample() i32 {{ {expr} }}");
    // The function body contributes one delimiter.
    let at_limit = source(expression(crate::MAX_DELIMITER_DEPTH - 1));
    let valid = parse(&at_limit, None).unwrap();
    assert!(valid.errors.is_empty());

    let over_limit = source(expression(crate::MAX_DELIMITER_DEPTH));
    let invalid = parse(&over_limit, None).unwrap();
    assert_eq!(invalid.errors.len(), 1);
    let error = &invalid.errors[0];
    assert_eq!(error.span.end, over_limit.find('1').unwrap());
    assert!(error.message.contains("nesting limit"));
    assert_eq!(invalid.tree.full_text(), over_limit);
    assert_eq!(
        valid
            .reparse(&crate::TextEdit::between(&at_limit, &over_limit), None)
            .unwrap(),
        invalid,
    );
    assert_eq!(
        invalid
            .reparse(&crate::TextEdit::between(&over_limit, &at_limit), None)
            .unwrap(),
        valid,
    );
}

fn kinds(node: &GreenNode, output: &mut Vec<SyntaxKind>) {
    output.push(node.kind().clone());
    for child in node.child_nodes() {
        kinds(child, output);
    }
}

fn assert_source_coverage(node: &GreenNode, source: &str) {
    for child in node.children() {
        let span = match child {
            GreenElement::Node(child) => {
                assert_source_coverage(child, source);
                child.span()
            }
            GreenElement::Token(token) => {
                assert_eq!(
                    source.get(token.span().start..token.span().end),
                    Some(token.text())
                );
                token.span()
            }
        };
        assert!(node.span().start <= span.start && span.end <= node.span().end);
    }
}

#[test]
fn speculative_type_snapshots_keep_nested_const_alternates() {
    for argument in [
        "Buffer[COUNT]",
        "Wrap[Buffer[COUNT]]",
        "(Buffer[COUNT], &Wrap[Buffer[OTHER]])",
    ] {
        let source = format!("fun inspect() () {{ consume[{argument}](); }}");
        let parsed = parse(&source, None).expect("grammar");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        for name in ["COUNT", "OTHER"] {
            for (start, _) in source.match_indices(name) {
                let span = nia_span::Span::new(start, start + name.len());
                let alternate = parsed
                    .alternate(AlternateKind::ConstArg, span)
                    .expect("nested const interpretation must survive the outer rewind");
                assert_source_coverage(alternate, &source);
            }
        }
    }
}

#[test]
fn complete_grammar_retains_nested_productions_and_semantic_alternates() {
    let source = "type Stored = Box[N];\nfun f[T](x: T) T { let y = call[T](x); match y { ?v => v, null => x, } }";
    let parsed = parse(source, None).expect("valid grammar tree");
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let mut actual = Vec::new();
    kinds(parsed.tree.green_root(), &mut actual);
    for expected in [
        SyntaxKind::TypeAliasDecl,
        SyntaxKind::TypeOrConstArg,
        SyntaxKind::FunctionDecl,
        SyntaxKind::GenericParamList,
        SyntaxKind::Param,
        SyntaxKind::Block,
        SyntaxKind::LetStmt,
        SyntaxKind::CallExpr,
        SyntaxKind::BracketArg,
        SyntaxKind::MatchExpr,
        SyntaxKind::OptionalPattern,
        SyntaxKind::NullPattern,
    ] {
        assert!(actual.contains(&expected), "missing {expected:?}");
    }
    assert!(
        parsed
            .alternates
            .iter()
            .any(|alternate| alternate.kind == AlternateKind::ConstArg)
    );
    assert!(
        parsed
            .alternates
            .iter()
            .any(|alternate| alternate.kind == AlternateKind::BracketType)
    );
    assert_eq!(parsed.tree.full_text(), source);
    assert_source_coverage(parsed.tree.green_root(), source);
}

#[test]
fn recovery_regions_are_lossless_and_preserve_later_declarations() {
    for source in [
        "module module retained;",
        "fun broken(value) {}\nmodule retained;",
        "fun broken[T(value: T) T;\nmodule retained;",
        "struct Broken { value: , next: i32 }\nmodule retained;",
        "fun broken() { call(, value); later(); }\nmodule retained;",
        "fun broken() { let = value; later(); }\nmodule retained;",
        "@[if ] fun broken() {}\nmodule retained;",
    ] {
        let parsed = parse(source, None).expect("recovered grammar tree");
        assert!(!parsed.errors.is_empty(), "{source}");
        assert_eq!(parsed.tree.full_text(), source);
        assert_source_coverage(parsed.tree.green_root(), source);
        let mut actual = Vec::new();
        kinds(parsed.tree.green_root(), &mut actual);
        assert!(
            actual.contains(&SyntaxKind::Error) || actual.contains(&SyntaxKind::Missing),
            "{source}"
        );
        let last = parsed
            .tree
            .green_root()
            .child_nodes()
            .last()
            .expect("retained item");
        assert!(last.child(&SyntaxKind::ModuleDecl).is_some(), "{source}");
    }
}
