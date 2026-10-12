use super::*;
use std::process::Command;

#[test]
fn explicit_linker_version_probe_preserves_program_contract() {
    let linker = ExecutableLinker::with_program("chosen-linker");
    let invocation = linker.version_invocation(linux()).unwrap();
    assert_eq!(invocation.program, "chosen-linker");
    assert_eq!(invocation.args, ["--version"]);
    assert!(
        linker
            .clone()
            .with_bundled_program("other-linker")
            .version_invocation(linux())
            .is_ok_and(|probe| probe == invocation)
    );
}

#[test]
fn lld_invocation_uses_gnu_like_arguments() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program_and_flavor("ld.lld", LinkerFlavor::Lld),
        ..LinkOptions::default()
    }
    .add_library_path("/lib")
    .add_library("m");
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
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
fn lld_link_invocation_uses_coff_arguments() {
    // The COFF contract is target-owned, so it holds on every host.
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("lld-link.exe"),
        ..LinkOptions::default()
    }
    .add_library_path("C:/lib")
    .add_library("user32")
    .with_system_imports(vec![
        SystemImportLibrary::new("kernel32", "imports/kernel32.lib", b"kernel32"),
        SystemImportLibrary::new(
            "bcryptprimitives",
            "imports/bcryptprimitives.lib",
            b"bcrypt",
        ),
    ]);
    let invocation = options
        .invocation(
            target("x86_64-pc-windows-msvc"),
            &link_inputs("main.obj"),
            PathBuf::from("main.exe"),
        )
        .expect("link invocation");
    assert_eq!(invocation.program, "lld-link.exe");
    assert!(invocation.args.iter().any(|arg| arg == "/TIMESTAMP:0"));
    assert!(invocation.args.iter().any(|arg| arg == "/ENTRY:_start"));
    assert!(
        invocation
            .args
            .iter()
            .any(|arg| arg == "/SUBSYSTEM:CONSOLE")
    );
    assert!(invocation.args.iter().any(|arg| arg == "main.obj"));
    assert!(invocation.args.iter().any(|arg| arg == "/LIBPATH:C:/lib"));
    assert!(
        invocation
            .args
            .iter()
            .any(|arg| arg == "/DEFAULTLIB:user32")
    );
    // System libraries come from toolchain import descriptions, never an SDK.
    for import in ["imports/kernel32.lib", "imports/bcryptprimitives.lib"] {
        assert!(invocation.args.iter().any(|arg| arg == import), "{import}");
    }
    assert!(
        !invocation
            .args
            .iter()
            .any(|arg| arg.contains("advapi32") || arg.contains("Windows Kits")),
        "{:?}",
        invocation.args
    );
    assert!(invocation.args.iter().any(|arg| arg == "/OUT:main.exe"));
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn lld_invocation_adds_native_linux_library_paths() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program_and_flavor("ld.lld", LinkerFlavor::Lld),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
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
    // Installed as a host executable: `ld.lld.exe` on Windows, even though
    // the bare name `ld.lld` already contains a dot.
    let linker = bin.join(format!("ld.lld{}", env::consts::EXE_SUFFIX));
    fs::write(&linker, "").expect("write mock linker");
    make_executable(&linker);
    let previous_path = env::var_os("PATH");
    let previous_nia_lld = env::var_os("NIA_LLD");
    unsafe {
        env::set_var("PATH", &bin);
        env::remove_var("NIA_LLD");
    }

    let options = LinkOptions {
        linker: ExecutableLinker::with_flavor(LinkerFlavor::Lld),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
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
        linker: ExecutableLinker::with_flavor(LinkerFlavor::Lld),
        ..LinkOptions::default()
    };
    assert!(matches!(
        options.invocation(linux(), &link_inputs("main.o"), PathBuf::from("main")),
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
        linker: ExecutableLinker::with_flavor(LinkerFlavor::Lld),
        ..LinkOptions::default()
    };
    assert!(matches!(
        options.invocation(linux(), &link_inputs("main.o"), PathBuf::from("main")),
        Err(LinkerConfigError::LinkerNotFound {
            flavor: LinkerFlavor::Lld,
            ..
        })
    ));

    restore_env("PATH", previous_path);
    restore_env("NIA_LLD", previous_nia_lld);
}

#[test]
fn lld_invocation_uses_bundled_program_before_path() {
    let _guard = ENV_LOCK.lock().expect("env test lock");
    let root = env::temp_dir().join(format!("nia-linker-lld-bundled-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create bundled linker directory");
    let linker = root.join("ld.lld");
    fs::write(&linker, "").expect("write bundled linker");
    make_executable(&linker);
    let previous_path = env::var_os("PATH");
    let previous_nia_lld = env::var_os("NIA_LLD");
    unsafe {
        env::set_var("PATH", "");
        env::remove_var("NIA_LLD");
    }

    let options = LinkOptions {
        linker: ExecutableLinker::with_flavor(LinkerFlavor::Lld)
            .with_bundled_program(linker.to_string_lossy()),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect("bundled linker invocation");
    assert_eq!(invocation.program, linker.to_string_lossy());

    restore_env("PATH", previous_path);
    restore_env("NIA_LLD", previous_nia_lld);
}

#[test]
fn self_hosted_elf_flavor_is_reserved() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program_and_flavor("nia-link", LinkerFlavor::SelfHostedElf),
        ..LinkOptions::default()
    };
    let error = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect_err("reserved linker flavor must be rejected");
    assert!(matches!(
        &error,
        LinkerConfigError::UnsupportedFlavor(LinkerFlavor::SelfHostedElf)
    ));
    assert_eq!(
        error.to_string(),
        "linker flavor `self-hosted-elf` is not implemented"
    );
}

#[test]
fn linker_flavors_have_stable_user_names() {
    assert_eq!(LinkerFlavor::Gnu.to_string(), "gnu");
    assert_eq!(LinkerFlavor::Lld.to_string(), "lld");
    assert_eq!(LinkerFlavor::LldLink.to_string(), "lld-link");
    assert_eq!(LinkerFlavor::Ld64Lld.to_string(), "ld64.lld");
    assert_eq!(LinkerFlavor::SelfHostedElf.to_string(), "self-hosted-elf");
}

#[test]
fn default_flavor_follows_the_target_object_format() {
    for target in nia_target::SUPPORTED_TARGETS {
        let discovered = ExecutableLinker::with_program("")
            .flavor_for(target)
            .expect("default flavor");
        let explicit = ExecutableLinker::with_program("linker")
            .flavor_for(target)
            .expect("explicit program flavor");
        let expected = match target.object_format() {
            ObjectFormat::Elf => (LinkerFlavor::Lld, LinkerFlavor::Gnu),
            ObjectFormat::Coff => (LinkerFlavor::LldLink, LinkerFlavor::LldLink),
            ObjectFormat::MachO => (LinkerFlavor::Ld64Lld, LinkerFlavor::Ld64Lld),
        };
        assert_eq!((discovered, explicit), expected, "{target}");
    }
}

#[test]
fn discovered_lld_selects_the_target_flavor_explicitly() {
    let _guard = ENV_LOCK.lock().expect("env test lock");
    let root = env::temp_dir().join(format!("nia-linker-lld-flavor-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create bundled linker directory");
    // One LLD binary, whatever its name, links every object format.
    let linker = root.join("ld.lld");
    fs::write(&linker, "").expect("write bundled linker");
    make_executable(&linker);
    let previous_nia_lld = env::var_os("NIA_LLD");
    unsafe {
        env::remove_var("NIA_LLD");
    }

    let options = LinkOptions {
        linker: ExecutableLinker::with_program("").with_bundled_program(linker.to_string_lossy()),
        ..LinkOptions::default()
    };
    for (name, flavor) in [
        ("x86_64-unknown-linux", "gnu"),
        ("x86_64-pc-windows-msvc", "link"),
        ("aarch64-apple-macos", "darwin"),
    ] {
        let invocation = options
            .invocation(target(name), &link_inputs("main.o"), PathBuf::from("main"))
            .expect("link invocation");
        assert_eq!(invocation.program, linker.to_string_lossy());
        assert_eq!(invocation.args[..2], ["-flavor", flavor], "{name}");
        let probe = options.linker.version_invocation(target(name)).unwrap();
        assert_eq!(probe.program, invocation.program, "{name}");
        assert_eq!(probe.args, ["-flavor", flavor, "--version"], "{name}");
    }

    restore_env("NIA_LLD", previous_nia_lld);
}

#[test]
fn discovered_named_lld_link_driver_does_not_receive_flavor_argument() {
    let root = env::temp_dir().join(format!("nia-linker-lld-link-driver-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create bundled linker directory");
    let linker = root.join(format!("lld-link{}", env::consts::EXE_SUFFIX));
    fs::write(&linker, "").expect("write mock linker");
    make_executable(&linker);

    let options = LinkOptions {
        linker: ExecutableLinker::with_program("").with_bundled_program(linker.to_string_lossy()),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(
            target("x86_64-pc-windows-msvc"),
            &link_inputs("main.obj"),
            PathBuf::from("main.exe"),
        )
        .expect("COFF link invocation");
    assert_eq!(invocation.program, linker.to_string_lossy());
    assert!(!invocation.args.iter().any(|argument| argument == "-flavor"));
    assert!(
        invocation
            .args
            .iter()
            .any(|argument| argument == "main.obj")
    );

    let probe = options
        .linker
        .version_invocation(target("x86_64-pc-windows-msvc"))
        .expect("COFF linker version probe");
    assert_eq!(probe.program, invocation.program);
    assert_eq!(probe.args, ["--version"]);
}

#[test]
fn explicit_program_receives_no_flavor_selection() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program_and_flavor("ld.lld", LinkerFlavor::Lld),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert!(!invocation.args.iter().any(|arg| arg == "-flavor"));
}

#[test]
fn flavor_for_another_object_format_is_rejected() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_flavor(LinkerFlavor::LldLink),
        ..LinkOptions::default()
    };
    let error = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect_err("a COFF flavor cannot link ELF");
    assert!(matches!(
        error,
        LinkerConfigError::IncompatibleFlavor {
            flavor: LinkerFlavor::LldLink,
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        "linker flavor `lld-link` cannot link executables for target \
         `x86_64-unknown-linux`; expected `lld`"
    );
}

#[test]
fn mach_o_invocation_uses_darwin_arguments() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld64.lld"),
        ..LinkOptions::default()
    }
    .with_system_imports(vec![SystemImportLibrary::new(
        "libSystem",
        "imports/libSystem.tbd",
        b"--- !tapi-tbd\ntbd-version: 4\n",
    )]);
    let invocation = options
        .invocation(
            target("aarch64-apple-macos"),
            &link_inputs("main.o"),
            PathBuf::from("main"),
        )
        .expect("Mach-O link invocation");
    assert_eq!(invocation.program, "ld64.lld");
    assert!(
        invocation
            .args
            .windows(2)
            .any(|args| args == ["-arch", "arm64"])
    );
    assert!(
        invocation
            .args
            .windows(4)
            .any(|args| { args == ["-platform_version", "macos", "15.0", "15.0"] })
    );
    assert!(
        invocation
            .args
            .windows(2)
            .any(|args| args == ["-e", "_main"])
    );
    assert!(invocation.args.iter().any(|arg| arg == "-pie"));
    assert!(invocation.args.iter().any(|arg| arg == "-adhoc_codesign"));
    assert!(invocation.args.iter().any(|arg| arg == "main.o"));
    assert!(
        invocation
            .args
            .iter()
            .any(|arg| arg == "imports/libSystem.tbd")
    );
    assert!(
        invocation
            .args
            .windows(2)
            .any(|args| args == ["-o", "main"])
    );
}

#[test]
fn darwin_tbd_stub_satisfies_an_undefined_symbol_link() {
    let clang = if Command::new("clang")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        "clang"
    } else {
        return;
    };
    if !Command::new("ld64.lld")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }

    let root = env::temp_dir().join(format!("nia-linker-darwin-tbd-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create Darwin link test directory");
    let source = root.join("main.c");
    let object = root.join("main.o");
    let stub = root.join("libSystem.tbd");
    let output = root.join("main");
    fs::write(
        &source,
        b"extern long write(int, const void *, unsigned long); int main(void) { return (int)write(1, 0, 0); }\n",
    )
    .expect("write Darwin link source");
    fs::write(
        &stub,
        b"--- !tapi-tbd\ntbd-version:     4\ntargets:         [ arm64-macos, x86_64-macos ]\nflags:           [ not_app_extension_safe ]\ninstall-name:    '/usr/lib/libSystem.B.dylib'\ncurrent-version: 0\ncompatibility-version: 0\nexports:\n  - targets:         [ arm64-macos, x86_64-macos ]\n    symbols:        [ _write ]\n...\n",
    )
    .expect("write Darwin import stub");
    let compile = Command::new(clang)
        .args(["-target", "x86_64-apple-macosx15.0", "-ffreestanding", "-c"])
        .arg(&source)
        .args(["-o"])
        .arg(&object)
        .output()
        .expect("run clang for Darwin link test");
    assert!(
        compile.status.success(),
        "clang failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld64.lld"),
        ..LinkOptions::default()
    }
    .with_system_imports(vec![SystemImportLibrary::new(
        "libSystem",
        &stub,
        b"darwin-tbd-v4",
    )]);
    let invocation = options
        .invocation(
            target("x86_64-apple-macos"),
            &link_inputs(&object.to_string_lossy()),
            output.clone(),
        )
        .expect("Darwin link invocation");
    let link = Command::new(&invocation.program)
        .args(&invocation.args)
        .output()
        .expect("run ld64.lld for Darwin link test");
    assert!(
        link.status.success(),
        "ld64.lld failed: {}",
        String::from_utf8_lossy(&link.stderr)
    );
    assert!(output.is_file(), "ld64.lld did not produce an executable");
}
