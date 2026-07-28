use super::*;

#[test]
fn lld_invocation_uses_gnu_like_arguments() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program_and_flavor("ld.lld", LinkerFlavor::Lld),
        ..LinkOptions::default()
    }
    .add_library_path("/lib")
    .add_library("m");
    let invocation = options
        .invocation(&link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert_eq!(invocation.program, "ld.lld");
    assert!(
        invocation
            .args
            .windows(2)
            .any(|args| args == ["-e", "_start"])
    );
    assert!(invocation.args.iter().any(|arg| arg == "main.o"));
    assert!(
        invocation
            .args
            .windows(2)
            .any(|args| args == ["-L", "/lib"])
    );
    assert!(invocation.args.windows(2).any(|args| args == ["-l", "m"]));
    assert!(
        invocation
            .args
            .windows(2)
            .any(|args| args == ["-o", "main"])
    );
}

#[test]
#[cfg(target_os = "linux")]
fn lld_invocation_adds_native_linux_library_paths() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program_and_flavor("ld.lld", LinkerFlavor::Lld),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(&link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert!(
        invocation
            .args
            .windows(2)
            .any(|args| args == ["-L", "/usr/lib64"] || args == ["-L", "/lib64"]),
        "{:?}",
        invocation.args
    );
}

#[test]
fn lld_invocation_resolves_program_from_path() {
    let _guard = ENV_LOCK.lock().expect("env test lock");
    let root = env::temp_dir().join(format!("nia-linker-lld-path-{}", std::process::id()));
    let bin = root.join("bin");
    fs::create_dir_all(&bin).expect("create bin dir");
    let linker = bin.join("ld.lld");
    fs::write(&linker, "").expect("write mock linker");
    make_executable(&linker);
    let previous_path = env::var_os("PATH");
    let previous_nia_lld = env::var_os("NIA_LLD");
    unsafe {
        env::set_var("PATH", &bin);
        env::remove_var("NIA_LLD");
    }

    let options = LinkOptions {
        linker: ExecutableLinker::lld(),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(&link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert_eq!(invocation.program, linker.to_string_lossy());

    restore_env("PATH", previous_path);
    restore_env("NIA_LLD", previous_nia_lld);
}

#[test]
#[cfg(unix)]
fn lld_invocation_ignores_non_executable_program_on_path() {
    let _guard = ENV_LOCK.lock().expect("env test lock");
    let root = env::temp_dir().join(format!(
        "nia-linker-lld-non-executable-{}",
        std::process::id()
    ));
    let bin = root.join("bin");
    fs::create_dir_all(&bin).expect("create bin dir");
    fs::write(bin.join("ld.lld"), "").expect("write mock linker");
    let previous_path = env::var_os("PATH");
    let previous_nia_lld = env::var_os("NIA_LLD");
    unsafe {
        env::set_var("PATH", &bin);
        env::remove_var("NIA_LLD");
    }

    let options = LinkOptions {
        linker: ExecutableLinker::lld(),
        ..LinkOptions::default()
    };
    assert!(matches!(
        options.invocation(&link_inputs("main.o"), PathBuf::from("main")),
        Err(LinkerConfigError::LinkerNotFound {
            flavor: LinkerFlavor::Lld,
            ..
        })
    ));

    restore_env("PATH", previous_path);
    restore_env("NIA_LLD", previous_nia_lld);
}

#[test]
fn lld_invocation_reports_missing_program() {
    let _guard = ENV_LOCK.lock().expect("env test lock");
    let previous_path = env::var_os("PATH");
    let previous_nia_lld = env::var_os("NIA_LLD");
    unsafe {
        env::set_var("PATH", "");
        env::remove_var("NIA_LLD");
    }

    let options = LinkOptions {
        linker: ExecutableLinker::lld(),
        ..LinkOptions::default()
    };
    assert!(matches!(
        options.invocation(&link_inputs("main.o"), PathBuf::from("main")),
        Err(LinkerConfigError::LinkerNotFound {
            flavor: LinkerFlavor::Lld,
            ..
        })
    ));

    restore_env("PATH", previous_path);
    restore_env("NIA_LLD", previous_nia_lld);
}

#[test]
fn self_hosted_elf_flavor_is_reserved() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program_and_flavor("nia-link", LinkerFlavor::SelfHostedElf),
        ..LinkOptions::default()
    };
    assert!(matches!(
        options.invocation(&link_inputs("main.o"), PathBuf::from("main")),
        Err(LinkerConfigError::UnsupportedFlavor(
            LinkerFlavor::SelfHostedElf
        ))
    ));
}
