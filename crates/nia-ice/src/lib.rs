// SPDX-License-Identifier: GPL-3.0-or-later
//! Structured internal compiler errors.

use std::fmt;
use std::panic::Location;

/// Structured internal compiler error with propagation context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ice {
    /// Human-readable invariant or panic message.
    pub message: String,
    /// Source location where the failure was detected.
    pub location: Option<String>,
    /// High-level compiler operations traversed while propagating the error.
    pub contexts: Vec<String>,
}

/// Result alias for operations that may surface an [`Ice`].
pub type IceResult<T> = Result<T, Ice>;

impl Ice {
    /// Creates an explicit ICE at the caller that detected the failed invariant.
    #[track_caller]
    pub fn new(message: impl Into<String>) -> Self {
        let caller = Location::caller();
        Self {
            message: message.into(),
            location: Some(format_location(
                caller.file(),
                caller.line(),
                caller.column(),
            )),
            contexts: Vec::new(),
        }
    }

    /// Attaches or replaces the source location and returns the error.
    pub fn with_location(mut self, location: Option<String>) -> Self {
        self.location = location;
        self
    }

    /// Adds an operation that was active while this ICE propagated.
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        let context = context.into();
        if self.contexts.last() != Some(&context) {
            self.contexts.push(context);
        }
        self
    }

    /// Renders the short summary used by diagnostics and logs.
    pub fn render_summary(&self) -> String {
        format!("internal compiler error: {}", self.message)
    }

    /// Renders an actionable multi-line message for users and bug reports.
    pub fn render_message(&self) -> String {
        let mut rendered = self.render_summary();
        if let Some(location) = &self.location {
            rendered.push_str(&format!("\ncompiler failure location: {location}"));
        }
        if !self.contexts.is_empty() {
            rendered.push_str("\ncompiler context:");
            for context in self.contexts.iter().rev() {
                rendered.push_str("\n  ");
                rendered.push_str(context);
            }
        }
        rendered.push_str(
            "\n\nThis is a compiler bug. Please report it with the source file and command that triggered it.",
        );
        rendered
    }
}

impl fmt::Display for Ice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render_summary())
    }
}

impl std::error::Error for Ice {}

fn format_location(file: &str, line: u32, column: u32) -> String {
    format!("{file}:{line}:{column}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_ice_records_location_and_context() {
        let ice = Ice::new("failed invariant").with_context("lowering module `main`");

        assert!(ice.location.is_some());
        assert_eq!(ice.contexts, ["lowering module `main`"]);
        assert!(ice.render_message().contains("compiler context"));
    }
}
