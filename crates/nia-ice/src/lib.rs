// SPDX-License-Identifier: GPL-3.0-or-later
use std::any::Any;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use nia_diagnostic::Diagnostic;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ice {
    pub message: String,
    pub location: Option<String>,
}

pub type IceResult<T> = Result<T, Ice>;

impl Ice {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            location: None,
        }
    }

    pub fn with_location(mut self, location: Option<String>) -> Self {
        self.location = location;
        self
    }

    pub fn diagnostic(&self) -> Diagnostic {
        let mut diagnostic =
            Diagnostic::internal_error(nia_diagnostic::codes::ICE, self.render_summary())
            .note("this is a compiler bug; please report it with the source file and command that triggered it");
        if let Some(location) = &self.location {
            diagnostic = diagnostic.debug("panic_location", location);
        }
        diagnostic.finish()
    }

    pub fn render_summary(&self) -> String {
        format!("internal compiler error: {}", self.message)
    }

    pub fn render_message(&self) -> String {
        let mut rendered = self.render_summary();
        if let Some(location) = &self.location {
            rendered.push_str(&format!("\ncompiler panic location: {location}"));
        }
        rendered.push_str(
            "\n\nThis is a compiler bug. Please report it with the source file and command that triggered it.",
        );
        rendered
    }
}

impl From<Ice> for Diagnostic {
    fn from(ice: Ice) -> Self {
        ice.diagnostic()
    }
}

pub fn catch_ice<T>(f: impl FnOnce() -> T) -> Result<T, Ice> {
    take_panic_location();
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        let location = take_panic_location();
        ice_from_panic(payload).with_location(location)
    })
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if let Some(location) = info.location() {
            LAST_PANIC_LOCATION.with(|slot| {
                *slot.borrow_mut() = Some(format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                ));
            });
        }
    }));
}

fn ice_from_panic(payload: Box<dyn Any + Send>) -> Ice {
    Ice::new(panic_payload_message(payload.as_ref()))
}

thread_local! {
    static LAST_PANIC_LOCATION: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn take_panic_location() -> Option<String> {
    LAST_PANIC_LOCATION.with(|slot| slot.borrow_mut().take())
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
    use nia_diagnostic::DiagnosticCategory;

    #[test]
    fn catches_string_panics_as_ice() {
        let err = catch_ice(|| panic!("Nia ICE: broken invariant")).unwrap_err();
        assert_eq!(err.message, "broken invariant");
    }

    #[test]
    fn renders_actionable_message() {
        let message = Ice::new("failed invariant").render_message();
        assert!(message.contains("internal compiler error"));
        assert!(message.contains("Please report it"));
    }

    #[test]
    fn converts_to_internal_diagnostic() {
        let diagnostic = Ice::new("failed invariant")
            .with_location(Some("main.rs:1:2".to_string()))
            .diagnostic();

        assert_eq!(diagnostic.category, DiagnosticCategory::Internal);
        assert_eq!(diagnostic.code.as_str(), "I0001");
        assert!(diagnostic.summary.contains("failed invariant"));
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|note| note.contains("compiler bug"))
        );
        assert!(
            diagnostic
                .debug
                .iter()
                .any(|field| field.key == "panic_location")
        );
    }

    #[test]
    fn records_panic_location_when_hook_is_installed() {
        install_panic_hook();

        let err = catch_ice(|| panic!("Nia ICE: broken invariant")).unwrap_err();

        assert!(err.location.is_some(), "{err:?}");
    }
}
