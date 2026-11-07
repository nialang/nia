// SPDX-License-Identifier: GPL-3.0-or-later
//! Build protocol names and the stricter contract for CLI-addressable steps.

use std::fmt;

/// A name is not a nonempty build-protocol identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StableNameError {
    /// The name has no bytes.
    Empty,
    /// A character is outside the ASCII stable-name alphabet.
    InvalidCharacter {
        /// Byte offset of the character.
        index: usize,
        /// Rejected character.
        character: char,
    },
}

impl fmt::Display for StableNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("stable name is empty"),
            Self::InvalidCharacter { index, character } => write!(
                f,
                "stable name contains invalid character `{character}` at byte {index}"
            ),
        }
    }
}

impl std::error::Error for StableNameError {}

/// Validates an identifier without imposing user-facing step restrictions.
pub fn validate_stable_name(name: &str) -> Result<(), StableNameError> {
    if name.is_empty() {
        return Err(StableNameError::Empty);
    }
    for (index, character) in name.char_indices() {
        if !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')) {
            return Err(StableNameError::InvalidCharacter { index, character });
        }
    }
    Ok(())
}

/// A stable name cannot be selected unambiguously as a CLI build step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicStepNameError {
    /// The name is not a stable identifier.
    StableName(StableNameError),
    /// Leading hyphens are reserved for command-line options.
    LeadingHyphen,
    /// The name is a directory navigation component.
    PathComponent,
}

impl fmt::Display for PublicStepNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StableName(error) => error.fmt(f),
            Self::LeadingHyphen => f.write_str("public step names must not start with `-`"),
            Self::PathComponent => f.write_str("public step names must not be `.` or `..`"),
        }
    }
}

impl std::error::Error for PublicStepNameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StableName(error) => Some(error),
            _ => None,
        }
    }
}

/// Validates a publicly selectable step, independently of CLI separators.
pub fn validate_public_step_name(name: &str) -> Result<(), PublicStepNameError> {
    validate_stable_name(name).map_err(PublicStepNameError::StableName)?;
    if name.starts_with('-') {
        return Err(PublicStepNameError::LeadingHyphen);
    }
    if matches!(name, "." | "..") {
        return Err(PublicStepNameError::PathComponent);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_step_rules_do_not_restrict_internal_identifiers() {
        for name in ["-", "--help", "-task", ".", ".."] {
            assert!(validate_stable_name(name).is_ok(), "{name}");
            assert!(validate_public_step_name(name).is_err(), "{name}");
        }
        for name in [
            "build",
            "run-app",
            "unit.compile",
            "_",
            ".hidden",
            "0",
            "...",
        ] {
            assert!(validate_public_step_name(name).is_ok(), "{name}");
        }
        for name in ["", "a/b", "a\\b", "a b", "a:b", "中文", "a\0b"] {
            assert!(validate_stable_name(name).is_err(), "{name}");
            assert!(validate_public_step_name(name).is_err(), "{name}");
        }
    }
}
