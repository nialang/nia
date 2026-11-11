// SPDX-License-Identifier: GPL-3.0-or-later
//! CPU detection must execute at baseline, including on AVX-capable hosts.

mod support;

use nia_test_support::{CommandExt, test_dir};
#[cfg(target_arch = "x86_64")]
use std::process::Command;

#[cfg(target_arch = "x86_64")]
#[test]
fn baseline_cpu_queries_match_the_host_and_cached_answers() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    use nia_target::CpuFeature::*;
    let features = [
        (Sse2, std::is_x86_feature_detected!("sse2")),
        (Sse3, std::is_x86_feature_detected!("sse3")),
        (Ssse3, std::is_x86_feature_detected!("ssse3")),
        (Sse4_1, std::is_x86_feature_detected!("sse4.1")),
        (Sse4_2, std::is_x86_feature_detected!("sse4.2")),
        (Popcnt, std::is_x86_feature_detected!("popcnt")),
        (Cmpxchg16b, std::is_x86_feature_detected!("cmpxchg16b")),
        (Avx, std::is_x86_feature_detected!("avx")),
        (Avx2, std::is_x86_feature_detected!("avx2")),
        (Fma, std::is_x86_feature_detected!("fma")),
        (Bmi1, std::is_x86_feature_detected!("bmi1")),
        (Bmi2, std::is_x86_feature_detected!("bmi2")),
        (Lzcnt, std::is_x86_feature_detected!("lzcnt")),
        (Movbe, std::is_x86_feature_detected!("movbe")),
        (F16c, std::is_x86_feature_detected!("f16c")),
        (Avx512f, std::is_x86_feature_detected!("avx512f")),
        (Avx512bw, std::is_x86_feature_detected!("avx512bw")),
        (Avx512cd, std::is_x86_feature_detected!("avx512cd")),
        (Avx512dq, std::is_x86_feature_detected!("avx512dq")),
        (Avx512vl, std::is_x86_feature_detected!("avx512vl")),
        (Neon, false),
    ];
    assert_eq!(
        features.map(|(feature, _)| feature),
        nia_target::CPU_FEATURES
    );
    let root = test_dir("cpu_queries");
    let source = root.join("main.nia");
    let mut text = String::from(
        "using std::{cpu, process};\npub fun main(init: process::Init) process::ExitCode!() {\n_ = init;\nfor repeat in 0..3 {\n_ = repeat;\n",
    );
    for (index, (feature, expected)) in features.into_iter().enumerate() {
        text.push_str(&format!(
            "if cpu::supports(.{feature:?}) != {expected} {{ return process::ExitCode({})!; }}\n",
            index + 1
        ));
    }
    text.push_str("}\n!()\n}\n");
    std::fs::write(&source, text).expect("write feature assertions");
    run_at_both_optimizations(&root, &source);
}

#[cfg(target_arch = "x86_64")]
fn run_at_both_optimizations(root: &std::path::Path, source: &std::path::Path) {
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    for optimization in ["-O0", "-O2"] {
        let output = support::nia_command()
            .args(["emit", "--exe", "--cpu", "baseline", optimization])
            .arg(source)
            .arg("-o")
            .arg(&exe)
            .output_timeout_for_build("build CPU detection executable");
        assert!(
            output.status.success(),
            "{optimization}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = Command::new(&exe).output_timeout_for_runtime("run CPU detection executable");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{optimization}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(target_arch = "x86_64")]
#[test]
fn x86_detection_requires_os_vector_state() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("cpu_os_state");
    let source = root.join("main.nia");
    // Test the private decoder with synthetic CPUID/XCR0 snapshots. Compile
    // the actual implementation as a local module, without a public test API.
    let implementation = include_str!("../../../lib/std/cpu/x86.nia").replace("root::", "std::");
    std::fs::write(&source, format!("{implementation}\n{}", r#"
using std::process;
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let all = 0xffffffffu32;
    let avxFeatures = bit(.Avx) | bit(.Avx2) | bit(.Fma) | bit(.F16c);
    let zmmFeatures = bit(.Avx512f) | bit(.Avx512bw) | bit(.Avx512cd) | bit(.Avx512dq) | bit(.Avx512vl);
    let base = 7u32 << 26;
    let basicCases = [
        (0u32, CpuFeature::Sse3), (9u32, CpuFeature::Ssse3),
        (19u32, CpuFeature::Sse4_1), (20u32, CpuFeature::Sse4_2),
        (23u32, CpuFeature::Popcnt), (13u32, CpuFeature::Cmpxchg16b),
        (22u32, CpuFeature::Movbe),
    ];
    for &(index, feature) in &basicCases[..] {
        if decode(1u32 << index, 0, 0, 0, 0) != bit(feature) { return process::ExitCode(10)!; }
    }
    if decode(0, 1u32 << 26, 0, 0, 0) != bit(.Sse2) { return process::ExitCode(11)!; }
    if decode(0, 0, 1u32 << 3, 0, 0) != bit(.Bmi1) { return process::ExitCode(12)!; }
    if decode(0, 0, 1u32 << 8, 0, 0) != bit(.Bmi2) { return process::ExitCode(13)!; }
    if decode(0, 0, 0, 1u32 << 5, 0) != bit(.Lzcnt) { return process::ExitCode(14)!; }
    if decode(base, 0, 0, 0, 6) != bit(.Avx) { return process::ExitCode(15)!; }
    if decode(base, 0, 1u32 << 5, 0, 6) != (bit(.Avx) | bit(.Avx2)) { return process::ExitCode(16)!; }
    if decode(base | (1u32 << 12), 0, 0, 0, 6) != (bit(.Avx) | bit(.Fma)) { return process::ExitCode(17)!; }
    if decode(base | (1u32 << 29), 0, 0, 0, 6) != (bit(.Avx) | bit(.F16c)) { return process::ExitCode(18)!; }
    let wideCases = [
        (30u32, CpuFeature::Avx512bw), (28u32, CpuFeature::Avx512cd),
        (17u32, CpuFeature::Avx512dq), (31u32, CpuFeature::Avx512vl),
    ];
    for &(index, feature) in &wideCases[..] {
        if decode(base, 0, (1u32 << 16) | (1u32 << index), 0, 0xe6) != (bit(.Avx) | bit(.Avx512f) | bit(feature)) {
            return process::ExitCode(19)!;
        }
    }
    if decode(0, 0, 0, 0, 0) != 0 { return process::ExitCode(1)!; }
    // OSXSAVE, XSAVE and AVX must each be present, even with every XCR0 bit.
    for missing in 26..29 {
        if (decode(all ^ (1u32 << missing), all, all, all, all) & (avxFeatures | zmmFeatures)) != 0 {
            return process::ExitCode(2)!;
        }
    }
    for state in 0..6 {
        if (decode(all, all, all, all, state as u32) & (avxFeatures | zmmFeatures)) != 0 {
            return process::ExitCode(3)!;
        }
    }
    if (decode(all, all, all, all, 6) & avxFeatures) != avxFeatures { return process::ExitCode(4)!; }
    @[if os != "macos"]
    if (decode(all, all, all, all, 6) & zmmFeatures) != 0 { return process::ExitCode(5)!; }
    for missing in 5..8 {
        @[if os != "macos"]
        if (decode(all, all, all, all, all ^ (1u32 << missing)) & zmmFeatures) != 0 { return process::ExitCode(6)!; }
    }
    if (decode(all, all, all, all, 0xe6) & zmmFeatures) != zmmFeatures { return process::ExitCode(7)!; }
    // Missing extended/leaf-7 data must not produce their features.
    if (decode(all, all, 0, 0, all) & (bit(.Lzcnt) | bit(.Bmi1) | bit(.Bmi2) | bit(.Avx2) | zmmFeatures)) != 0 {
        return process::ExitCode(8)!;
    }
    if (decode(all, all, all, all, 0) & bit(.Neon)) != 0 { return process::ExitCode(9)!; }
    !()
}
"#)).expect("write synthetic detector");
    run_at_both_optimizations(&root, &source);
}

#[test]
fn runtime_cpu_query_compiles_for_every_maintained_target() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = test_dir("cpu_cross_targets");
    let source = root.join("main.nia");
    std::fs::write(&source, "pub fun query(feature: std::builtin::target::CpuFeature) bool { std::cpu::supports(feature) }\n").expect("write query");
    for target in nia_target::SUPPORTED_TARGETS {
        let output = support::nia_command()
            .args([
                "--target",
                &target.name(),
                "emit",
                "--obj",
                "--cpu",
                "baseline",
            ])
            .arg(&source)
            .arg("--out-dir")
            .arg(root.join(target.name()))
            .output_timeout_for_compiler("compile runtime CPU query");
        assert!(
            output.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
