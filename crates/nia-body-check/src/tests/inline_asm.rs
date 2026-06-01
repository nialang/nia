// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn checks_inline_asm_configuration() {
    let checked = pipeline(
        r#"
fn main() void {
    var ret: i64 = 0;
    @asm({
        code: b"syscall",
        outputs: { rax: ret },
        inputs: { rax: 39 },
        clobbers: [b"rcx", b"r11", b"memory"],
        options: [b"volatile"],
    });
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad = pipeline(
        r#"
fn main() void {
    @asm({
        code: 1,
        outputs: { rax: 10 },
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
            .any(|diagnostic| diagnostic.message.contains("field `code`")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("inline assembly output")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("clobbers")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown `@asm` option")),
        "{:?}",
        bad.diagnostics
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown `@asm` field")),
        "{:?}",
        bad.diagnostics
    );

    let bare_option = pipeline(
        r#"
fn main() void {
    var volatile = 0;
    @asm({
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
            .any(|diagnostic| diagnostic.message.contains("string literals")),
        "{:?}",
        bare_option.diagnostics
    );

    let aggregate_operand = pipeline(
        r#"
struct Pair { x: i64 }

fn main() void {
    var pair: Pair = { x: 1 };
    @asm({
        code: b"nop",
        inputs: { rax: pair },
        outputs: { rax: pair },
    });
}
"#,
    );
    assert!(
        aggregate_operand
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("inline assembly")
                && diagnostic.message.contains("aggregate type"))
            .count()
            >= 2,
        "{:?}",
        aggregate_operand.diagnostics
    );
}
