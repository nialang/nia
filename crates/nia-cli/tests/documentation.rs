// SPDX-License-Identifier: GPL-3.0-or-later
//! Every `nia` code block in the repository's documentation type-checks.
//!
//! The fence's info string says how a block is checked:
//!
//! | Fence | Checked as |
//! | --- | --- |
//! | ` ```nia ` | a complete source file |
//! | ` ```nia fragment ` | statements, inside a function body |
//! | ` ```nia continued ` | appended to the previous checked block of the file |
//! | ` ```nia ignore ` | not checked |
//!
//! A source that declares `pub fun main(init` is checked with the
//! `freestanding` runtime, so its entry signature is verified too; every other
//! source is checked with `bare`. `ignore` is for literal lists, multi-file
//! layouts whose other files are not shown, and build-script excerpts that
//! need a surrounding `build` function.

use std::path::{Path, PathBuf};
mod support;
use nia_test_support::{CommandExt, test_dir as temp_dir};

/// Each documentation file with `nia` blocks gets its own test, so files are
/// checked in parallel. `every_documented_file_is_listed` keeps the list
/// complete.
const DOCUMENTS: [&str; 21] = [
    "README.md",
    "lib/README.md",
    "crates/nia-body-check/README.md",
    "docs/learn/README.md",
    "docs/learn/getting-started.md",
    "docs/learn/basics.md",
    "docs/learn/structs-and-enums.md",
    "docs/learn/pointers-and-slices.md",
    "docs/learn/errors.md",
    "docs/learn/memory.md",
    "docs/learn/generics-and-traits.md",
    "docs/learn/closures.md",
    "docs/learn/modules.md",
    "docs/learn/testing.md",
    "docs/guides/build-scripts.md",
    "docs/guides/targets.md",
    "docs/guides/c-interop.md",
    "docs/reference/abi.md",
    "docs/reference/builtins.md",
    "docs/reference/language.md",
    "docs/reference/platform-support.md",
];

struct CheckedBlock {
    line: usize,
    source: String,
}

fn checked_blocks(markdown: &str, document: &str) -> Vec<CheckedBlock> {
    let mut blocks = Vec::new();
    let mut previous = String::new();
    let mut open: Option<(usize, String, String)> = None;
    for (index, line) in markdown.lines().enumerate() {
        let trimmed = line.trim_end();
        match open.as_mut() {
            None => {
                if let Some(info) = trimmed.strip_prefix("```nia") {
                    let mode = info.trim();
                    assert!(
                        info.is_empty() || info.starts_with(' '),
                        "{document}:{}: unknown fence `{trimmed}`",
                        index + 1
                    );
                    open = Some((index + 1, mode.to_owned(), String::new()));
                }
            }
            Some((start, mode, body)) => {
                if trimmed == "```" {
                    let source = match mode.as_str() {
                        "" => std::mem::take(body),
                        "fragment" => format!("fun fragment() {{\n{body}}}\n"),
                        "continued" => format!("{previous}{body}"),
                        "ignore" => {
                            open = None;
                            continue;
                        }
                        other => {
                            panic!("{document}:{start}: unknown `nia` fence mode `{other}`")
                        }
                    };
                    previous.clone_from(&source);
                    blocks.push(CheckedBlock {
                        line: *start,
                        source,
                    });
                    open = None;
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
    }
    assert!(open.is_none(), "{document}: unterminated `nia` code block");
    blocks
}

fn check_document(document: &str) {
    let path = nia_test_support::workspace_root().join(document);
    let markdown = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let blocks = checked_blocks(&markdown, document);
    assert!(!blocks.is_empty(), "{document} has no checked `nia` blocks");

    let root = temp_dir(&format!(
        "documentation_{}",
        document.replace(['/', '.'], "_")
    ));
    let mut failures = Vec::new();
    for block in blocks {
        let source = root.join(format!("line_{}.nia", block.line));
        std::fs::write(&source, &block.source).expect("write documentation block");
        let runtime = if block.source.contains("pub fun main(init") {
            "freestanding"
        } else {
            "bare"
        };
        let output = support::nia_command()
            .args(["check", "--runtime", runtime])
            .arg(&source)
            .output_timeout_for_compiler(&format!("check {document}:{}", block.line));
        if !output.status.success() {
            failures.push(format!(
                "{document}:{}:\n{}",
                block.line,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn markdown_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read documentation directory") {
        let path = entry.expect("documentation entry").path();
        if path.is_dir() {
            markdown_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

#[test]
fn every_documented_file_is_listed() {
    let workspace = nia_test_support::workspace_root();
    let mut files = vec![
        workspace.join("README.md"),
        workspace.join("lib/README.md"),
        workspace.join("examples/README.md"),
    ];
    markdown_files(&workspace.join("docs"), &mut files);
    for entry in std::fs::read_dir(workspace.join("crates")).expect("read crates") {
        let readme = entry.expect("crate entry").path().join("README.md");
        if readme.is_file() {
            files.push(readme);
        }
    }
    // A file whose `nia` blocks are all `ignore` has nothing to check.
    let mut documented = files
        .into_iter()
        .filter(|path| {
            std::fs::read_to_string(path)
                .expect("read markdown")
                .lines()
                .map(str::trim_end)
                .any(|line| line.starts_with("```nia") && line != "```nia ignore")
        })
        .map(|path| {
            path.strip_prefix(workspace)
                .expect("documentation inside the workspace")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();
    documented.sort();
    let mut listed = DOCUMENTS.map(str::to_owned).to_vec();
    listed.sort();
    assert_eq!(documented, listed);
}

macro_rules! document_tests {
    ($($name:ident => $index:expr,)*) => {
        $(
            #[test]
            fn $name() {
                check_document(DOCUMENTS[$index]);
            }
        )*
    };
}

document_tests! {
    readme_blocks_check => 0,
    std_overview_blocks_check => 1,
    closure_inference_blocks_check => 2,
    learn_index_blocks_check => 3,
    learn_getting_started_blocks_check => 4,
    learn_basics_blocks_check => 5,
    learn_structs_and_enums_blocks_check => 6,
    learn_pointers_and_slices_blocks_check => 7,
    learn_errors_blocks_check => 8,
    learn_memory_blocks_check => 9,
    learn_generics_and_traits_blocks_check => 10,
    learn_closures_blocks_check => 11,
    learn_modules_blocks_check => 12,
    learn_testing_blocks_check => 13,
    build_scripts_guide_blocks_check => 14,
    targets_guide_blocks_check => 15,
    c_interop_guide_blocks_check => 16,
    abi_reference_blocks_check => 17,
    builtins_reference_blocks_check => 18,
    language_reference_blocks_check => 19,
    platform_support_reference_blocks_check => 20,
}
