use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::MaintainResult;

const REGISTRY: &str = "crates/nia-compat/src/lib.rs";
const DOMAIN_IDENTITY: &str =
    r"nia\.[a-z0-9]+(?:-[a-z0-9]+)*(?:\.[a-z0-9]+(?:-[a-z0-9]+)*)*\.v[1-9][0-9]*";
const FORBIDDEN_IDENTITY_NAMES: [&str; 13] = [
    "RESOURCE_LAYOUT_SCHEMA",
    "STD_SCHEMA",
    "BUILD_PROTOCOL_SCHEMA",
    "BUILD_PLAN_SCHEMA_VERSION",
    "MANGLE_ABI_VERSION",
    "CODEGEN_ABI_VERSION",
    "RUNNER_CONFIG_SCHEMA_VERSION",
    "RUNNER_CONFIG_MAGIC",
    "RUNNER_CONFIG_MAGIC_TEXT",
    "FRONTEND_CACHE_SCHEMA",
    "OBJECT_WORK_PRODUCT_SCHEMA",
    "LINK_RESULT_SCHEMA",
    "ARCHIVE_SCHEMA",
];
const TEXT_SUFFIXES: [&str; 12] = [
    "json", "md", "meta", "nia", "py", "rs", "sh", "toml", "txt", "yaml", "yml", "lock",
];
const VERSION_AUTHORITIES: [&str; 3] = ["Cargo.toml", "Cargo.lock", "lib/toolchain.meta"];

static DOMAIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^{DOMAIN_IDENTITY}$")).expect("valid domain regex"));
static STRING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""((?:\\.|[^"\\\r\n])*)""#).expect("valid string regex"));
static CONSTRUCTOR_DOMAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)(?:QueryFingerprintBuilder|Encoder)::new\s*\(\s*"((?:\\.|[^"\\\r\n])*)""#)
        .expect("valid constructor domain regex")
});
static TYPED_DOMAIN_DECLARATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?s)\bconst\s+([A-Z][A-Z0-9_]*)\s*:\s*FingerprintDomain\s*=\s*FingerprintDomain::new\s*\(\s*"({DOMAIN_IDENTITY})"\s*\)"#
    ))
    .expect("valid typed domain regex")
});
static BYTE_STRING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"b"((?:\\.|[^"\\\r\n])*)""#).expect("valid byte string regex"));
static TEST_MODULE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#\[cfg\(test\)\]\s*mod\s+tests\s*\{").expect("valid test module regex")
});
static FORBIDDEN_IDENTITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"\b(?:pub(?:\([^)]*\))?\s+)?const\s+({})\b",
        FORBIDDEN_IDENTITY_NAMES.join("|")
    ))
    .expect("valid forbidden identity regex")
});

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) -> MaintainResult<()> {
    let entries = fs::read_dir(root)
        .map_err(|error| format!("failed to read directory {}: {error}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "failed to inspect directory entry in {}: {error}",
                root.display()
            )
        })?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        let path = entry.path();
        if file_type.is_dir() {
            let name = entry.file_name();
            if matches!(
                name.to_str(),
                Some(".git" | "target" | "node_modules" | "__pycache__")
            ) {
                continue;
            }
            collect_files(&path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn source_files(root: &Path) -> MaintainResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn rust_sources(root: &Path) -> MaintainResult<Vec<PathBuf>> {
    let crates = root.join("crates");
    if !crates.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(&crates)
        .map_err(|error| format!("failed to read {}: {error}", crates.display()))?
    {
        let crate_path = entry
            .map_err(|error| format!("failed to inspect {}: {error}", crates.display()))?
            .path();
        let source = crate_path.join("src");
        if source.is_dir() {
            collect_files(&source, &mut files)?;
        }
    }
    files.retain(|path| path.extension().is_some_and(|extension| extension == "rs"));
    files.sort();
    Ok(files)
}

fn production_rust_sources(root: &Path) -> MaintainResult<Vec<PathBuf>> {
    Ok(rust_sources(root)?
        .into_iter()
        .filter(|path| {
            path.file_name().is_none_or(|name| name != "tests.rs")
                && !path.strip_prefix(root).is_ok_and(|relative| {
                    relative
                        .components()
                        .any(|part| part.as_os_str() == "tests")
                })
        })
        .collect())
}

fn production_source(path: &Path) -> MaintainResult<String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(TEST_MODULE.find(&source).map_or(source.clone(), |matched| {
        source[..matched.start()].to_owned()
    }))
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn decode_byte_string(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        index += 1;
        let escaped = *bytes.get(index)?;
        match escaped {
            b'\\' | b'"' | b'\'' => decoded.push(escaped),
            b'n' => decoded.push(b'\n'),
            b'r' => decoded.push(b'\r'),
            b't' => decoded.push(b'\t'),
            b'0' => decoded.push(0),
            b'x' => {
                let high = *bytes.get(index + 1)?;
                let low = *bytes.get(index + 2)?;
                let digit = |byte: u8| match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' => Some(byte - b'a' + 10),
                    b'A'..=b'F' => Some(byte - b'A' + 10),
                    _ => None,
                };
                decoded.push(digit(high)? * 16 + digit(low)?);
                index += 2;
            }
            _ => return None,
        }
        index += 1;
    }
    Some(decoded)
}

fn registered_magics(root: &Path) -> MaintainResult<BTreeSet<Vec<u8>>> {
    let registry = root.join(REGISTRY);
    let source = fs::read_to_string(&registry)
        .map_err(|error| format!("failed to read {}: {error}", registry.display()))?;
    Ok(BYTE_STRING
        .captures_iter(&source)
        .filter_map(|captures| decode_byte_string(&captures[1]))
        .filter(|value| value.starts_with(b"NIA") && value.len() == 8)
        .collect())
}

pub fn fingerprint_domain_errors(root: &Path) -> MaintainResult<Vec<String>> {
    let mut errors = Vec::new();
    for path in production_rust_sources(root)? {
        let source = production_source(&path)?;
        let path = relative(root, &path);
        let mut checked = BTreeSet::new();
        for captures in CONSTRUCTOR_DOMAIN.captures_iter(&source) {
            let matched = captures.get(1).expect("constructor domain capture");
            let domain = matched.as_str();
            checked.insert((matched.start(), domain.to_owned()));
            if !DOMAIN.is_match(domain) {
                errors.push(format!("{path}: invalid fingerprint domain `{domain}`"));
            }
        }
        for captures in STRING.captures_iter(&source) {
            let matched = captures.get(1).expect("string capture");
            let value = matched.as_str();
            if !value.starts_with("nia.") || value == "nia.compiler_builtins" {
                continue;
            }
            if !DOMAIN.is_match(value) && !checked.contains(&(matched.start(), value.to_owned())) {
                errors.push(format!("{path}: invalid Nia identity string `{value}`"));
            }
        }
    }
    Ok(errors)
}

pub fn typed_fingerprint_domain_errors(root: &Path) -> MaintainResult<Vec<String>> {
    let mut errors = Vec::new();
    let mut declarations = BTreeMap::<String, Vec<String>>::new();
    for path in production_rust_sources(root)? {
        let source = production_source(&path)?;
        let path = relative(root, &path);
        let mut declaration_spans = Vec::new();
        for captures in TYPED_DOMAIN_DECLARATION.captures_iter(&source) {
            let domain = captures.get(2).expect("typed domain capture");
            declaration_spans.push(domain.start()..domain.end());
            let line = source[..captures.get(0).expect("declaration capture").start()]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            declarations
                .entry(domain.as_str().to_owned())
                .or_default()
                .push(format!("{path}:{line}"));
        }
        for captures in CONSTRUCTOR_DOMAIN.captures_iter(&source) {
            errors.push(format!(
                "{path}: fingerprint builder receives raw literal `{}`",
                &captures[1]
            ));
        }
        for captures in STRING.captures_iter(&source) {
            let matched = captures.get(1).expect("string capture");
            let value = matched.as_str();
            if DOMAIN.is_match(value)
                && !declaration_spans
                    .iter()
                    .any(|span| span.contains(&matched.start()))
            {
                errors.push(format!(
                    "{path}: fingerprint domain `{value}` is not an owner-local typed constant"
                ));
            }
        }
    }
    for (domain, owners) in declarations {
        if owners.len() > 1 {
            errors.push(format!(
                "duplicate fingerprint domain `{domain}` declared at {}",
                owners.join(", ")
            ));
        }
    }
    Ok(errors)
}

pub fn global_identity_errors(root: &Path) -> MaintainResult<Vec<String>> {
    let mut errors = Vec::new();
    let registry = root.join(REGISTRY);
    let magics = registered_magics(root)?;
    for path in rust_sources(root)? {
        if path == registry {
            continue;
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let relative = relative(root, &path);
        for captures in BYTE_STRING.captures_iter(&source) {
            if let Some(value) = decode_byte_string(&captures[1])
                && value.starts_with(b"NIA")
                && magics.contains(&value)
            {
                errors.push(format!(
                    "{relative}: registered magic is defined outside nia-compat"
                ));
            }
        }
        for captures in FORBIDDEN_IDENTITY.captures_iter(&source) {
            errors.push(format!(
                "{relative}: obsolete compatibility identity `{}`",
                &captures[1]
            ));
        }
        if source.contains("CARGO_PKG_VERSION") {
            errors.push(format!(
                "{relative}: release version is read outside nia-compat"
            ));
        }
    }
    Ok(errors)
}

fn workspace_version(root: &Path) -> MaintainResult<String> {
    let path = root.join("Cargo.toml");
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let manifest: toml::Value = toml::from_str(&source)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Cargo.toml has no workspace.package.version".to_owned())
}

pub fn release_version_errors(root: &Path) -> MaintainResult<Vec<String>> {
    let version = workspace_version(root)?;
    let authorities = VERSION_AUTHORITIES.into_iter().collect::<BTreeSet<_>>();
    let suffixes = TEXT_SUFFIXES.into_iter().collect::<BTreeSet<_>>();
    let mut errors = Vec::new();
    for path in source_files(root)? {
        let relative = relative(root, &path);
        let suffix = path.extension().and_then(|value| value.to_str());
        if authorities.contains(relative.as_str()) || !suffix.is_some_and(|s| suffixes.contains(s))
        {
            continue;
        }
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        if source.contains(&version) {
            errors.push(format!(
                "{relative}: workspace version `{version}` is duplicated"
            ));
        }
    }
    Ok(errors)
}

pub fn audit(root: &Path) -> MaintainResult<Vec<String>> {
    let mut errors = fingerprint_domain_errors(root)?;
    errors.extend(typed_fingerprint_domain_errors(root)?);
    errors.extend(global_identity_errors(root)?);
    errors.extend(release_version_errors(root)?);
    Ok(errors)
}

pub fn run(root: &Path) -> MaintainResult<()> {
    let errors = audit(root)?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "compatibility audit failed:\n{}",
            errors.join("\n")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDirectory;

    fn repository(name: &str) -> TestDirectory {
        let directory = TestDirectory::new(name);
        directory.write(
            "Cargo.toml",
            "[workspace]\nmembers = []\n[workspace.package]\nversion = \"1.2.3\"\n",
        );
        directory.write(
            "crates/nia-compat/src/lib.rs",
            "pub const FORMAT: &[u8; 8] = b\"NIAFMT01\";\n",
        );
        directory.write("crates/owner/src/lib.rs", "");
        directory
    }

    #[test]
    fn accepts_versioned_fingerprint_domains() {
        let directory = repository("valid-domain");
        directory.write(
            "crates/owner/src/lib.rs",
            "const PRODUCT_DOMAIN: FingerprintDomain =\n    FingerprintDomain::new(\"nia.owner.product.v2\");\nQueryFingerprintBuilder::new(PRODUCT_DOMAIN);\n",
        );
        assert!(
            fingerprint_domain_errors(directory.path())
                .unwrap()
                .is_empty()
        );
        assert!(
            typed_fingerprint_domain_errors(directory.path())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rejects_unversioned_and_raw_constructor_domains() {
        let directory = repository("raw-domain");
        directory.write(
            "crates/owner/src/lib.rs",
            "QueryFingerprintBuilder::new(\"nia.owner.product.v2\");\nQueryFingerprintBuilder::new(\"owner-product\");\n",
        );
        assert!(fingerprint_domain_errors(directory.path()).unwrap()[0].contains("owner-product"));
        assert!(
            typed_fingerprint_domain_errors(directory.path()).unwrap()[0].contains("raw literal")
        );
    }

    #[test]
    fn rejects_malformed_and_duplicate_typed_domains() {
        let directory = repository("duplicate-domain");
        directory.write(
            "crates/owner/src/lib.rs",
            "const BAD_DOMAIN: FingerprintDomain = FingerprintDomain::new(\"nia.owner..product.v2\");\nconst FIRST_DOMAIN: FingerprintDomain = FingerprintDomain::new(\"nia.owner.product.v2\");\nconst SECOND_DOMAIN: FingerprintDomain = FingerprintDomain::new(\"nia.owner.product.v2\");\n",
        );
        assert!(fingerprint_domain_errors(directory.path()).unwrap()[0].contains("owner..product"));
        assert!(
            typed_fingerprint_domain_errors(directory.path())
                .unwrap()
                .iter()
                .any(|error| error.contains("duplicate"))
        );
    }

    #[test]
    fn rejects_registered_magic_outside_registry() {
        let directory = repository("duplicate-magic");
        directory.write(
            "crates/owner/src/lib.rs",
            "const MAGIC: &[u8; 8] = b\"NIAFMT01\";\n",
        );
        assert!(
            global_identity_errors(directory.path()).unwrap()[0].contains("outside nia-compat")
        );
    }

    #[test]
    fn rejects_workspace_version_outside_authorities() {
        let directory = repository("duplicate-version");
        directory.write("README.md", "current version: 1.2.3\n");
        assert!(release_version_errors(directory.path()).unwrap()[0].contains("README.md"));
    }
}
