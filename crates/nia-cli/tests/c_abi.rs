// SPDX-License-Identifier: GPL-3.0-or-later
//! `extern` functions follow each target's C calling convention exactly.
//!
//! Clang is the reference: the declarations Nia emits for a broad set of C
//! value shapes must equal the ones clang emits for the same C signatures on
//! every maintained target, and Nia and C code must exchange those values
//! correctly in both directions on every Linux target this host runs.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
mod support;
use nia_test_support::{CommandExt, test_dir};

const DECLARATIONS_C: &str = include_str!("c_abi/declarations.c");
const DECLARATIONS_NIA: &str = include_str!("c_abi/declarations.nia");
const DECLARATIONS_I128_NIA: &str = include_str!("c_abi/declarations_i128.nia");
const EXCHANGE_C: &str = include_str!("c_abi/exchange.c");
const EXCHANGE_NIA: &str = include_str!("c_abi/exchange.nia");

/// Each maintained target with the clang triple of the same C ABI.
const TARGETS: [(&str, &str); 6] = [
    ("x86_64-unknown-linux", "x86_64-unknown-linux-gnu"),
    ("x86-unknown-linux", "i686-unknown-linux-gnu"),
    ("aarch64-unknown-linux", "aarch64-unknown-linux-gnu"),
    ("x86_64-pc-windows-msvc", "x86_64-pc-windows-msvc"),
    ("x86_64-apple-macos", "x86_64-apple-macosx15.0"),
    ("aarch64-apple-macos", "aarch64-apple-macosx15.0"),
];

fn clang() -> &'static str {
    ["clang-23", "clang"]
        .into_iter()
        .find(|program| Command::new(program).arg("--version").output().is_ok())
        .expect("the C ABI reference needs clang 23 on PATH; install clang-23")
}

fn clang_triple(target: &str) -> &'static str {
    TARGETS
        .iter()
        .find(|(nia, _)| *nia == target)
        .map(|(_, clang)| *clang)
        .expect("maintained target")
}

fn clang_ir(root: &Path, triple: &str) -> String {
    let source = root.join("declarations.c");
    std::fs::write(&source, DECLARATIONS_C).expect("write C declarations");
    let output = Command::new(clang())
        .args(["-target", triple, "-O0", "-S", "-emit-llvm", "-o", "-"])
        .arg(&source)
        .output_timeout_for_build("emit clang IR");
    assert!(
        output.status.success(),
        "clang {triple}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("clang IR is UTF-8")
}

fn nia_ir(root: &Path, target: &str) -> String {
    let source = root.join(format!("declarations-{target}.nia"));
    let mut text = DECLARATIONS_NIA.to_owned();
    // i386 C has no 128-bit integer; the ABI checker rejects one there.
    if target != "x86-unknown-linux" {
        text.push_str(DECLARATIONS_I128_NIA);
    }
    std::fs::write(&source, text).expect("write Nia declarations");
    let output = support::nia_command()
        .args(["--target", target, "emit", "--llvm"])
        .arg(&source)
        .output_timeout_for_build("emit Nia IR");
    assert!(
        output.status.success(),
        "{target}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("Nia IR is UTF-8")
}

/// Replaces every named struct type in `text` with its body, so types from
/// the two compilers compare by structure rather than by name.
fn expand_named_types(text: &str, types: &HashMap<String, String>) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        result.push_str(&rest[..start]);
        let name_len = rest[start + 1..]
            .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | '$')))
            .unwrap_or(rest.len() - start - 1);
        let name = &rest[start..start + 1 + name_len];
        match types.get(name) {
            Some(body) => result.push_str(&expand_named_types(body, types)),
            None => result.push_str(name),
        }
        rest = &rest[start + 1 + name_len..];
    }
    result.push_str(rest);
    result
}

/// The ABI-relevant part of each declaration of a probe function, sorted.
/// Attributes that only state optimization facts are dropped.
fn probe_declarations(ir: &str) -> Vec<String> {
    let types = ir
        .lines()
        .filter_map(|line| line.split_once(" = type "))
        .filter(|(name, _)| name.starts_with('%'))
        .map(|(name, body)| (name.to_owned(), body.trim().to_owned()))
        .collect::<HashMap<_, _>>();
    let mut declarations = ir
        .lines()
        .filter(|line| {
            line.starts_with("declare ") && (line.contains("@t_") || line.contains("@r_"))
        })
        .map(|line| {
            let line = line.split(" #").next().unwrap_or(line);
            let line = [
                " noundef",
                "dso_local ",
                " dead_on_return",
                " dead_on_unwind",
                " writable",
                " local_unnamed_addr",
            ]
            .iter()
            .fold(line.to_owned(), |line, noise| line.replace(noise, ""));
            expand_named_types(line.trim_end(), &types)
        })
        .collect::<Vec<_>>();
    declarations.sort();
    declarations
}

/// The argument types of the variadic probe call, without value names.
fn variadic_call(ir: &str) -> String {
    let line = ir
        .lines()
        .find(|line| line.contains("call ") && line.contains("@t_var("))
        .expect("variadic probe call");
    let args = &line[line.find("@t_var(").expect("callee") + "@t_var(".len()..];
    let args = &args[..args.rfind(')').expect("argument list end")];
    args.replace(" noundef", "")
        .replace(" dead_on_return", "")
        .split(", ")
        .map(|arg| mask_byval_type(arg.rsplit_once(' ').map_or(arg, |(ty, _)| ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `byval(T)` names the struct differently in the two compilers.
fn mask_byval_type(ty: &str) -> String {
    match ty.split_once("byval(") {
        Some((before, after)) => {
            let after = after.split_once(')').map_or("", |(_, rest)| rest);
            format!("{before}byval(T){after}")
        }
        None => ty.to_owned(),
    }
}

#[test]
fn extern_declarations_match_clang_on_every_target() {
    let root = test_dir("extern_declarations_match_clang_on_every_target");
    for (target, triple) in TARGETS {
        let clang = clang_ir(&root, triple);
        let nia = nia_ir(&root, target);
        let expected = probe_declarations(&clang);
        assert!(expected.len() >= 58, "{target}: {expected:#?}");
        assert_eq!(probe_declarations(&nia), expected, "{target}");
        assert_eq!(variadic_call(&nia), variadic_call(&clang), "{target}");
    }
}

#[test]
fn c_and_nia_exchange_values_on_every_linux_target() {
    let root = test_dir("c_and_nia_exchange_values_on_every_linux_target");
    let c_source = root.join("exchange.c");
    std::fs::write(&c_source, EXCHANGE_C).expect("write C half");
    let nia_source = root.join("exchange.nia");
    std::fs::write(&nia_source, EXCHANGE_NIA).expect("write Nia half");
    for target in nia_test_support::LINUX_TARGETS {
        let object = root.join(format!("exchange-{target}.o"));
        let compiled = Command::new(clang())
            .args(["-target", clang_triple(target), "-O1", "-ffreestanding"])
            .args(["-fno-stack-protector", "-fPIE", "-c"])
            .arg(&c_source)
            .arg("-o")
            .arg(&object)
            .output_timeout_for_build("compile C half");
        assert!(
            compiled.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        for optimization in ["-O0", "-O2"] {
            let exe = root.join(format!("exchange-{target}{optimization}"));
            let linked = support::nia_command()
                .args(["--target", target, "emit", "--exe", optimization])
                .arg(&nia_source)
                .arg("--link-arg")
                .arg(&object)
                .arg("-o")
                .arg(&exe)
                .output_timeout_for_build("link exchange executable");
            assert!(
                linked.status.success(),
                "{target}: {}",
                String::from_utf8_lossy(&linked.stderr)
            );
            // The exit status names the first failing check: below 100 on
            // the Nia side, 100 and above on the C side.
            let status = nia_test_support::linux_target_command(target, &exe)
                .output_timeout_for_runtime("run exchange executable")
                .status;
            assert_eq!(status.code(), Some(0), "{target} {optimization}");
        }
    }
}
