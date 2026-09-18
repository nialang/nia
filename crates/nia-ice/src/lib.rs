// SPDX-License-Identifier: GPL-3.0-or-later
//! Structured internal compiler errors and last-resort panic isolation.

use std::any::Any;
use std::cell::RefCell;
use std::fmt;
use std::panic::{AssertUnwindSafe, Location, catch_unwind};
use std::sync::Once;

/// Describes how an internal compiler error entered the error pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IceOrigin {
    /// Compiler code detected and reported a violated invariant explicitly.
    Invariant,
    /// A Rust panic escaped code that could not report a structured failure.
    UnexpectedPanic,
}

/// Structured internal compiler error with propagation context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ice {
    /// Human-readable invariant or panic message.
    pub message: String,
    /// Source location where the failure was detected.
    pub location: Option<String>,
    /// High-level compiler operations traversed while propagating the error.
    pub contexts: Vec<String>,
    /// Whether the failure was explicit or recovered from an unexpected panic.
    pub origin: IceOrigin,
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
            origin: IceOrigin::Invariant,
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

    fn from_panic(payload: Box<dyn Any + Send>, metadata: Option<PanicMetadata>) -> Self {
        Self {
            message: panic_payload_message(payload.as_ref()),
            location: metadata.and_then(|metadata| metadata.location),
            contexts: Vec::new(),
            origin: IceOrigin::UnexpectedPanic,
        }
    }
}

impl fmt::Display for Ice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render_summary())
    }
}

impl std::error::Error for Ice {}

/// Runs a last-resort panic boundary and converts an unexpected unwind into an [`Ice`].
///
/// Compiler operations should return structured errors directly. This function is reserved for
/// thread, process, FFI, and third-party boundaries where an unknown Rust panic must not escape.
pub fn catch_unexpected_panic<T>(f: impl FnOnce() -> T) -> IceResult<T> {
    install_panic_hook();
    let capture = PanicCaptureGuard::enter();
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => {
            capture.finish();
            Ok(value)
        }
        Err(payload) => Err(Ice::from_panic(payload, capture.finish())),
    }
}

/// Installs the process hook used to enrich [`catch_unexpected_panic`] failures.
///
/// Panics outside an active Nia boundary are forwarded to the previously installed hook. Calling
/// this function more than once is harmless.
pub fn install_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let captured = PANIC_CAPTURES
                .try_with(|captures| {
                    let mut captures = captures.borrow_mut();
                    let Some(capture) = captures.last_mut() else {
                        return false;
                    };
                    capture.location = info.location().map(|location| {
                        format_location(location.file(), location.line(), location.column())
                    });
                    true
                })
                .unwrap_or(false);
            if !captured {
                previous(info);
            }
        }));
    });
}

#[derive(Default)]
struct PanicMetadata {
    location: Option<String>,
}

thread_local! {
    static PANIC_CAPTURES: RefCell<Vec<PanicMetadata>> = const { RefCell::new(Vec::new()) };
}

struct PanicCaptureGuard {
    active: bool,
}

impl PanicCaptureGuard {
    fn enter() -> Self {
        PANIC_CAPTURES.with(|captures| captures.borrow_mut().push(PanicMetadata::default()));
        Self { active: true }
    }

    fn finish(mut self) -> Option<PanicMetadata> {
        self.active = false;
        PANIC_CAPTURES.with(|captures| captures.borrow_mut().pop())
    }
}

impl Drop for PanicCaptureGuard {
    fn drop(&mut self) {
        if self.active {
            PANIC_CAPTURES.with(|captures| {
                captures.borrow_mut().pop();
            });
        }
    }
}

fn format_location(file: &str, line: u32, column: u32) -> String {
    format!("{file}:{line}:{column}")
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return clean_panic_message(message);
    }
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return clean_panic_message(message);
    }
    "compiler panicked with non-string payload".to_string()
}

fn clean_panic_message(message: &str) -> String {
    message
        .strip_prefix("Nia ICE: ")
        .or_else(|| message.strip_prefix("Nia ICE (LLVM): "))
        .unwrap_or(message)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catches_string_panics_as_unexpected_ice() {
        let err = catch_unexpected_panic(|| panic!("Nia ICE: broken invariant")).unwrap_err();
        assert_eq!(err.message, "broken invariant");
        assert_eq!(err.origin, IceOrigin::UnexpectedPanic);
    }

    #[test]
    fn normalizes_llvm_prefix_and_non_string_payloads() {
        let llvm = catch_unexpected_panic(|| panic!("Nia ICE (LLVM): null handle")).unwrap_err();
        assert_eq!(llvm.message, "null handle");

        let non_string = catch_unexpected_panic(|| std::panic::panic_any(42_u32)).unwrap_err();
        assert_eq!(
            non_string.message,
            "compiler panicked with non-string payload"
        );
    }

    #[test]
    fn explicit_ice_records_location_and_context() {
        let ice = Ice::new("failed invariant").with_context("lowering module `main`");

        assert!(ice.location.is_some());
        assert_eq!(ice.contexts, ["lowering module `main`"]);
        assert_eq!(ice.origin, IceOrigin::Invariant);
        assert!(ice.render_message().contains("compiler context"));
    }

    #[test]
    fn records_panic_location_when_hook_is_installed() {
        install_panic_hook();

        let err = catch_unexpected_panic(|| panic!("Nia ICE: broken invariant")).unwrap_err();

        assert!(err.location.is_some(), "{err:?}");
    }

    #[test]
    fn nested_boundaries_keep_their_own_panic_metadata() {
        install_panic_hook();

        let outer = catch_unexpected_panic(|| {
            let inner = catch_unexpected_panic(|| panic!("inner")).unwrap_err();
            assert!(inner.location.is_some());
            panic!("outer")
        })
        .unwrap_err();

        assert!(outer.location.is_some());
        assert_eq!(outer.message, "outer");
    }
}
