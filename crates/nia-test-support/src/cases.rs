// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
};

/// Consuming reader for a fixture directory's `case.meta` manifest.
pub struct CaseManifest {
    path: PathBuf,
    values: BTreeMap<String, String>,
}

impl CaseManifest {
    /// Loads and validates unique non-empty `key=value` entries.
    pub fn load(case_root: &Path) -> Self {
        let path = case_root.join("case.meta");
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let mut values = BTreeMap::new();
        for (line_number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').unwrap_or_else(|| {
                panic!(
                    "invalid {}:{}; expected key=value",
                    path.display(),
                    line_number + 1
                )
            });
            let key = key.trim();
            let value = value.trim();
            assert!(
                !key.is_empty() && !value.is_empty(),
                "empty key or value in {}:{}",
                path.display(),
                line_number + 1
            );
            assert!(
                values.insert(key.to_owned(), value.to_owned()).is_none(),
                "duplicate case key {key:?} in {}",
                path.display()
            );
        }
        Self { path, values }
    }

    /// Returns the loaded manifest path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes and returns a required manifest value.
    pub fn required(&mut self, key: &str) -> String {
        self.values
            .remove(key)
            .unwrap_or_else(|| panic!("{} must declare {key}", self.path.display()))
    }

    /// Removes and parses a non-empty comma-separated value list.
    pub fn required_list(&mut self, key: &str) -> Vec<String> {
        let value = self.required(key);
        let values = value
            .split(',')
            .map(str::trim)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(
            !values.is_empty() && values.iter().all(|value| !value.is_empty()),
            "{} must declare a non-empty comma-separated {key}",
            self.path.display()
        );
        values
    }

    /// Removes a required key and asserts that its value equals `expected`.
    pub fn expect(&mut self, key: &str, expected: &str) {
        let actual = self.required(key);
        assert_eq!(
            actual,
            expected,
            "{} must declare {key}={expected}",
            self.path.display()
        );
    }

    /// Removes and parses a required `usize` value.
    pub fn required_usize(&mut self, key: &str) -> usize {
        let value = self.required(key);
        value.parse().unwrap_or_else(|error| {
            panic!(
                "{} must declare numeric {key}: {error}",
                self.path.display()
            )
        })
    }

    /// Removes and parses a required `i32` value.
    pub fn required_i32(&mut self, key: &str) -> i32 {
        let value = self.required(key);
        value.parse().unwrap_or_else(|error| {
            panic!(
                "{} must declare numeric {key}: {error}",
                self.path.display()
            )
        })
    }

    /// Removes all prefixed entries and returns validated relative paths by suffix.
    pub fn required_prefixed_paths(&mut self, prefix: &str) -> BTreeMap<String, PathBuf> {
        let keys = self
            .values
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect::<Vec<_>>();
        let mut paths = BTreeMap::new();
        for key in keys {
            let name = key
                .strip_prefix(prefix)
                .expect("filtered manifest key prefix")
                .to_owned();
            assert!(
                !name.is_empty(),
                "{} contains an empty {prefix} name",
                self.path.display()
            );
            let value = self.required(&key);
            let path = fixture_relative_path(&self.path, value);
            assert!(
                paths.insert(name.clone(), path).is_none(),
                "{} contains duplicate {prefix} name {name}",
                self.path.display()
            );
        }
        paths
    }

    /// Asserts that every manifest entry was consumed.
    pub fn finish(&self) {
        assert!(
            self.values.is_empty(),
            "unknown case keys in {}: {:?}",
            self.path.display(),
            self.values.keys().collect::<Vec<_>>()
        );
    }
}

/// Returns sorted fixture directories, requiring each to contain `case.meta`.
pub fn case_directories(root: &Path, suite: &str) -> Vec<PathBuf> {
    let mut cases = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read {} cases: {error}", root.display()))
        .map(|entry| entry.expect("read case entry").path())
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "{suite} suite must contain cases");
    for case_root in &cases {
        assert!(
            case_root.is_dir(),
            "{suite} case {} must be a directory with case.meta",
            case_root.display()
        );
        assert!(
            case_root.join("case.meta").is_file(),
            "{suite} case {} must contain case.meta",
            case_root.display()
        );
    }
    cases
}

/// Copies a fixture tree while excluding generated build and cache directories.
pub fn copy_case_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination)
        .unwrap_or_else(|error| panic!("create {}: {error}", destination.display()));
    for entry in
        fs::read_dir(source).unwrap_or_else(|error| panic!("read {}: {error}", source.display()))
    {
        let entry = entry.expect("read fixture entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            if is_generated_case_state(entry.file_name().as_ref()) {
                continue;
            }
            copy_case_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).unwrap_or_else(|error| {
                panic!(
                    "copy fixture {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            });
        }
    }
}

fn is_generated_case_state(name: &OsStr) -> bool {
    name == OsStr::new(".nia-build") || name == OsStr::new(".nia-cache")
}

/// Validates a fixture-relative path with no absolute or parent components.
pub fn fixture_relative_path(manifest_path: &Path, value: String) -> PathBuf {
    let path = PathBuf::from(value);
    assert!(
        !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
        "{} must use a relative fixture path without parent components",
        manifest_path.display()
    );
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_case_tree_excludes_generated_build_and_cache_state() {
        let root = crate::test_dir("copy-case-tree-generated-state");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested/.nia-cache")).expect("create nested cache state");
        fs::create_dir_all(source.join(".nia-build")).expect("create build state");
        fs::write(source.join("nested/input.nia"), b"fn main() () {}")
            .expect("write ordinary fixture input");
        fs::write(source.join(".fixture-config"), b"kept").expect("write ordinary fixture dotfile");
        fs::write(source.join("nested/.nia-cache/stale"), b"cache")
            .expect("write cached fixture state");
        fs::write(source.join(".nia-build/output"), b"build").expect("write build fixture state");

        copy_case_tree(&source, &destination);

        assert!(destination.join("nested/input.nia").is_file());
        assert!(destination.join(".fixture-config").is_file());
        assert!(!destination.join("nested/.nia-cache").exists());
        assert!(!destination.join(".nia-build").exists());
    }
}
