static ENV_LOCK: Mutex<()> = Mutex::new(());

fn link_inputs(path: &str) -> IncrementalLinkInputs<PathBuf> {
    IncrementalLinkInputs::new(vec![IncrementalLinkInput {
        key: CodegenUnitKey::SourceModule {
            source_identity: SourceIdentity::new("main.nia"),
            ordinal: 0,
        },
        fingerprint: CodegenUnitFingerprint::from_parts([1, 2]),
        object: PathBuf::from(path),
    }])
}

fn fingerprint_linker(name: &str, bytes: &[u8]) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "nia-linker-fingerprint-{name}-{}",
        std::process::id()
    ));
    fs::write(&path, bytes).expect("write fingerprint linker");
    make_executable(&path);
    path
}

fn fingerprint_options(linker: &Path) -> LinkOptions {
    LinkOptions {
        linker: ExecutableLinker::with_program(linker.to_string_lossy()),
        ..LinkOptions::default()
    }
}

fn target(arch: &str, os: &str, abi: &str) -> LinkTarget {
    LinkTarget {
        arch: arch.to_string(),
        os: os.to_string(),
        abi: abi.to_string(),
    }
}

fn restore_env(name: &str, value: Option<std::ffi::OsString>) {
    unsafe {
        if let Some(value) = value {
            env::set_var(name, value);
        } else {
            env::remove_var(name);
        }
    }
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path)
            .expect("mock linker metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("make mock linker executable");
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}
