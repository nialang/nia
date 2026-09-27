use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

fn llvm_config() -> OsString {
    env::var_os("DEP_LLVM_23_CONFIG_PATH").unwrap_or_else(|| OsString::from("llvm-config"))
}

fn llvm_config_output(arguments: &[&str]) -> String {
    let program = llvm_config();
    let output = Command::new(&program)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to run {}: {error}",
                PathBuf::from(&program).display()
            )
        });
    if !output.status.success() {
        panic!(
            "{} {} failed with {}: {}",
            PathBuf::from(&program).display(),
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout)
        .expect("llvm-config output must be UTF-8")
        .trim()
        .to_owned()
}

fn main() {
    println!("cargo:rerun-if-changed=src/llvm_lto_bridge.cpp");
    println!("cargo:rerun-if-env-changed=LLVM_SYS_231_PREFIX");

    let includedir = llvm_config_output(&["--includedir"]);
    let cxxflags = llvm_config_output(&["--cxxflags"]);
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .include(includedir)
        .file("src/llvm_lto_bridge.cpp")
        .warnings(false);

    // Keep the bridge's MSVC runtime aligned with the Rust target. The
    // prebuilt Windows LLVM archives use /MT, while a locally built LLVM may
    // use /MD; Rust's `crt-static` feature is the stable source of truth for
    // the host binary and its C++ bridge.
    let target_features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    if cfg!(target_env = "msvc") && target_features.split(',').any(|f| f == "crt-static") {
        build.flag("/MT");
    }

    for flag in shlex::split(&cxxflags).expect("llvm-config returned malformed C++ flags") {
        if flag.starts_with("-D") || flag == "-fno-exceptions" || flag == "-fno-rtti" {
            build.flag(&flag);
        }
    }
    build.compile("nia_llvm_lto_bridge");
}
