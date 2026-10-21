// SPDX-License-Identifier: GPL-3.0-or-later
//! Exercise SSH checkout through the public API with an isolated client process.

use cap_std::{ambient_authority, fs::Dir};
use nia_git::{
    checkout_source, restrict_test_identity_permissions, with_test_ssh_checkout_fixture,
};
use std::path::PathBuf;
use std::process::Command;

#[test]
fn ssh_key_exchange_authenticates_and_opens_git_exec_channel() {
    // Re-enter this test in a child so HOME and agent selection never affect
    // other tests running in the parent process.
    if let Some(source) = std::env::var_os("NIA_SSH_TEST_SOURCE") {
        let destination = PathBuf::from(std::env::var_os("NIA_SSH_TEST_DESTINATION").unwrap());
        std::fs::create_dir(&destination).unwrap();
        let destination_dir = Dir::open_ambient_dir(&destination, ambient_authority()).unwrap();
        let expected_revision = std::env::var("NIA_SSH_TEST_REVISION").unwrap();
        let revision = checkout_source(source.to_str().unwrap(), "HEAD", &destination_dir).unwrap();
        assert_eq!(revision, expected_revision);
        assert_eq!(
            std::fs::read(destination.join("file.txt")).unwrap(),
            b"native SSH checkout\n"
        );
        assert!(!destination.join(".git").exists());
        return;
    }

    with_test_ssh_checkout_fixture(|root, source, config, repository| {
        let home = root.join("client-home");
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        for name in ["known_hosts", "id_ed25519"] {
            std::fs::copy(config.join(name), ssh.join(name)).unwrap();
        }
        restrict_test_identity_permissions(&ssh.join("id_ed25519"));
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repository)
            .output()
            .unwrap();
        assert!(revision.status.success());
        let revision = String::from_utf8(revision.stdout).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ssh_key_exchange_authenticates_and_opens_git_exec_channel",
                "--nocapture",
            ])
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("SSH_AUTH_SOCK", root.join("absent-agent"))
            .env("NIA_SSH_TEST_SOURCE", source)
            .env("NIA_SSH_TEST_DESTINATION", root.join("checkout"))
            .env("NIA_SSH_TEST_REVISION", revision.trim())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    });
}
