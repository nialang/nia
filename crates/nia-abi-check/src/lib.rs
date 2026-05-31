// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_defs::{DefCollection, DefId};
use nia_diagnostic::Diagnostic;
use nia_ids::GlobalDefId;
use nia_item_signatures::{FunctionSignature, ItemSignatures, StructSignature, UnionSignature};
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyInterner, TyKind};

#[derive(Debug, Clone, Copy)]
pub struct ProgramAbiSignatures<'a> {
    pub structs: &'a HashMap<GlobalDefId, StructSignature>,
    pub unions: &'a HashMap<GlobalDefId, UnionSignature>,
    pub enums: &'a HashMap<GlobalDefId, nia_item_signatures::EnumSignature>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AbiCheck {
    pub diagnostics: Vec<Diagnostic>,
}

pub fn check_module_abi(
    defs: &DefCollection,
    interner: &TyInterner,
    signatures: &ItemSignatures,
) -> AbiCheck {
    let empty_structs = HashMap::new();
    let empty_unions = HashMap::new();
    let empty_enums = HashMap::new();
    check_module_abi_with_program_signatures(
        defs,
        interner,
        signatures,
        ProgramAbiSignatures {
            structs: &empty_structs,
            unions: &empty_unions,
            enums: &empty_enums,
        },
    )
}

pub fn check_module_abi_with_program_signatures(
    defs: &DefCollection,
    interner: &TyInterner,
    signatures: &ItemSignatures,
    program_signatures: ProgramAbiSignatures<'_>,
) -> AbiCheck {
    let mut checker = AbiChecker {
        defs,
        interner,
        signatures,
        program_signatures,
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
    program_signatures: ProgramAbiSignatures<'a>,
    diagnostics: Vec<Diagnostic>,
}

impl AbiChecker<'_> {
    fn check(&mut self) {
        for (def_id, signature) in &self.signatures.functions {
            if signature.is_extern {
                self.check_extern_function(*def_id, signature);
            }
        }
        for signature in self.signatures.structs.values() {
            if signature.is_extern {
                self.check_extern_struct(signature);
            }
        }
        for signature in self.signatures.globals.values() {
            if signature.is_extern {
                self.check_extern_global(signature);
            }
        }
    }

    fn check_extern_struct(&mut self, signature: &nia_item_signatures::StructSignature) {
        for field in &signature.fields {
            self.check_extern_ty(field.span, field.ty, "extern struct field");
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
        if self.is_never(signature.return_type) {
            self.diagnostics.push(Diagnostic::error(
                signature.span,
                "extern return type cannot use `!`",
            ));
        } else if !self.is_void(signature.return_type) {
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

    fn check_extern_ty(&mut self, span: Span, ty: nia_ids::InternedTyId, context: &str) {
        match self.interner.get(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Bool)) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use `bool` directly"),
            )),
            Some(TyKind::Primitive(PrimitiveTy::Char)) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use `char` directly"),
            )),
            Some(TyKind::Primitive(PrimitiveTy::Void)) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use `void` directly"),
            )),
            Some(TyKind::Primitive(PrimitiveTy::Never)) => self.diagnostics.push(
                Diagnostic::error(span, format!("{context} cannot use `!` directly")),
            ),
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
                if !self.is_void(*return_type) {
                    self.check_extern_ty(span, *return_type, "extern function pointer return type");
                }
            }
            Some(TyKind::Array { .. }) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use array by value"),
            )),
            Some(TyKind::Nominal { def_id, .. }) => {
                if self.is_enum_def(*def_id) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!("{context} cannot use enum directly; use its backing integer type"),
                    ));
                }
                if self.is_union_def(*def_id) {
                    // NIA-FUTURE(internal-abi): classify union by-value passing separately from C ABI.
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!("{context} cannot use union by value"),
                    ));
                }
                if let Some(signature) = self.struct_signature(*def_id) {
                    if signature.fields.is_empty() {
                        self.diagnostics.push(Diagnostic::error(
                            span,
                            format!("{context} cannot use empty struct by value"),
                        ));
                    } else if !signature.is_extern {
                        self.diagnostics.push(Diagnostic::error(
                            span,
                            format!("{context} cannot use normal Nia struct by value"),
                        ));
                    }
                }
            }
            Some(TyKind::BuiltinTrait { .. }) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use trait type directly"),
            )),
            Some(TyKind::GenericParam(_)) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use generic parameter"),
            )),
            Some(TyKind::Projection { .. }) => self.diagnostics.push(Diagnostic::error(
                span,
                format!("{context} cannot use unresolved associated type projection"),
            )),
            Some(TyKind::Error) | None => {}
        }
    }

    fn is_void(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::Void))
        )
    }

    fn struct_signature(&self, def_id: GlobalDefId) -> Option<&StructSignature> {
        if def_id.module_id == self.defs.module_id {
            self.signatures.structs.get(&def_id.def_id)
        } else {
            self.program_signatures.structs.get(&def_id)
        }
    }

    fn is_union_def(&self, def_id: GlobalDefId) -> bool {
        if def_id.module_id == self.defs.module_id {
            self.signatures.unions.contains_key(&def_id.def_id)
        } else {
            self.program_signatures.unions.contains_key(&def_id)
        }
    }

    fn is_enum_def(&self, def_id: GlobalDefId) -> bool {
        if def_id.module_id == self.defs.module_id {
            self.signatures.enums.contains_key(&def_id.def_id)
        } else {
            self.program_signatures.enums.contains_key(&def_id)
        }
    }

    fn is_never(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::Never))
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
struct Pair { x: i32 }
struct Empty {}
extern struct BadExtern { flag: bool }
extern struct ExternEmpty {}

extern fn bad(flag: bool, ch: char, nothing: void, color: Color, xs: [2]u8, pair: Pair, empty: Empty, extern_empty: ExternEmpty, cb: &const fn(i32, ...) !, ...);
extern fn bad_never_return() !;
extern fn bad_variadic_definition(fmt: &u8, ...) {
}
extern const bad_global: bool;
union Bits { i: i32 }
extern fn bad_union(bits: Bits);
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
            "`void`",
            "`!`",
            "enum directly",
            "array by value",
            "normal Nia struct by value",
            "empty struct by value",
            "variadic function pointer",
            "variadic function definition",
            "union by value",
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
