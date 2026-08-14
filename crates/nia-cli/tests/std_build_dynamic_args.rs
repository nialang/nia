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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    !()
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
using std::string;

pub fn build(b: &mut build::Build) build::Error!() {
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
        build::ModuleOptions::init(&"root", fs::PathView::init(&"src/main.nia"))
            .withImports(&imports[..]),
    ).?;
    let executable = b.addExecutable(
        build::ExecutableOptions::init(&"many-imports", rootModule),
    ).?;
    _ = b.addEmitExecutableStep(&"emit", executable).?;
    let runArguments: [&[char]; 32] = [
"#,
    );
    for index in 0..32 {
        build_source.push_str(&format!("        &\"argument-{index:02}\",\n"));
    }
    build_source.push_str(
        r#"    ];
    let run = b.addRunExecutableStep(
        &"run",
        build::RunOptions::init(executable).withArguments(&runArguments[..]),
    ).?;
    b.setDefaultStep(run)
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
    let plan = nia_build::read_build_plan(&root.join(".nia-build/build-plan.bin"))
        .expect("decode dynamic argument build plan");
    let arguments = plan
        .actions()
        .iter()
        .find_map(|action| match &action.kind {
            nia_build::ActionKind::ExternalCommand { arguments, .. } => Some(arguments),
            _ => None,
        })
        .expect("run action in dynamic argument build plan");
    assert_eq!(arguments.len(), 32);
    assert_eq!(
        arguments.first(),
        Some(&nia_build::CommandArgument::Literal(
            "argument-00".to_string()
        ))
    );
    assert_eq!(
        arguments.last(),
        Some(&nia_build::CommandArgument::Literal(
            "argument-31".to_string()
        ))
    );
    let executable = root.join(".nia-build/many-imports");
    assert_eq!(
        Command::new(&executable)
            .status_timeout("run many-import executable")
            .code(),
        Some(0)
    );
}
