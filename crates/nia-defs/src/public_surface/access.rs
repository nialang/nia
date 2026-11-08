// SPDX-License-Identifier: GPL-3.0-or-later
//! Structured export access results shared by reporting and speculative lookup.

use super::ExportScope;
use nia_ids::{AccessRestriction, Visibility};

/// An exported name may be absent, accessible, or present behind a restriction.
/// Silent probes deliberately project this product to an Option; reporting
/// resolvers retain the denial until all candidate namespaces are exhausted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportLookup<T> {
    /// No such name was exported in this namespace.
    Missing,
    /// At least one complete route permits access.
    Visible(T),
    /// The name exists, but every route has a failed restriction.
    Inaccessible(ExportDenial),
}

impl<T> ExportLookup<T> {
    /// Applies export access to an owned or borrowed target. Missing targets do
    /// not evaluate the scope projection.
    pub fn with_access(
        target: Option<T>,
        scope: impl FnOnce() -> ExportScope,
        allows: &dyn Fn(nia_ids::AccessRestriction) -> bool,
    ) -> Self {
        let Some(target) = target else {
            return Self::Missing;
        };
        match scope().check_access(allows) {
            Ok(()) => Self::Visible(target),
            Err(denial) => Self::Inaccessible(denial),
        }
    }

    /// Drops denial information for a speculative lookup.
    pub fn into_visible(self) -> Option<T> {
        match self {
            Self::Visible(target) => Some(target),
            _ => None,
        }
    }

    /// Access evidence for a reporting lookup that found no visible candidate.
    pub fn denial(&self) -> Option<&ExportDenial> {
        match self {
            Self::Inaccessible(denial) => Some(denial),
            _ => None,
        }
    }
}

/// Failed restrictions on every alternative export route. A route that passes
/// every restriction prevents a denial from being constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDenial {
    pub(super) blocked_routes: Vec<Vec<AccessRestriction>>,
}

impl ExportDenial {
    /// Explains a failed lookup without confusing access denial with absence.
    pub fn diagnostic(
        &self,
        kind: &str,
        name: &str,
        span: nia_span::Span,
    ) -> nia_diagnostic::Diagnostic {
        use nia_diagnostic::{Diagnostic, codes};

        let mut diagnostic = Diagnostic::user_error(codes::NAME_RESOLUTION,
            format!("{kind} `{name}` is restricted by its export path"))
            .primary(span, "this name exists, but no export path permits access from this module")
            .help("use this name from an allowed scope or change the restricting declaration or re-export");
        for (index, route) in self.blocked_routes.iter().enumerate() {
            let mut reasons = Vec::new();
            for restriction in route {
                let reason = match restriction {
                    AccessRestriction::Item(_, Visibility::Private) => {
                        "a private item or import is visible only in its declaring module"
                    }
                    AccessRestriction::Module(_, Visibility::Private) => {
                        "a private module namespace is visible only in its declaring module and descendants"
                    }
                    AccessRestriction::Item(_, Visibility::PublicSuper)
                    | AccessRestriction::Module(_, Visibility::PublicSuper) => {
                        "pub(super) restricts access to its parent module and descendants"
                    }
                    AccessRestriction::Item(_, Visibility::PublicRoot)
                    | AccessRestriction::Module(_, Visibility::PublicRoot) => {
                        "pub(root) restricts access to its module tree"
                    }
                    AccessRestriction::Item(_, Visibility::Public)
                    | AccessRestriction::Module(_, Visibility::Public) => continue,
                };
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
            }
            diagnostic =
                diagnostic.note(format!("export path {}: {}", index + 1, reasons.join("; ")));
        }
        diagnostic.finish()
    }
}
