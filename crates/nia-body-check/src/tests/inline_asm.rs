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
    std::builtin::asm(AsmConfig {
        code: b"syscall",
        outputs: AsmOutputs { rax: ret },
        inputs: AsmInputs { rax: 39 },
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
