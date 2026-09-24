// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

const ASM_TYPES: &str = r#"
@[builtin("AsmConfig")]
type AsmConfig;
@[builtin("AsmInputs")]
type AsmInputs;
@[builtin("AsmOutputs")]
type AsmOutputs;
"#;

fn asm_pipeline(source: &str) -> TestBodyCheck {
    pipeline(&format!("{ASM_TYPES}\n{source}"))
}

#[test]
fn checks_inline_asm_configuration() {
    let checked = asm_pipeline(
        r#"
fn main() () {
    let mut ret: i64 = 0;
    std::builtin::asm(.{
        code: b"syscall",
        outputs: .{ rax: ret },
        inputs: .{ rax: 39 },
        clobbers: [b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
}

"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad = asm_pipeline(
        r#"
fn main() () {
    std::builtin::asm(AsmConfig {
        code: 1,
        outputs: AsmOutputs { rax: 10 },
        clobbers: [1],
        options: [b"unknown"],
        extra: 0,
    });
}
"#,
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("field `code`")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("inline assembly output")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("clobbers")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unknown `asm` option")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unknown `asm` field")),
        "{:?}",
        bad.diagnostics
    );

    let bare_option = asm_pipeline(
        r#"
fn main() () {
    let mut volatile = 0;
    std::builtin::asm(AsmConfig {
        code: b"nop",
        options: [volatile],
    });
}
"#,
    );
    assert!(
        bare_option
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("string literals")),
        "{:?}",
        bare_option.diagnostics
    );

    let aggregate_operand = asm_pipeline(
        r#"
struct Pair { x: i64 }

fn main() () {
    let mut pair = Pair { x: 1 };
    std::builtin::asm(AsmConfig {
        code: b"nop",
        inputs: AsmInputs { rax: pair },
        outputs: AsmOutputs { rax: pair },
    });
}
"#,
    );
    assert!(
        aggregate_operand
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("inline assembly")
                && diagnostic.summary.contains("aggregate type"))
            .count()
            >= 2,
        "{:?}",
        aggregate_operand.diagnostics
    );
}

#[test]
fn inline_asm_does_not_shape_check_structural_recovery_operands() {
    let checked = asm_pipeline(
        r#"
fn main() () {
    std::builtin::asm(.{
        code: b"nop",
        inputs: .{ rax: (missing, 1i64) },
    });
}
"#,
    );
    let summaries = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.contains("unknown value"))
            .count(),
        1,
        "{summaries:?}"
    );
    assert!(
        summaries
            .iter()
            .all(|summary| !summary.contains("aggregate type directly")),
        "{summaries:?}"
    );
}

#[test]
fn inline_asm_preserves_unresolved_literal_field_roots() {
    let checked = asm_pipeline(
        r#"
fn main() () {
    std::builtin::asm(.{
        code: missing_code,
        clobbers: [missing_clobber],
        options: [missing_option],
    });
}
"#,
    );
    let summaries = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        summaries
            .iter()
            .filter(|summary| summary.contains("unknown value"))
            .count(),
        3,
        "{summaries:?}"
    );
    assert!(
        summaries.iter().all(|summary| {
            !summary.contains("must be a byte string literal")
                && !summary.contains("must be byte string literals")
        }),
        "{summaries:?}"
    );
}
