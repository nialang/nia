// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_defs::{DefCollection, DefId};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::GlobalDefId;
use nia_item_signatures::{
    EnumSignature, FunctionAttribute, FunctionSignature, GlobalSignature, ItemSignatures,
    StructSignature, UnionSignature,
};
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyKind, TypeStore};

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

#[derive(Debug, Clone, Copy)]
pub struct ModuleAbiSignatures<'a> {
    pub functions: &'a HashMap<DefId, FunctionSignature>,
    pub structs: &'a HashMap<DefId, StructSignature>,
    pub unions: &'a HashMap<DefId, UnionSignature>,
    pub enums: &'a HashMap<DefId, EnumSignature>,
    pub globals: &'a HashMap<DefId, GlobalSignature>,
}

pub fn check_module_abi(
    defs: &DefCollection,
    type_store: &TypeStore,
    signatures: &ItemSignatures,
) -> AbiCheck {
    let empty_structs = HashMap::new();
    let empty_unions = HashMap::new();
    let empty_enums = HashMap::new();
    check_module_abi_with_program_signatures(
        defs,
        type_store,
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
    type_store: &TypeStore,
    signatures: &ItemSignatures,
    program_signatures: ProgramAbiSignatures<'_>,
) -> AbiCheck {
    check_module_abi_families_with_program_signatures(
        defs,
        type_store,
        ModuleAbiSignatures {
            functions: &signatures.functions,
            structs: &signatures.structs,
            unions: &signatures.unions,
            enums: &signatures.enums,
            globals: &signatures.globals,
        },
        program_signatures,
    )
}

pub fn check_module_abi_families_with_program_signatures(
    defs: &DefCollection,
    type_store: &TypeStore,
    signatures: ModuleAbiSignatures<'_>,
    program_signatures: ProgramAbiSignatures<'_>,
) -> AbiCheck {
    let mut checker = AbiChecker {
        defs,
        type_store,
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
    type_store: &'a TypeStore,
    signatures: ModuleAbiSignatures<'a>,
    program_signatures: ProgramAbiSignatures<'a>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternTyContext {
    StructField,
    FunctionParameter,
    FunctionReturn,
    FunctionPointerParameter,
    FunctionPointerReturn,
    Global,
}

impl ExternTyContext {
    fn description(self) -> &'static str {
        match self {
            Self::StructField => "extern struct field",
            Self::FunctionParameter => "extern parameter",
            Self::FunctionReturn => "extern return type",
            Self::FunctionPointerParameter => "extern function pointer parameter",
            Self::FunctionPointerReturn => "extern function pointer return type",
            Self::Global => "extern global",
        }
    }
}

impl AbiChecker<'_> {
    fn check(&mut self) {
        for (def_id, signature) in self.signatures.functions {
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
            self.check_extern_ty(field.span, field.ty, ExternTyContext::StructField);
        }
    }

    fn check_extern_global(&mut self, signature: &nia_item_signatures::GlobalSignature) {
        let Some(ty) = signature.explicit_type else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                signature.span,
                "extern global requires an explicit type",
            ));
            return;
        };
        self.check_extern_ty(signature.span, ty, ExternTyContext::Global);
    }

    fn check_extern_function(&mut self, def_id: DefId, signature: &FunctionSignature) {
        if signature
            .attributes
            .iter()
            .any(|attribute| matches!(attribute, FunctionAttribute::Builtin(_)))
        {
            return;
        }
        if !signature.generics.is_empty() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                signature.span,
                "extern function cannot have generic parameters",
            ));
        }
        if signature.is_variadic && signature.params.is_empty() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                signature.span,
                "extern variadic function requires at least one fixed parameter",
            ));
        }
        if signature.is_variadic && signature.has_body {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                signature.span,
                "extern variadic function definition is not supported",
            ));
        }
        for param in &signature.params {
            self.check_extern_ty(param.span, param.ty, ExternTyContext::FunctionParameter);
        }
        if self.is_never(signature.return_type) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                signature.span,
                "extern return type cannot use `never`",
            ));
        } else if !self.is_unit(signature.return_type) {
            self.check_extern_ty(
                signature.span,
                signature.return_type,
                ExternTyContext::FunctionReturn,
            );
        }
        if let Some(def) = self.defs.defs.get(def_id)
            && def.parent.is_some()
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                signature.span,
                "extern method ABI is not defined",
            ));
        }
    }

    fn check_extern_ty(&mut self, span: Span, ty: nia_ids::InternedTyId, context: ExternTyContext) {
        let context_desc = context.description();
        match self.type_store.get(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Bool)) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::STATIC_CHECK,
                    span,
                    format!("{context_desc} cannot use `bool` directly"),
                ))
            }
            Some(TyKind::Primitive(PrimitiveTy::Char)) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::STATIC_CHECK,
                    span,
                    format!("{context_desc} cannot use `char` directly"),
                ))
            }
            Some(TyKind::Primitive(PrimitiveTy::Never)) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::STATIC_CHECK,
                    span,
                    format!("{context_desc} cannot use `never` directly"),
                ))
            }
            Some(TyKind::Primitive(_))
            | Some(TyKind::Pointer { .. })
            | Some(TyKind::VolatilePointer { .. }) => {}
            Some(TyKind::Opaque) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use incomplete `opaque` directly"),
            )),
            Some(TyKind::Tuple(_)) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use tuple by value"),
            )),
            Some(TyKind::Vector { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use SIMD vector by value"),
            )),
            Some(TyKind::Slice { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use nia slice directly"),
            )),
            Some(TyKind::SlicePointee { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use unsized slice pointee directly"),
            )),
            Some(TyKind::TraitObject { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use nia trait object directly"),
            )),
            Some(TyKind::TraitObjectPointee { .. }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::STATIC_CHECK,
                    span,
                    format!("{context_desc} cannot use unsized trait object pointee directly"),
                ))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                if *is_variadic {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        span,
                        format!("{context_desc} cannot use variadic function pointer"),
                    ));
                }
                for param in params {
                    self.check_extern_ty(span, *param, ExternTyContext::FunctionPointerParameter);
                }
                if !self.is_unit(*return_type) {
                    self.check_extern_ty(
                        span,
                        *return_type,
                        ExternTyContext::FunctionPointerReturn,
                    );
                }
            }
            Some(TyKind::Array { len, elem }) => {
                if context == ExternTyContext::StructField {
                    if matches!(len, ArrayLenTy::Infer) {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::STATIC_CHECK,
                            span,
                            "extern struct field cannot use inferred array length",
                        ));
                    }
                    self.check_extern_ty(span, *elem, ExternTyContext::StructField);
                } else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        span,
                        format!("{context_desc} cannot use array by value"),
                    ));
                }
            }
            Some(TyKind::Range { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use range by value"),
            )),
            Some(TyKind::Optional { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use optional by value"),
            )),
            Some(TyKind::ErrorUnion { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use error union by value"),
            )),
            Some(TyKind::Nominal { def_id, .. }) => {
                if self.is_enum_def(*def_id) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        span,
                        format!(
                            "{context_desc} cannot use enum directly; use its backing integer type"
                        ),
                    ));
                }
                if self.is_union_def(*def_id) {
                    // NIA-FUTURE(internal-abi): classify union by-value passing separately from C ABI.
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::STATIC_CHECK,
                        span,
                        format!("{context_desc} cannot use union by value"),
                    ));
                }
                if let Some(signature) = self.struct_signature(*def_id) {
                    if signature.fields.is_empty() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::STATIC_CHECK,
                            span,
                            format!("{context_desc} cannot use empty struct by value"),
                        ));
                    } else if !signature.is_extern {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::STATIC_CHECK,
                            span,
                            format!("{context_desc} cannot use normal Nia struct by value"),
                        ));
                    }
                }
            }
            Some(TyKind::BuiltinTrait { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use trait type directly"),
            )),
            Some(TyKind::BuiltinType(builtin)) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!(
                    "{context_desc} cannot use builtin type `{}` directly",
                    builtin.name()
                ),
            )),
            Some(TyKind::GenericParam(_) | TyKind::SelfParam) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::STATIC_CHECK,
                    span,
                    format!("{context_desc} cannot use generic parameter"),
                ))
            }
            Some(TyKind::Projection { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use unresolved associated type projection"),
            )),
            Some(TyKind::ConstOnly) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use const-only value"),
            )),
            Some(TyKind::Error) => {}
            None => self.diagnostics.push(Diagnostic::internal_error_at(
                codes::STATIC_CHECK,
                span,
                format!("extern ABI type {ty:?} is missing from the session type store"),
            )),
        }
    }

    fn is_unit(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(self.type_store.get(ty), Some(TyKind::Tuple(elems)) if elems.is_empty())
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
            self.type_store.get(ty),
            Some(TyKind::Primitive(PrimitiveTy::Never))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::collect_module_defs;
    use nia_ids::ModuleIdAllocator;
    use nia_item_signatures::{ItemSignatureInput, ItemSignatureSource, collect_item_signatures};
    use nia_parser::parse_module;
    use nia_type_lower::{TypeLoweringContext, lower_module_types_with_context};
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

extern fn bad(flag: bool, ch: char, nothing: (), color: Color, xs: [2]u8, pair: Pair, empty: Empty, extern_empty: ExternEmpty, cb: &fn(i32, ...) never, ...);
extern fn bad_never_return() never;
extern fn bad_variadic_definition(fmt: &u8, ...) {
}
extern static bad_global: bool;
union Bits { i: i32 }
extern fn bad_union(bits: Bits);
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let lowered = lower_module_types_with_context(
            module_id,
            &module,
            &resolved,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_item_signatures(ItemSignatureInput {
            source: ItemSignatureSource::Module(&module),
            defs: &defs,
            lowered: &lowered,
            type_store: &type_store,
            symbols: None,
        });
        let checked = check_module_abi(&defs, &type_store, &signatures);
        for expected in [
            "`bool`",
            "`char`",
            "`never`",
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
                    .any(|diagnostic| diagnostic.summary.contains(expected)),
                "{expected}: {:?}",
                checked.diagnostics
            );
        }
    }

    #[test]
    fn allows_fixed_arrays_inside_extern_structs() {
        let (module, errors) = parse_module(
            r#"
extern struct Header {
    tag: u32,
    reserved: [4]u64,
}

extern fn consume(header: Header);
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let lowered = lower_module_types_with_context(
            module_id,
            &module,
            &resolved,
            TypeLoweringContext::empty(&type_store),
        );
        let signatures = collect_item_signatures(ItemSignatureInput {
            source: ItemSignatureSource::Module(&module),
            defs: &defs,
            lowered: &lowered,
            type_store: &type_store,
            symbols: None,
        });
        let checked = check_module_abi(&defs, &type_store, &signatures);
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }
}
