use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::MaintainResult;

// A bodyless `extern fn` is an import; one with a body is an export. The
// optional `linkName` attribute names the imported symbol.
static IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:@\[linkName\("([^"]+)"\)\]\s*)?(?:pub(?:\([a-z]+\))?\s+)?extern fn ([A-Za-z_][A-Za-z0-9_]*)\((?:[^()]|\([^()]*\))*\)[^;{]*;"#,
    )
    .expect("valid import regex")
});

/// Windows sources whose imports the toolchain's descriptions must cover.
const WINDOWS_SOURCES: [&str; 3] = [
    "lib/std/os/windows",
    "lib/runtime/start/freestanding/windows",
    "lib/runtime/builtins/windows",
];
const WINDOWS_DESCRIPTIONS: &str = "lib/imports/windows";

/// Checks that every Windows system function the standard library and runtime
/// import is exported by exactly one toolchain import description, and that
/// the descriptions list nothing else.
pub fn run(root: &Path) -> MaintainResult<()> {
    let mut imports = BTreeSet::new();
    for directory in WINDOWS_SOURCES {
        for path in nia_sources(&root.join(directory))? {
            let source = read(&path)?;
            for capture in IMPORT.captures_iter(&source) {
                let symbol = capture.get(1).or(capture.get(2)).expect("import name");
                imports.insert(symbol.as_str().to_owned());
            }
        }
    }
    let mut exports = BTreeMap::<String, Vec<String>>::new();
    for path in files_with_extension(&root.join(WINDOWS_DESCRIPTIONS), "def")? {
        let library = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("invalid import description name {}", path.display()))?
            .to_owned();
        for symbol in module_definition_exports(&read(&path)?) {
            exports.entry(symbol).or_default().push(library.clone());
        }
    }

    let mut errors = Vec::new();
    for symbol in &imports {
        match exports.get(symbol).map(Vec::as_slice) {
            None | Some([]) => errors.push(format!(
                "`{symbol}` is imported but no description in {WINDOWS_DESCRIPTIONS} exports it"
            )),
            Some([_]) => {}
            Some(libraries) => errors.push(format!(
                "`{symbol}` is exported by more than one description: {}",
                libraries.join(", ")
            )),
        }
    }
    for (symbol, libraries) in &exports {
        if !imports.contains(symbol) {
            errors.push(format!(
                "`{symbol}` is exported by {} but nothing imports it",
                libraries.join(", ")
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "system import descriptions do not match the Windows sources:\n  {}",
            errors.join("\n  ")
        ))
    }
}

// Exports follow the `EXPORTS` statement, one name per line; `;` starts a
// comment. Ordinals and other attributes are not used by Nia descriptions.
fn module_definition_exports(source: &str) -> Vec<String> {
    let mut in_exports = false;
    let mut exports = Vec::new();
    for line in source.lines() {
        let line = line.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.eq_ignore_ascii_case("EXPORTS") {
            in_exports = true;
        } else if in_exports {
            exports.extend(line.split_whitespace().next().map(str::to_owned));
        }
    }
    exports
}

fn nia_sources(directory: &Path) -> MaintainResult<Vec<PathBuf>> {
    let mut files = files_with_extension(directory, "nia")?;
    for entry in read_dir(directory)? {
        if entry.is_dir() {
            files.extend(nia_sources(&entry)?);
        }
    }
    Ok(files)
}

fn files_with_extension(directory: &Path, extension: &str) -> MaintainResult<Vec<PathBuf>> {
    Ok(read_dir(directory)?
        .into_iter()
        .filter(|path| path.is_file() && path.extension().is_some_and(|found| found == extension))
        .collect())
}

fn read_dir(directory: &Path) -> MaintainResult<Vec<PathBuf>> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?;
    entries.sort();
    Ok(entries)
}

fn read(path: &Path) -> MaintainResult<String> {
    fs::read_to_string(path).map_err(|error| format!("failed to read {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository_root;

    #[test]
    fn repository_descriptions_match_the_windows_sources() {
        run(&repository_root()).expect("system import descriptions");
    }

    #[test]
    fn imports_use_their_link_name_and_exports_ignore_comments() {
        let source = "@[linkName(\"Real\")]\npub(pkg) extern fn alias(\n    value: u32,\n) i32;\n\
                      extern fn Plain(a: &u16) ();\npub extern fn _start() () {\n}\n";
        let names = IMPORT
            .captures_iter(source)
            .map(|capture| capture.get(1).or(capture.get(2)).unwrap().as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Real", "Plain"]);
        assert_eq!(
            module_definition_exports("; note\nLIBRARY k.dll\nEXPORTS\n    A ; used\n\n    B\n"),
            ["A", "B"]
        );
    }
}
