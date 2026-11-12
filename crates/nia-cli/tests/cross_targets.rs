// SPDX-License-Identifier: GPL-3.0-or-later
//! Every maintained target compiles from every maintained host.

mod support;

use nia_test_support::{CommandExt, test_dir as temp_dir};

/// Object-file identity checked from raw bytes, so no platform tool is needed.
fn object_format(bytes: &[u8]) -> Option<(&'static str, u16)> {
    match bytes {
        // ELF: `e_machine` at offset 18, little-endian.
        [0x7f, b'E', b'L', b'F', ..] if bytes.len() > 19 => {
            Some(("elf", u16::from_le_bytes([bytes[18], bytes[19]])))
        }
        // 64-bit little-endian Mach-O: `cputype` follows the magic.
        [0xcf, 0xfa, 0xed, 0xfe, cpu0, cpu1, ..] => {
            Some(("mach-o", u16::from_le_bytes([*cpu0, *cpu1])))
        }
        // COFF object: `Machine` is the first field.
        [machine0, machine1, ..] => Some(("coff", u16::from_le_bytes([*machine0, *machine1]))),
        _ => None,
    }
}

#[test]
fn every_maintained_target_emits_its_native_object_format() {
    let root = temp_dir("cross_target_objects");
    let source = root.join("main.nia");
    std::fs::write(
        &source,
        "fun add(a: i32, b: i32) i32 { a + b }\npub fun main() () { _ = add(1, 2); }\n",
    )
    .expect("write source");
    // Machine codes: ELF x86-64 62, i386 3, AArch64 183; Mach-O x86_64 7 and
    // arm64 12 (low half of the CPU type); COFF AMD64 0x8664.
    for (target, expected) in [
        ("x86_64-unknown-linux", ("elf", 62)),
        ("x86-unknown-linux", ("elf", 3)),
        ("aarch64-unknown-linux", ("elf", 183)),
        ("x86_64-pc-windows-msvc", ("coff", 0x8664)),
        ("x86_64-apple-macos", ("mach-o", 7)),
        ("aarch64-apple-macos", ("mach-o", 12)),
    ] {
        let objects = root.join(target);
        let output = support::nia_command()
            .args(["--target", target, "emit", "--obj"])
            .arg(&source)
            .arg("--out-dir")
            .arg(&objects)
            .output_timeout_for_compiler("emit cross-target object");
        assert!(
            output.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let objects = nia_test_support::object_files(&objects);
        assert!(!objects.is_empty(), "{target}: no object was emitted");
        for object in objects {
            let bytes = std::fs::read(&object).expect("read object");
            assert_eq!(
                object_format(&bytes),
                Some(expected),
                "{target}: {}",
                object.display()
            );
        }
    }
}

#[test]
fn unsupported_target_names_are_diagnosed_with_the_matrix() {
    let output = support::nia_command()
        .args(["--target", "x86_64-unknown-linux-gnu", "check", "main.nia"])
        .output_timeout_for_compiler("reject unsupported target");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("unsupported target `x86_64-unknown-linux-gnu`"),
        "{stderr}"
    );
    assert!(stderr.contains("`aarch64-apple-macos`"), "{stderr}");
}

#[test]
fn cpu_levels_are_checked_against_the_artifact_target() {
    let reject = |args: &[&str], expected: &str| {
        let output = support::nia_command()
            .args(args)
            .output_timeout_for_compiler("reject CPU level");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    };
    reject(
        &["emit", "--llvm", "--cpu", "x86-64-v3", "main.nia"],
        "unknown CPU level `x86-64-v3`; CPU levels are `baseline`, `x86_64-v2`",
    );
    reject(
        &[
            "--target",
            "aarch64-unknown-linux",
            "emit",
            "--obj",
            "--cpu",
            "x86_64-v3",
            "main.nia",
        ],
        "CPU level `x86_64-v3` is for x86_64 targets, not `aarch64-unknown-linux`",
    );
    // `native` describes the machine running the compiler, so it needs the
    // host to be the target. Pick a maintained target that is not the host.
    let foreign = nia_target::SUPPORTED_TARGETS
        .into_iter()
        .find(|target| Some(*target) != nia_target::TargetConfig::host())
        .expect("the matrix has more than one target")
        .name();
    reject(
        &[
            "--target", &foreign, "emit", "--exe", "--cpu", "native", "main.nia",
        ],
        "CPU level `native` describes this machine",
    );
    // Printing tokens or syntax compiles nothing, so no level applies.
    reject(
        &["emit", "--ast", "--cpu", "x86_64-v2", "main.nia"],
        "`--cpu` is not an option of `nia emit --ast`",
    );
}

#[test]
fn check_prunes_by_cpu_level_and_keeps_levels_apart_in_the_cache() {
    let root = temp_dir("check_prunes_by_cpu_level");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        "@[if hasFeature(\"ssse3\")]\nfun needsSsse3() i32 {\n    1\n}\n\npub fun value() i32 {\n    needsSsse3()\n}\n",
    )
    .expect("write source");
    let cache = root.join("cache");
    let check = |level: &str| {
        support::nia_command()
            .args([
                "--target",
                "x86_64-unknown-linux",
                "check",
                "--cpu",
                level,
                "--cache-dir",
            ])
            .arg(&cache)
            .arg(&main)
            .output_timeout_for_compiler("check at a CPU level")
    };
    // Without SSSE3 the function is pruned, so the call names nothing. The
    // shared cache must not hand the x86_64-v2 result back to the baseline.
    for (level, passes) in [
        ("baseline", false),
        ("x86_64-v2", true),
        ("baseline", false),
    ] {
        let output = check(level);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.success(), passes, "{level}: {stderr}");
        if !passes {
            assert!(
                stderr.contains("unknown value `needsSsse3`"),
                "{level}: {stderr}"
            );
        }
    }

    std::fs::write(&main, "@[if hasFeature(\"avx3\")]\nfun f() () {}\n").expect("write source");
    let output = check("baseline");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("unknown CPU feature `avx3`; CPU features are `sse2`, `sse3`"),
        "{stderr}"
    );
}

/// A program asserting, at compile time, what `std::builtin::target::hasFeature`
/// reports for every feature; `flip` inverts the expectation of one feature.
fn feature_assertions(
    expected: nia_target::CpuFeatures,
    flip: Option<nia_target::CpuFeature>,
) -> String {
    // Keep this const-only, use the re-exported builtin, and branch directly
    // on the cross-module call: each used to need a separate workaround.
    let mut source = String::from("using std::builtin::{target, error};\n");
    for (index, feature) in nia_target::CPU_FEATURES.into_iter().enumerate() {
        let want = expected.contains(feature) != (flip == Some(feature));
        source.push_str(&format!(
            "const CHECK_{index}: usize = if target::hasFeature(.{feature:?}) == {want} {{ 1 }} else {{ error(\"{feature} should be {want}\") }};\n"
        ));
    }
    source
}

#[test]
fn target_feature_query_matches_the_cpu_level_on_every_target() {
    // The std enum and the compiler's table must name the same features.
    let std_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lib/std/builtin/target.nia"),
    )
    .expect("read std target module");
    let start = std_source
        .find("pub enum CpuFeature {")
        .expect("CpuFeature enum");
    let body = &std_source[start
        ..std_source[start..]
            .find('}')
            .map(|end| start + end)
            .expect("enum end")];
    let declared = body
        .lines()
        .skip(1)
        .map(|line| line.trim().trim_end_matches(','))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let table = nia_target::CPU_FEATURES.map(|feature| format!("{feature:?}"));
    assert_eq!(
        declared, table,
        "std::builtin::target::CpuFeature must list CPU_FEATURES in order"
    );

    let root = temp_dir("target_feature_query");
    let main = root.join("main.nia");
    let check = |target: &str, level: &str| {
        support::nia_command()
            .args(["--target", target, "check", "--cpu", level])
            .arg(&main)
            .output_timeout_for_compiler("check the feature query")
    };
    use nia_target::CpuLevel;
    for (target, level) in [
        ("x86_64-unknown-linux", CpuLevel::Baseline),
        ("x86_64-unknown-linux", CpuLevel::X86_64V2),
        ("x86_64-unknown-linux", CpuLevel::X86_64V3),
        ("x86_64-unknown-linux", CpuLevel::X86_64V4),
        ("x86_64-apple-macos", CpuLevel::Baseline),
        ("x86-unknown-linux", CpuLevel::Baseline),
        ("aarch64-unknown-linux", CpuLevel::Baseline),
    ] {
        let config = nia_target::TargetConfig::parse(target).expect("maintained target");
        let expected = nia_target::CpuFeatures::for_level(config, level).expect("named level");
        std::fs::write(&main, feature_assertions(expected, None)).expect("write assertions");
        let output = check(target, level.name());
        assert!(
            output.status.success(),
            "{target} {level}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // A wrong expectation must fail, so the assertions above were evaluated.
    let config =
        nia_target::TargetConfig::parse("x86_64-unknown-linux").expect("maintained target");
    let expected =
        nia_target::CpuFeatures::for_level(config, CpuLevel::X86_64V3).expect("named level");
    std::fs::write(
        &main,
        feature_assertions(expected, Some(nia_target::CpuFeature::Avx2)),
    )
    .expect("write a wrong assertion");
    let output = check("x86_64-unknown-linux", "x86_64-v3");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "a wrong expectation passed");
    assert!(stderr.contains("avx2 should be false"), "{stderr}");
    assert_eq!(stderr.matches("error[").count(), 1, "{stderr}");
}

#[test]
fn unknown_target_feature_variant_names_its_enum_in_a_const_only_file() {
    let root = temp_dir("unknown_target_feature_variant");
    let source = root.join("main.nia");
    std::fs::write(
        &source,
        "using std::builtin::target;\nconst Y: bool = target::hasFeature(.Avx3);\n",
    )
    .expect("write unknown feature");
    let output = support::nia_command()
        .args(["check", "--runtime", "bare"])
        .arg(&source)
        .output_timeout_for_compiler("check unknown feature variant");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "an unknown variant passed");
    assert!(
        stderr.contains("unknown variant `Avx3` of enum `CpuFeature`"),
        "{stderr}",
    );
    assert_eq!(stderr.matches("error[").count(), 1, "{stderr}");
}

/// Linking resolves every symbol that generated code calls, so a target that
/// lowers some operation to a routine the image lacks fails here. The program
/// zero-fills a large array, which the code generator lowers to a library
/// call (`memset`, or `bzero` on Darwin), and each link mode is used because
/// link-time optimization decides which symbols stay visible to the linker.
#[test]
fn every_maintained_target_links_executables_at_every_lto_mode() {
    let root = temp_dir("cross_target_links");
    let source = root.join("main.nia");
    std::fs::write(
        &source,
        r#"
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    let mut buffer: [u8; 4096] = [0; 4096];
    let index = init.args().len();
    buffer[index] = 1;
    if buffer[4095 - index] == 1 { process::ExitCode(1)! } else { !() }
}
"#,
    )
    .expect("write source");
    let mut failures = Vec::new();
    for target in [
        "x86_64-unknown-linux",
        "x86-unknown-linux",
        "aarch64-unknown-linux",
        "x86_64-pc-windows-msvc",
        "x86_64-apple-macos",
        "aarch64-apple-macos",
    ] {
        for level in ["-O0", "-O2"] {
            for lto in ["off", "thin", "full"] {
                let output = support::nia_command()
                    .args(["--target", target, level, "emit", "--exe"])
                    .arg(&source)
                    .args(["--lto", lto, "-o"])
                    .arg(root.join(format!("{target}{level}-{lto}")))
                    .output_timeout_for_build("cross-link executable");
                if !output.status.success() {
                    failures.push(format!(
                        "{target} {level} --lto {lto}:\n{}",
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
