// SPDX-License-Identifier: GPL-3.0-or-later
//! Rejects addresses of a function's own storage that outlive its call.
//!
//! `nia_escape_check` finds the escapes; this module names the storage and
//! explains each one. Only direct data flow is followed, so every diagnostic
//! is a definite escape: the pointer reaches a return, a `static`, or memory
//! the caller owns while pointing into a local, a by-value parameter, or a
//! temporary of the returning call.

use crate::BodyChecker;
use nia_diagnostic::{Diagnostic, codes};
use nia_escape_check::{StackEscapeSink, StackStorage};
use nia_local_resolve::{LocalBindingName, LocalKind};

/// How a diagnostic refers to the storage an escaping address points into.
enum Storage {
    Local(String),
    Parameter(String),
    Temporary,
    StringLiteral,
}

impl Storage {
    /// The storage as the object of a sentence.
    fn phrase(&self) -> String {
        match self {
            Storage::Local(name) => format!("local `{name}`"),
            Storage::Parameter(name) => format!("parameter `{name}`"),
            Storage::Temporary => "a temporary".to_owned(),
            Storage::StringLiteral => "a string literal".to_owned(),
        }
    }

    /// The storage as the subject of the address-site label.
    fn subject(&self) -> String {
        match self {
            Storage::Local(name) | Storage::Parameter(name) => format!("`{name}`"),
            Storage::Temporary => "the temporary".to_owned(),
            Storage::StringLiteral => "the string literal".to_owned(),
        }
    }

    fn lifetime_note(&self) -> String {
        match self {
            Storage::Local(name) | Storage::Parameter(name) => format!(
                "`{name}` lives in this call's frame, which is reclaimed when the call returns"
            ),
            Storage::Temporary => {
                "a temporary lives only until the end of its enclosing block".to_owned()
            }
            Storage::StringLiteral => {
                "a string literal is an array value; evaluated in a function, it has no static storage"
                    .to_owned()
            }
        }
    }

    fn help(&self) -> &'static str {
        match self {
            Storage::Local(_) | Storage::Parameter(_) => {
                "return or store the value itself, or keep it in storage the caller provides"
            }
            Storage::Temporary => {
                "keep the value in storage that outlives the call, such as the caller's or a `static`"
            }
            Storage::StringLiteral => "declare the text as a `static` and point to that",
        }
    }
}

impl BodyChecker<'_> {
    /// Reports every address of `body`'s own storage that outlives the call.
    pub(super) fn check_stack_address_escapes(&mut self, body: &nia_body_ir::TypedBody) {
        let escapes = match nia_escape_check::stack_address_escapes(body, self.type_store) {
            Ok(escapes) => escapes,
            Err(error) => {
                self.record_internal(error.with_context("checking stack address escapes"));
                return;
            }
        };
        for escape in escapes {
            let storage = self.escaping_storage(escape.storage);
            let what = storage.phrase();
            let (summary, action) = match escape.sink {
                StackEscapeSink::Return => (
                    format!("returned value points to {what}, which does not outlive this call"),
                    "returned",
                ),
                StackEscapeSink::Static => (
                    format!(
                        "a `static` is given a pointer to {what}, which does not outlive this call"
                    ),
                    "stored in a `static`",
                ),
                StackEscapeSink::CallerMemory => (
                    format!(
                        "memory the caller owns is given a pointer to {what}, which does not outlive this call"
                    ),
                    "stored through a parameter",
                ),
            };
            let subject = storage.subject();
            let diagnostic = Diagnostic::user_error(codes::TYPE_CHECK, summary);
            // An address taken implicitly where it escapes has one span.
            let diagnostic = if escape.site == escape.span {
                diagnostic.primary(
                    escape.span,
                    format!("{subject} is addressed and {action} here"),
                )
            } else {
                diagnostic
                    .primary(escape.span, format!("the pointer is {action} here"))
                    .secondary(escape.site, format!("{subject} is addressed here"))
            };
            self.diagnostics.push(
                diagnostic
                    .note(storage.lifetime_note())
                    .help(storage.help())
                    .finish(),
            );
        }
    }

    fn escaping_storage(&self, storage: StackStorage) -> Storage {
        match storage {
            StackStorage::Local(local_id) => match self.locals.locals.get(local_id) {
                Some(local) => {
                    let name = match local.name {
                        LocalBindingName::Named(symbol) => self.symbol_name(symbol),
                        LocalBindingName::SelfValue => "self".to_owned(),
                    };
                    if local.kind == LocalKind::Param {
                        Storage::Parameter(name)
                    } else {
                        Storage::Local(name)
                    }
                }
                // Locals the source does not name are lowering temporaries.
                None => Storage::Temporary,
            },
            StackStorage::Temporary => Storage::Temporary,
            StackStorage::StringLiteral => Storage::StringLiteral,
        }
    }
}
