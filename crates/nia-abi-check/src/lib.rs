// SPDX-License-Identifier: GPL-3.0-or-later
use nia_defs::{DefCollection, DefId};
use nia_diagnostic::Diagnostic;
use nia_item_signatures::{FunctionSignature, ItemSignatures};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};

#[derive(Debug, Clone, PartialEq)]
pub struct AbiCheck {
    pub diagnostics: Vec<Diagnostic>,
}

pub fn check_module_abi(
    defs: &DefCollection,
    interner: &TyInterner,
    signatures: &ItemSignatures,
) -> AbiCheck {
    let mut checker = AbiChecker {
        defs,
        interner,
        signatures,
        diagnostics: Vec::new(),
    };
    checker.check();
    AbiCheck {
        diagnostics: checker.diagnostics,
    }
}

struct AbiChecker<'a> {
    defs: &'a DefCollection,
    interner: &'a TyInterner,
    signatures: &'a ItemSignatures,
    diagnostics: Vec<Diagnostic>,
}

impl AbiChecker<'_> {
    fn check(&mut self) {
        for (def_id, signature) in &self.signatures.functions {
            if signature.is_extern {
                self.check_extern_function(*def_id, signature);
            }
        }
        for signature in self.signatures.globals.values() {
            if signature.is_extern {
                self.check_extern_global(signature);
            }
        }
    }

    fn check_extern_global(&mut self, signature: &nia_item_signatures::GlobalSignature) {
        let Some(ty) = signature.explicit_type else {
            self.diagnostics.push(Diagnostic::error(
                signature.span,
                "extern global requires an explicit type",
            ));
            return;
        };
        self.check_extern_ty(signature.span, ty, "extern global");
    }

    fn check_extern_function(&mut self, def_id: DefId, signature: &FunctionSignature) {
        if !signature.generics.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                signature.span,
                "extern function cannot have generic parameters",
            ));
        }
        if signature.is_variadic && signature.params.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                signature.span,
                "extern variadic function requires at least one fixed parameter",
            ));
        }
        if signature.is_variadic && signature.has_body {
            self.diagnostics.push(Diagnostic::error(
                signature.span,
                "extern variadic function definition is not supported",
            ));
        }
        for param in &signature.params {
            self.check_extern_ty(param.span, param.ty, "extern parameter");
        }
        if !self.is_void_or_never(signature.return_type) {
            self.check_extern_ty(signature.span, signature.return_type, "extern return type");
        }
        if let Some(def) = self.defs.defs.get(def_id)
            && def.parent.is_some()
        {
            self.diagnostics.push(Diagnostic::error(
                signature.span,
                "extern method ABI is not defined",
            ));
        }
    }

    fn check_extern_ty(&mut self, span: Span, ty: nia_ids::TyId, context: &str) {
        match self.interner.get(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Bool)) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use `bool` directly"),
            )),
            Some(TyKind::Primitive(PrimitiveTy::Char)) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use `char` directly"),
            )),
            Some(TyKind::Primitive(_)) | Some(TyKind::Pointer { .. }) => {}
            Some(TyKind::Slice { .. }) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use nia slice directly"),
            )),
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                if *is_variadic {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!("{context} cannot use variadic function pointer"),
                    ));
                }
                for param in params {
                    self.check_extern_ty(span, *param, "extern function pointer parameter");
                }
                if !self.is_void_or_never(*return_type) {
                    self.check_extern_ty(span, *return_type, "extern function pointer return type");
                }
            }
            Some(TyKind::Array { .. }) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use array by value"),
            )),
            Some(TyKind::Nominal { def_id, .. }) => {
                if def_id.module_id != self.defs.module_id {
                    return;
                }
                if self.signatures.enums.contains_key(&def_id.def_id) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!("{context} cannot use enum directly; use its backing integer type"),
                    ));
                }
            }
            Some(TyKind::GenericParam(_)) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use generic parameter"),
            )),
            Some(TyKind::Error) | None => {}
        }
    }

    fn is_void_or_never(&self, ty: nia_ids::TyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::Void | PrimitiveTy::Never))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_item_signatures::collect_item_signatures;
    use nia_parser::parse_module;
    use nia_type_lower::lower_module_types_with_id;
    use nia_type_resolve::resolve_module_types;

    #[test]
    fn rejects_undefined_extern_abi_types() {
        let (module, errors) = parse_module(
            r#"
enum Color: u8 { Red }

extern fn bad(flag: bool, ch: char, color: Color, xs: [2]u8, cb: &const fn(i32, ...) void, ...);
extern fn bad_variadic_definition(fmt: &u8, ...) {
}
extern const bad_global: bool;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        let lowered = lower_module_types_with_id(ModuleId(0), &module, &resolved);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        let checked = check_module_abi(&defs, &lowered.interner, &signatures);
        for expected in [
            "`bool`",
            "`char`",
            "enum directly",
            "array by value",
            "variadic function pointer",
            "variadic function definition",
        ] {
            assert!(
                checked
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(expected)),
                "{expected}: {:?}",
                checked.diagnostics
            );
        }
    }
}
