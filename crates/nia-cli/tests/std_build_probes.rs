// SPDX-License-Identifier: GPL-3.0-or-later
mod support;

use nia_test_support::{CommandExt, test_dir as temp_dir};

#[test]
fn build_environment_probes_distinguish_absent_empty_and_changed_values() {
    let root = temp_dir("build_environment_probes_distinguish_states");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;

pub fn build(b: &mut build::Build) build::Error!() {
    _ = b.probeEnv(&"NIA_BUILD_PROBE_ABSENT").?;
    _ = b.probeEnv(&"NIA_BUILD_PROBE_EMPTY").?;
    _ = b.probeEnv(&"NIA_BUILD_PROBE_VALUE").?;
    let step = b.addAggregateStep(&"default").?;
    b.setDefaultStep(step)
}

"#,
    )
    .expect("write build script");

    let first = support::nia_command()
        .env_remove("NIA_BUILD_PROBE_ABSENT")
        .env("NIA_BUILD_PROBE_EMPTY", "")
        .env("NIA_BUILD_PROBE_VALUE", "first")
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg("--print-cache")
        .arg("--print-observations")
        .output_timeout_for_build("run environment probe build");
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let observations = String::from_utf8_lossy(&first.stdout);
    assert!(
        observations.contains("NIA_BUILD_PROBE_ABSENT"),
        "{observations}"
    );
    assert!(
        observations.contains("NIA_BUILD_PROBE_EMPTY"),
        "{observations}"
    );
    assert!(
        observations.contains("NIA_BUILD_PROBE_VALUE"),
        "{observations}"
    );

    let warm = support::nia_command()
        .env_remove("NIA_BUILD_PROBE_ABSENT")
        .env("NIA_BUILD_PROBE_EMPTY", "")
        .env("NIA_BUILD_PROBE_VALUE", "first")
        .args(["build", "--root"])
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("reuse unchanged environment observations");
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(
        String::from_utf8_lossy(&warm.stdout).contains("runner reused"),
        "{}",
        String::from_utf8_lossy(&warm.stdout)
    );
    let second = support::nia_command()
        .env_remove("NIA_BUILD_PROBE_ABSENT")
        .env("NIA_BUILD_PROBE_EMPTY", "")
        .env("NIA_BUILD_PROBE_VALUE", "second")
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("rerun environment probe build");
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let cache = String::from_utf8_lossy(&second.stdout);
    assert!(cache.contains("observation changed"), "{cache}");
    let present = support::nia_command()
        .env("NIA_BUILD_PROBE_ABSENT", "")
        .env("NIA_BUILD_PROBE_EMPTY", "")
        .env("NIA_BUILD_PROBE_VALUE", "second")
        .args(["build", "--root"])
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("observe absent variable becoming explicitly empty");
    assert!(
        present.status.success(),
        "{}",
        String::from_utf8_lossy(&present.stderr)
    );
    assert!(
        String::from_utf8_lossy(&present.stdout).contains("observation changed"),
        "{}",
        String::from_utf8_lossy(&present.stdout)
    );
}

#[test]
fn build_directory_probe_records_entry_size_and_invalidates_on_resize() {
    let root = temp_dir("build_directory_probe_records_entry_size");
    std::fs::create_dir(root.join("inputs")).expect("create observed directory");
    std::fs::write(
        root.join("nia.package"),
        "name=probe\nversion=0.1.0\nexclude=inputs\n",
    )
    .expect("write package manifest");
    std::fs::write(
        root.join("build.nia"),
        r#"
using std::build;
using std::fs;

pub fn build(b: &mut build::Build) build::Error!() {
    let mut buffer: [u8; 4096] = [0; 4096];
    b.probeDirectory(fs::PathView::init(&"inputs"), &mut buffer[..]).?;
    let step = b.addAggregateStep(&"default").?;
    b.setDefaultStep(step)
}
"#,
    )
    .expect("write build script");

    let first = support::nia_command()
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg("--print-observations")
        .output_timeout_for_build("run directory probe build");
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        String::from_utf8_lossy(&first.stdout).contains("directory-contents"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let warm = support::nia_command()
        .args(["build", "--root"])
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("reuse unchanged directory observation");
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(
        String::from_utf8_lossy(&warm.stdout).contains("runner reused"),
        "{}",
        String::from_utf8_lossy(&warm.stdout)
    );
    std::fs::write(root.join("inputs/value.txt"), b"one").expect("write observed entry");
    let second = support::nia_command()
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("rerun directory probe after entry creation");
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(
        String::from_utf8_lossy(&second.stdout).contains("observation changed"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );

    std::fs::write(root.join("inputs/value.txt"), b"one-two").expect("resize observed entry");
    let third = support::nia_command()
        .arg("build")
        .arg("--root")
        .arg(&root)
        .arg("--print-cache")
        .output_timeout_for_build("rerun directory probe after entry resize");
    assert!(
        third.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&third.stderr)
    );
    assert!(
        String::from_utf8_lossy(&third.stdout).contains("observation changed"),
        "{}",
        String::from_utf8_lossy(&third.stdout)
    );
}

#[test]
fn directory_iterator_compiles_for_all_linux_targets() {
    let root = temp_dir("directory_iterator_linux_targets");
    let source = root.join("directory.nia");
    std::fs::write(
        &source,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut directory = fs::Dir::cwd().?;
    defer directory.close().?;
    let mut buffer: [u8; 4096] = [0; 4096];
    let mut entries = directory.entries(&mut buffer[..]).?;
    defer entries.close().?;
    let mut size: u64 = 0;
    loop {
        let entry = match entries.next() {
            ?result => result.?,
            null => break,
        };
        size = size | entry.size();
    }
    if size == u64::MAX { return process::ExitCode(1)!; }
    !()
}
"#,
    )
    .expect("write Linux directory iterator source");
    for target in nia_test_support::LINUX_TARGETS {
        let output = support::nia_command()
            .args(["--target", target, "check"])
            .arg(&source)
            .output_timeout_for_build("check Linux directory iterator");
        assert!(
            output.status.success(),
            "target {target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
