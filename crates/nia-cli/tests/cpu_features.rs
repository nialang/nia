// SPDX-License-Identifier: GPL-3.0-or-later
//! Function-local instruction sets, their ABI boundaries, and checked entries.
mod support;

use nia_test_support::{CommandExt, test_dir};
use std::process::Command;

#[test]
fn function_feature_diagnostics_precede_codegen() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("cpu_feature_diagnostics");
    let source = root.join("main.nia");
    let check = |text: &str, cpu: &str| {
        std::fs::write(&source, text).expect("write source");
        support::nia_command()
            .args(["--target", "x86_64-unknown-linux", "check", "--cpu", cpu])
            .arg(&source)
            .output_timeout_for_compiler("check CPU function features")
    };
    for (text, message) in [
        (
            "@[cpuFeatures()] pub fun f() () {}",
            "at least one string literal",
        ),
        (
            "@[cpuFeatures(42)] pub fun f() () {}",
            "expects string literal",
        ),
        (
            "@[cpuFeatures(\"avx3\")] pub fun f() () {}",
            "unknown CPU feature `avx3`",
        ),
        (
            "@[cpuFeatures(\"neon\")] pub fun f() () {}",
            "not available on `x86_64-unknown-linux`",
        ),
        (
            "@[cpuFeatures(\"avx2\")] extern fun f() ();",
            "requires a function body",
        ),
        (
            "@[naked] @[cpuFeatures(\"avx2\")] extern fun f() () { std::builtin::asm(.{ code: b\"ret\" }); }",
            "cannot be combined with `@[naked]`",
        ),
        (
            "@[inlineAlways] @[cpuFeatures(\"avx2\")] fun fast(x: i32) i32 { x } pub fun f() i32 { fast(7) }",
            "requires CPU features the caller lacks: `avx2`",
        ),
    ] {
        let output = check(text, "baseline");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && stderr.contains(message),
            "{text}: {stderr}"
        );
        assert!(!stderr.contains("internal compiler error"), "{stderr}");
    }
    let text = "@[inlineAlways] @[cpuFeatures(\"avx\")] fun fast(x: i32) i32 { x } @[cpuFeatures(\"avx2\")] pub fun f() i32 { fast(7) }";
    let output = check(text, "baseline");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = "@[inlineAlways] @[cpuFeatures(\"avx2\")] fun fast(x: i32) i32 { x } pub fun f() i32 { fast(7) }";
    let output = check(text, "x86_64-v3");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // A trait call inlines the implementation it selects, not the trait's
    // default: overriding a featured default with a plain body is valid.
    let text = r#"
trait D { @[inlineAlways] @[cpuFeatures("avx2")] fun dd(self) i32 { _ = self; 3 } }
struct Other {}
extend Other : D { fun dd(self) i32 { _ = self; 1 } }
pub fun viaOverride() i32 { Other {}.dd() }
fun generic[T](value: T) i32 where T: D { value.dd() }
pub fun viaGenericOverride() i32 { generic(Other {}) }
"#;
    let output = check(text, "baseline");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The architecture error names the written feature, not its implications.
    std::fs::write(&source, "@[cpuFeatures(\"avx2\")] pub fun f() () {}").expect("write source");
    let output = support::nia_command()
        .args(["--target", "aarch64-unknown-linux", "check"])
        .arg(&source)
        .output_timeout_for_compiler("check foreign CPU feature");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success()
            && stderr.contains("CPU feature `avx2` is not available")
            && stderr.matches("is not available").count() == 1,
        "{stderr}"
    );
}

#[test]
fn feature_boundaries_survive_inlining_and_profile_changes() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("cpu_feature_ir");
    let source = root.join("main.nia");
    std::fs::write(
        &source,
        r#"
@[cpuFeatures("avx2")]
fun accelerated(v: i32) i32 { v }
pub extern fun dispatch(v: i32) i32 { accelerated(v) }
@[cpuFeatures("avx2")]
pub fun closure(v: u8x32) u8x32 { let f = \x: u8x32 -> x + x; f(v) }
@[cpuFeatures("avx2")]
pub fun buildCondition() i32 {
    @[if hasFeature("avx2")]
    return 1;
    @[if not hasFeature("avx2")]
    return 2;
}

"#,
    )
    .expect("write source");
    for profile in ["--debug", "--release"] {
        let output = support::nia_command()
            .args([
                "--target",
                "x86_64-unknown-linux",
                profile,
                "emit",
                "--llvm",
                "-O2",
            ])
            .arg(&source)
            .output_timeout_for_compiler("emit function feature IR");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let ir = String::from_utf8(output.stdout).expect("LLVM text");
        assert!(
            ir.contains("+avx2") && ir.contains("+avx") && ir.contains("+sse4.2"),
            "{ir}"
        );
        assert!(
            ir.contains("ret i32 2"),
            "function features changed build-time conditions: {ir}"
        );
        assert_eq!(
            ir.contains("call void @__nia_check_cpu_features"),
            profile == "--debug",
            "{ir}"
        );
        // The identity function is a leaf-inline candidate. Its feature
        // boundary must still be present in the baseline dispatcher at O2.
        let dispatch = ir
            .split("@dispatch(")
            .nth(1)
            .expect("dispatcher")
            .split("\n}")
            .next()
            .expect("dispatcher body");
        assert!(dispatch.contains("call i32"), "{ir}");
    }
}

#[test]
fn checked_features_round_trip_through_all_callable_forms() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("cpu_feature_runtime");
    let source = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &source,
        r#"
using std::{builtin, cpu, process};
@[if arch == "x86" or arch == "x86_64"]
@[cpuFeatures("avx2")]
@[inlineNever]
fun accelerated(v: u8x32) u8x32 { v + v }
@[if arch == "x86" or arch == "x86_64"]
@[cpuFeatures("avx2")]
@[trackCaller]
fun tracked(v: u8x32) u8x32 { v + v }
@[if arch == "x86" or arch == "x86_64"]
module specialized;
@[if arch == "x86" or arch == "x86_64"]
@[cpuFeatures("avx2")]
fun generic[V](v: V) V { let f = \x: V -> x; f(v) }
@[if arch == "x86" or arch == "x86_64"]
@[cpuFeatures("avx2")]
fun makeClosure() &fun(u8x32) u8x32 { \v: u8x32 -> specialized::helper(v) }
@[if arch == "x86" or arch == "x86_64"]
@[cpuFeatures("avx2")]
fun compatible(v: u8x32) u8x32 { specialized::helper(v) }
@[if arch == "x86" or arch == "x86_64"]
@[cpuFeatures("avx2")]
fun captured(v: u8x32) u8x32 {
    let closure = \[v] x: u8x32 -> x + v;
    view(closure, v)
}
extern struct Packet { values: [u64; 4] }
@[if arch == "x86" or arch == "x86_64"]
@[cpuFeatures("avx2")]
extern fun cPacket(value: Packet) Packet {
    let mut result = value;
    result.values[3] += 1;
    result
}
@[if arch == "x86" or arch == "x86_64"]
@[cpuFeatures("avx2")]
extern fun cByte(value: i8) i8 { value }
@[inlineNever]
fun indirect(f: &fun(u8x32) u8x32, v: u8x32) u8x32 { f(v) }
@[inlineNever]
fun view(f: &Fun(u8x32) u8x32, v: u8x32) u8x32 { f(v) }
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    @[if arch == "x86" or arch == "x86_64"]
    if cpu::supports(.Avx2) {
        let v = builtin::splat[u8x32](17u8);
        let f: &fun(u8x32) u8x32 = accelerated;
        let h: &fun(u8x32) u8x32 = specialized::helper;
        if builtin::extract(accelerated(v), 31) != 34
            or cPacket(Packet { values: [1, 2, 3, 4] }).values[3] != 5
            or cByte(-7) != -7
            or builtin::extract(indirect(f, v), 31) != 34
            or builtin::extract(view(f, v), 31) != 34
            or builtin::extract(indirect(h, v), 31) != 34
            or builtin::extract(generic(v), 31) != 17
            or builtin::extract(indirect(makeClosure(), v), 31) != 34
            or builtin::extract(compatible(v), 31) != 34
            or builtin::extract(tracked(v), 31) != 34
            or builtin::extract(captured(v), 31) != 34 {
            return process::ExitCode(1)!;
        }
    }
    !()
}
"#,
    )
    .expect("write source");
    std::fs::write(
        root.join("specialized.nia"),
        "@[cpuFeatures(\"avx2\")] @[inlineAlways] pub fun helper(v: u8x32) u8x32 { v + v }",
    )
    .expect("write specialized module");
    for target in nia_target::SUPPORTED_TARGETS {
        for profile in ["--debug", "--release"] {
            for optimization in ["-O0", "-O2"] {
                for lto in ["off", "thin", "full"] {
                    let output = support::nia_command()
                        .args([
                            "--target",
                            &target.name(),
                            profile,
                            "emit",
                            "--exe",
                            optimization,
                            "--lto",
                            lto,
                        ])
                        .arg(&source)
                        .arg("-o")
                        .arg(&exe)
                        .output_timeout_for_build("build CPU feature callable paths");
                    assert!(
                        output.status.success(),
                        "{target} {profile} {lto}: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    if Some(target) == nia_target::TargetConfig::host() {
                        let output = Command::new(&exe)
                            .output_timeout_for_runtime("run CPU feature callable paths");
                        assert_eq!(
                            output.status.code(),
                            Some(0),
                            "{profile} {lto}: {}",
                            String::from_utf8_lossy(&output.stderr)
                        );
                    }
                }
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[test]
fn generic_trait_resolution_checks_always_inline_features_at_every_level() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Compiler);
    let root = test_dir("cpu_feature_generic_inline");
    let source = root.join("main.nia");
    std::fs::write(
        &source,
        r#"
trait Step { fun step(self) i32; }
struct Value {}
extend Value : Step {
    @[cpuFeatures("avx2")]
    @[inlineAlways]
    fun step(self) i32 { _ = self; 7 }
}
fun generic[T](value: T) i32 where T: Step { value.step() }
pub fun f() i32 { generic(Value {}) }
"#,
    )
    .expect("write generic trait call");
    for optimization in ["-O0", "-O2"] {
        let output = support::nia_command()
            .args([
                "--target",
                "x86_64-unknown-linux",
                "emit",
                "--llvm",
                optimization,
            ])
            .arg(&source)
            .output_timeout_for_compiler("check resolved trait CPU features");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success()
                && stderr.contains("requires CPU features the caller lacks: `avx2`"),
            "{optimization}: {stderr}"
        );
        assert!(!stderr.contains("internal compiler error"), "{stderr}");
    }
}

#[test]
fn runtime_feature_failure_names_the_missing_instruction_set() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("cpu_feature_failure");
    let source = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    // Ask the actual runtime support ABI for a feature of another ISA. This
    // deterministically tests failure even on a host with every x86 feature.
    let missing = 1 << 20;
    std::fs::write(&source, format!("using std::process; extern fun __nia_check_cpu_features(required: u32) (); @[cpuFeatures(\"avx2\")] pub fun main(init: process::Init) process::ExitCode!() {{ _ = init; __nia_check_cpu_features({missing}u32); !() }}")).expect("write source");
    let output = support::nia_command()
        .args(["emit", "--exe"])
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("build missing CPU feature diagnostic");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output =
        Command::new(&exe).output_timeout_for_runtime("run missing CPU feature diagnostic");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required CPU feature") && stderr.contains("is unavailable"),
        "{stderr}"
    );
}

#[test]
fn feature_edits_and_profiles_match_clean_objects() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Compiler);
    let root = test_dir("cpu_feature_cache");
    let source = root.join("main.nia");
    let warm = root.join("warm");
    let emit = |cache: &std::path::Path, profile: &str| {
        let out = root.join("objects");
        let output = support::nia_command()
            .args([
                "--target",
                "x86_64-unknown-linux",
                profile,
                "emit",
                "--obj",
                "-O2",
                "--cache-dir",
            ])
            .arg(cache)
            .arg(&source)
            .arg("--out-dir")
            .arg(&out)
            .output_timeout_for_compiler("emit cached function features");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let path = std::fs::read_dir(&out)
            .expect("list objects")
            .map(|entry| entry.expect("object entry").path())
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("0000_"))
            })
            .expect("source object");
        std::fs::read(path).expect("read object")
    };
    let mut previous = None;
    for (index, feature) in ["avx2", "bmi2", "avx2"].into_iter().enumerate() {
        std::fs::write(
            &source,
            format!(
                "@[cpuFeatures(\"{feature}\")] pub extern fun f(value: u64) u64 {{ value + 1 }}"
            ),
        )
        .expect("edit feature");
        for profile in ["--debug", "--release", "--debug"] {
            let object = emit(&warm, profile);
            let clean = emit(&root.join(format!("clean{index}{profile}")), profile);
            assert_eq!(
                object, clean,
                "{feature} {profile}: incremental differs from clean"
            );
            if profile == "--debug" {
                if let Some((old_feature, old_object)) = &previous
                    && *old_feature != feature
                {
                    assert_ne!(&object, old_object, "CPU feature edit reused old code");
                }
                previous = Some((feature, object));
            }
        }
    }
}

#[test]
fn unused_features_do_not_root_debug_runtime_support() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Compiler);
    let root = test_dir("cpu_feature_runtime_roots");
    let source = root.join("main.nia");
    std::fs::write(
        &source,
        r#"
using std::process;
@[if arch == "x86_64"]
@[cpuFeatures("avx2")]
fun unused() i32 { 7 }
pub fun main(init: process::Init) process::ExitCode!() { _ = init; !() }
"#,
    )
    .expect("write source");
    let output = support::nia_command()
        .args(["emit", "--llvm", "--runtime", "freestanding"])
        .arg(&source)
        .output_timeout_for_compiler("inspect unused CPU runtime roots");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("__nia_check_cpu_features"));
}

#[test]
fn malformed_cpu_runtime_support_is_a_user_diagnostic() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Compiler);
    let root = test_dir("cpu_feature_support_signature");
    let source = root.join("main.nia");
    std::fs::write(
        &source,
        r#"
extern fun __nia_check_cpu_features(required: u64) ();
@[cpuFeatures("avx2")]
pub fun f() () { __nia_check_cpu_features(0u64); }
"#,
    )
    .expect("write malformed native support declaration");
    let output = support::nia_command()
        .args(["--target", "x86_64-unknown-linux", "emit", "--llvm"])
        .arg(&source)
        .output_timeout_for_compiler("diagnose malformed CPU support ABI");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("must have C signature"),
        "{stderr}"
    );
    assert!(!stderr.contains("internal compiler error"), "{stderr}");
}
