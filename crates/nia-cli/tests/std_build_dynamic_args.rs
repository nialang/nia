// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn build_accepts_more_than_legacy_import_and_argv_limits() {
    let root = temp_dir("build_accepts_more_than_legacy_import_and_argv_limits");
    std::fs::create_dir_all(root.join("src")).expect("create source directory");
    std::fs::write(
        root.join("src/main.nia"),
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    !{}
}
"#,
    )
    .expect("write executable source");
    std::fs::write(root.join("provider.nia"), "pub fn value() i32 { 1 }")
        .expect("write import provider");

    let mut build_source = String::from(
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!void {
    let imports = [
"#,
    );
    for index in 0..32 {
        build_source.push_str(&format!(
            "        build::ModuleImport::init(&\"dep{index:02}\", fs::PathView::init(&\"provider.nia\")),\n"
        ));
    }
    build_source.push_str(
        r#"    ];
    let rootModule = b.addModule(
        build::ModuleOptions::init(fs::PathView::init(&"src/main.nia"))
            .withImports(&imports[..]),
    ).?;
    let executable = b.addExecutable(
        build::ExecutableOptions::init(&"many-imports", rootModule),
    ).?;
    let emit = b.addEmitExecutableStep(&"emit", executable).?;
    b.setDefaultStep(emit)
}
"#,
    );
    std::fs::write(root.join("build.nia"), build_source).expect("write build script");

    let output = support::nia_command()
        .arg("build")
        .arg("--root")
        .arg(&root)
        .output_timeout_for_build("run build with dynamic import argv");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let executable = root.join(".nia-build/many-imports");
    assert_eq!(
        Command::new(&executable)
            .status_timeout("run many-import executable")
            .code(),
        Some(0)
    );
}
