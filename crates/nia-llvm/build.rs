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
    println!("cargo:rerun-if-changed=src/llvm_coff_bridge.cpp");
    println!("cargo:rerun-if-env-changed=LLVM_SYS_231_PREFIX");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_FEATURE");

    let includedir = llvm_config_output(&["--includedir"]);
    let cxxflags = llvm_config_output(&["--cxxflags"]);
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .include(includedir)
        .file("src/llvm_lto_bridge.cpp")
        .file("src/llvm_coff_bridge.cpp")
        .warnings(false);

    // Nia's MSVC build requires the static CRT in Rust, this bridge, and LLVM.
    // llvm-config --shared-mode describes LLVM library linkage, not its CRT;
    // use Cargo's target feature for the Rust/bridge contract. The LLVM prefix
    // must independently have been built with CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded.
    let target_features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "msvc" {
        let crt_static = target_features.split(',').any(|f| f == "crt-static");
        if !crt_static {
            panic!(
                "Nia's MSVC build requires the static CRT; enable -Ctarget-feature=+crt-static and use an LLVM prefix built with CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded"
            );
        }
        build.static_crt(true);
    }

    for flag in shlex::split(&cxxflags).expect("llvm-config returned malformed C++ flags") {
        if flag.starts_with("-D") || flag == "-fno-exceptions" || flag == "-fno-rtti" {
            build.flag(&flag);
        }
    }
    build.compile("nia_llvm_bridge");
}
