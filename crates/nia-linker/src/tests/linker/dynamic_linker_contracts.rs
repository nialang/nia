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
#[cfg(target_os = "linux")]
fn elf_interpreter_reads_bounded_program_headers_and_payload() {
    let root = env::temp_dir().join(format!("nia-linker-elf-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create ELF test root");
    let path = root.join("program");
    let interpreter = b"/lib64/ld-linux-x86-64.so.2\0";
    let mut elf = vec![0; 64 + 56 + interpreter.len()];
    elf[0..4].copy_from_slice(b"\x7fELF");
    elf[4] = 2;
    elf[5] = 1;
    elf[32..40].copy_from_slice(&64u64.to_le_bytes());
    elf[54..56].copy_from_slice(&56u16.to_le_bytes());
    elf[56..58].copy_from_slice(&1u16.to_le_bytes());
    elf[64..68].copy_from_slice(&3u32.to_le_bytes());
    elf[72..80].copy_from_slice(&120u64.to_le_bytes());
    elf[96..104].copy_from_slice(&(interpreter.len() as u64).to_le_bytes());
    elf[120..].copy_from_slice(interpreter);
    fs::write(&path, elf).expect("write ELF fixture");

    assert_eq!(
        elf_interpreter(&path).expect("read ELF interpreter"),
        Some("/lib64/ld-linux-x86-64.so.2".to_string())
    );

    let mut oversized = fs::read(&path).expect("read ELF fixture");
    oversized[96..104].copy_from_slice(&4097u64.to_le_bytes());
    fs::write(&path, oversized).expect("write oversized ELF fixture");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open oversized ELF fixture");
    file.set_len(120 + 4097)
        .expect("extend oversized ELF fixture");
    assert!(matches!(
        elf_interpreter(&path),
        Err(LinkerConfigError::InvalidElf { .. })
    ));
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

#[test]
fn ld_so_conf_includes_are_sorted_and_oversized_files_are_ignored() {
    let root = env::temp_dir().join(format!(
        "nia-linker-ld-so-conf-bounds-{}",
        std::process::id()
    ));
    let first = root.join("first");
    let second = root.join("second");
    let smuggled = root.join("smuggled");
    let conf_dir = root.join("conf.d");
    for path in [&first, &second, &smuggled, &conf_dir] {
        fs::create_dir_all(path).expect("create ld.so.conf fixture directory");
    }
    fs::write(
        root.join("ld.so.conf"),
        format!("include {}\n", conf_dir.join("*.conf").display()),
    )
    .expect("write root conf");
    fs::write(conf_dir.join("b.conf"), format!("{}\n", second.display()))
        .expect("write second conf");
    fs::write(conf_dir.join("a.conf"), format!("{}\n", first.display())).expect("write first conf");
    let oversized = conf_dir.join("c.conf");
    fs::write(&oversized, format!("{}\n", smuggled.display()))
        .expect("write oversized conf prefix");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&oversized)
        .expect("open oversized conf");
    file.set_len((MAX_LD_SO_CONF_FILE_BYTES + 1) as u64)
        .expect("extend oversized conf");

    let mut paths = Vec::new();
    read_ld_so_conf(&mut paths, &root.join("ld.so.conf"), 0);

    assert_eq!(
        paths,
        [
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ]
    );
}

#[test]
fn ld_so_conf_reader_visits_canonical_files_once() {
    let root = env::temp_dir().join(format!(
        "nia-linker-ld-so-conf-cycle-{}",
        std::process::id()
    ));
    let lib = root.join("lib");
    fs::create_dir_all(&lib).expect("create lib dir");
    fs::write(
        root.join("ld.so.conf"),
        format!(
            "{}\ninclude {}\n",
            lib.display(),
            root.join("ld.so.conf").display()
        ),
    )
    .expect("write cyclic conf");

    let mut paths = Vec::new();
    read_ld_so_conf(&mut paths, &root.join("ld.so.conf"), 0);

    assert_eq!(paths, [lib.to_string_lossy().into_owned()]);
}

#[test]
#[cfg(unix)]
fn ld_so_conf_symlink_keeps_the_include_base_of_the_visible_path() {
    use std::os::unix::fs::symlink;

    let root = env::temp_dir().join(format!(
        "nia-linker-ld-so-conf-symlink-{}",
        std::process::id()
    ));
    let visible = root.join("visible");
    let storage = root.join("storage");
    let lib = visible.join("lib");
    fs::create_dir_all(&lib).expect("create visible lib dir");
    fs::create_dir_all(&storage).expect("create storage dir");
    fs::write(storage.join("real.conf"), "include child.conf\n").expect("write real config");
    fs::write(visible.join("child.conf"), format!("{}\n", lib.display()))
        .expect("write visible child config");
    symlink(storage.join("real.conf"), visible.join("ld.so.conf")).expect("link visible config");

    let mut paths = Vec::new();
    read_ld_so_conf(&mut paths, &visible.join("ld.so.conf"), 0);

    assert_eq!(paths, [lib.to_string_lossy().into_owned()]);
}
