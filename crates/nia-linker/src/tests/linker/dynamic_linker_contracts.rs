use super::*;

#[test]
fn standard_dynamic_linker_covers_common_linux_gnu_targets() {
    assert_eq!(
        standard_dynamic_linker_for(&target("x86_64", "linux", "gnu")).as_deref(),
        Some("/lib64/ld-linux-x86-64.so.2")
    );
    assert_eq!(
        standard_dynamic_linker_for(&target("aarch64", "linux", "gnu")).as_deref(),
        Some("/lib/ld-linux-aarch64.so.1")
    );
    assert_eq!(
        standard_dynamic_linker_for(&target("riscv64", "linux", "gnu")).as_deref(),
        Some("/lib/ld-linux-riscv64-lp64d.so.1")
    );
}

#[test]
fn standard_dynamic_linker_covers_common_linux_musl_targets() {
    assert_eq!(
        standard_dynamic_linker_for(&target("x86_64", "linux", "musl")).as_deref(),
        Some("/lib/ld-musl-x86_64.so.1")
    );
    assert_eq!(
        standard_dynamic_linker_for(&target("aarch64", "linux", "musl")).as_deref(),
        Some("/lib/ld-musl-aarch64.so.1")
    );
    assert_eq!(
        standard_dynamic_linker_for(&target("arm", "linux", "musleabihf")).as_deref(),
        Some("/lib/ld-musl-armhf.so.1")
    );
}

#[test]
#[cfg(target_os = "linux")]
fn detects_native_dynamic_linker_from_usr_bin_env() {
    let dynamic_linker = native_dynamic_linker().expect("native dynamic linker");
    assert!(
        dynamic_linker
            .as_deref()
            .is_some_and(|path| path.contains("ld-linux") || path.contains("ld-musl")),
        "{dynamic_linker:?}"
    );
}

#[test]
fn ld_so_conf_reader_follows_simple_include_patterns() {
    let root = env::temp_dir().join(format!("nia-linker-ld-so-conf-{}", std::process::id()));
    let lib = root.join("lib");
    let conf_dir = root.join("conf.d");
    fs::create_dir_all(&lib).expect("create lib dir");
    fs::create_dir_all(&conf_dir).expect("create conf dir");
    fs::write(
        root.join("ld.so.conf"),
        format!("include {}\n", conf_dir.join("*.conf").display()),
    )
    .expect("write root conf");
    fs::write(conf_dir.join("local.conf"), format!("{}\n", lib.display()))
        .expect("write included conf");

    let mut paths = Vec::new();
    read_ld_so_conf(&mut paths, &root.join("ld.so.conf"), 0);

    assert!(
        paths.contains(&lib.to_string_lossy().into_owned()),
        "{paths:?}"
    );
}
