// SPDX-License-Identifier: GPL-3.0-or-later
//! Vector calls keep their ABI across build levels and callable forms.

mod support;

use nia_test_support::{CommandExt, test_dir};
use std::process::Command;

#[test]
fn vector_signatures_use_the_architecture_baseline_at_every_cpu_level() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("vector_abi_ir");
    let source = root.join("main.nia");
    std::fs::write(
        &source,
        r#"
@[inlineNever]
pub fun narrow(v: u8x16) u8x16 { v + v }
@[inlineNever]
pub fun wide(v: u8x32) u8x32 { v + v }
@[inlineNever]
pub fun widest(v: u8x64) u8x64 { v + v }
@[inlineNever]
pub fun narrowMask(m: boolx16) boolx16 { m & m }
// Masks are bit-packed in memory but byte-per-lane in registers.
@[inlineNever]
pub fun wideMask(m: boolx32) boolx32 { m & m }
"#,
    )
    .expect("write vector functions");
    for target in nia_target::SUPPORTED_TARGETS {
        let levels: &[&str] = if target.arch() == nia_target::Arch::X86_64 {
            &["baseline", "x86_64-v2", "x86_64-v3", "x86_64-v4"]
        } else {
            &["baseline"]
        };
        for level in levels {
            let output = support::nia_command()
                .args([
                    "--target",
                    &target.name(),
                    "emit",
                    "--llvm",
                    "-O0",
                    "--cpu",
                    level,
                ])
                .arg(&source)
                .output_timeout_for_compiler("emit vector ABI IR");
            assert!(
                output.status.success(),
                "{target} {level}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let ir = String::from_utf8(output.stdout).expect("LLVM text");
            let definitions = ir
                .lines()
                .filter(|line| line.starts_with("define "))
                .collect::<Vec<_>>();
            assert_eq!(definitions.len(), 5, "{target} {level}: {ir}");
            let is_direct = |line: &&&str| line.contains("<16 x i8>") || line.contains("<16 x i1>");
            let direct = definitions.iter().filter(is_direct).count();
            assert_eq!(
                direct,
                2 * usize::from(target.arch() != nia_target::Arch::X86),
                "{target} {level}: {ir}"
            );
            for line in &definitions {
                assert!(
                    !line.contains("<32 x i8>")
                        && !line.contains("<64 x i8>")
                        && !line.contains("<32 x i1>"),
                    "{target} {level}: {line}"
                );
                if !is_direct(&line) {
                    assert!(
                        line.contains("void @") && line.matches("ptr").count() == 2,
                        "{line}"
                    );
                }
            }
            // Changing the boundary must preserve SIMD operations in the body.
            assert!(
                ir.contains("@llvm.uadd.with.overflow.v32i8")
                    && ir.contains("@llvm.uadd.with.overflow.v64i8"),
                "{ir}"
            );
        }
    }
}

#[test]
fn wide_vectors_round_trip_through_calls_pointers_views_and_defer() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("vector_abi_runtime");
    let source = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &source,
        r#"
using std::builtin;
using std::process;

@[inlineNever]
fun twice(v: u8x32) u8x32 { v + v }
@[inlineNever]
fun throughPointer(f: &fun(u8x32) u8x32, v: u8x32) u8x32 { f(v) }
@[inlineNever]
fun throughView(f: &Fun(u8x32) u8x32, v: u8x32) u8x32 { f(v) }
@[inlineNever]
fun deferred(v: u8x32, out: &mut u8) u8x32 {
    defer { out.* = 91; };
    twice(v)
}
@[inlineNever]
@[trackCaller]
fun tracked(v: u8x32) u8x32 { twice(v) }
@[inlineNever]
fun generic[V](v: V) V { v }
@[inlineNever]
fun widest(v: u8x64) u8x64 { v + v }

pub fun main(init: process::Init) process::ExitCode!() {
    let seed = init.args().len() as u8;
    let v = builtin::insert(builtin::splat[u8x32](seed), 31, 17u8);
    let f: &fun(u8x32) u8x32 = twice;
    let capture = builtin::splat[u8x32](3u8);
    let closure = \[capture] x: u8x32 -> x + capture;
    let noCapture: &fun(u8x32) u8x32 = \x -> x + x;
    let mut out = 0u8;
    if builtin::extract(twice(v), 31) != 34
        or builtin::extract(throughPointer(f, v), 31) != 34
        or builtin::extract(throughView(f, v), 31) != 34
        or builtin::extract(throughView(closure, v), 31) != 20
        or builtin::extract(closure(v), 0) != seed + 3
        or builtin::extract(throughPointer(noCapture, v), 31) != 34
        or builtin::extract(deferred(v, &mut out), 31) != 34
        or builtin::extract(tracked(v), 31) != 34
        or builtin::extract(generic(v), 31) != 17
        or builtin::extract(widest(builtin::splat[u8x64](13u8)), 63) != 26
        or out != 91 {
        return process::ExitCode(1)!;
    }
    _ = twice(v);
    _ = throughPointer(f, v);
    _ = throughView(closure, v);
    !()
}
"#,
    )
    .expect("write runtime source");
    for target in nia_target::SUPPORTED_TARGETS {
        for optimization in ["-O0", "-O2"] {
            for lto in ["off", "thin", "full"] {
                let output = support::nia_command()
                    .args([
                        "--target",
                        &target.name(),
                        "emit",
                        "--exe",
                        optimization,
                        "--lto",
                        lto,
                    ])
                    .arg(&source)
                    .arg("-o")
                    .arg(&exe)
                    .output_timeout_for_build("build vector ABI executable");
                assert!(
                    output.status.success(),
                    "{target} {optimization} {lto}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                if Some(target) != nia_target::TargetConfig::host() {
                    continue;
                }
                let output =
                    Command::new(&exe).output_timeout_for_runtime("run vector ABI executable");
                assert_eq!(
                    output.status.code(),
                    Some(0),
                    "{target} {optimization} {lto}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}
