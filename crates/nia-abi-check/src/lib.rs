// SPDX-License-Identifier: GPL-3.0-or-later
//! Validates the subset of Nia types that may cross a C ABI boundary.
//!
//! ABI checking runs on signature types before general type normalization, so
//! aliases must be expanded here rather than treated as opaque nominal types.
//! The checker is intentionally conservative for Nia-specific fat pointers,
//! tagged values, closures, and aggregates whose representation is not a C
//! contract.

use std::collections::HashMap;

use nia_defs::{DefCollection, DefId};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::GlobalDefId;
use nia_item_signatures::{
    EnumSignature, FunctionAttribute, FunctionSignature, GlobalSignature, ItemSignatures,
    StructSignature, TypeAliasSignature, UnionSignature, generic_argument_substitutions,
};
use nia_span::Span;
use nia_ty::{ArrayLenTy, PrimitiveTy, TyKind, TypeStore};

/// Program-wide nominal declarations needed to classify imported ABI types.
///
/// Type aliases are part of this index because a foreign alias remains nominal
/// in the requesting module's signature store until this checker expands it.
#[derive(Debug, Clone, Copy)]
pub struct ProgramAbiSignatures<'a> {
    /// Imported struct signatures keyed by global definition.
    pub structs: &'a HashMap<GlobalDefId, StructSignature>,
    /// Imported union signatures keyed by global definition.
    pub unions: &'a HashMap<GlobalDefId, UnionSignature>,
    /// Imported enum signatures keyed by global definition.
    pub enums: &'a HashMap<GlobalDefId, nia_item_signatures::EnumSignature>,
    /// Imported type-alias signatures keyed by global definition.
    pub type_aliases: &'a HashMap<GlobalDefId, TypeAliasSignature>,
}

/// Diagnostics produced while validating one module's foreign declarations.
#[derive(Debug, Clone, PartialEq)]
pub struct AbiCheck {
    /// Diagnostics emitted while validating ABI declarations.
    pub diagnostics: Vec<Diagnostic>,
}

/// Per-item-family signature views for the module being checked.
///
/// Keeping these as borrowed maps lets query providers assemble independently
/// cached function, type, and value signature products without cloning them.
#[derive(Debug, Clone, Copy)]
pub struct ModuleAbiSignatures<'a> {
    /// Function signatures declared by the module.
    pub functions: &'a HashMap<DefId, FunctionSignature>,
    /// Struct signatures declared by the module.
    pub structs: &'a HashMap<DefId, StructSignature>,
    /// Union signatures declared by the module.
    pub unions: &'a HashMap<DefId, UnionSignature>,
    /// Enum signatures declared by the module.
    pub enums: &'a HashMap<DefId, EnumSignature>,
    /// Type aliases declared by the module.
    pub type_aliases: &'a HashMap<DefId, TypeAliasSignature>,
    /// Global signatures declared by the module.
    pub globals: &'a HashMap<DefId, GlobalSignature>,
}

/// Checks ABI declarations from a complete, eagerly collected module snapshot.
pub fn check_module_abi(
    defs: &DefCollection,
    type_store: &TypeStore,
    signatures: &ItemSignatures,
) -> AbiCheck {
    let empty_structs = HashMap::new();
    let empty_unions = HashMap::new();
    let empty_enums = HashMap::new();
    let empty_type_aliases = HashMap::new();
    check_module_abi_with_program_signatures(
        defs,
        type_store,
        signatures,
        ProgramAbiSignatures {
            structs: &empty_structs,
            unions: &empty_unions,
            enums: &empty_enums,
            type_aliases: &empty_type_aliases,
        },
    )
}

/// Checks one module while resolving imported nominal types from `program_signatures`.
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
            type_aliases: &signatures.type_aliases,
            globals: &signatures.globals,
        },
        program_signatures,
    )
}

/// Checks independently cached signature families against one C ABI contract.
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
        self.check_extern_ty_inner(span, ty, context, &mut Vec::new());
    }

    fn check_extern_ty_inner(
        &mut self,
        span: Span,
        ty: nia_ids::InternedTyId,
        context: ExternTyContext,
        nominal_stack: &mut Vec<GlobalDefId>,
    ) {
        if matches!(
            context,
            ExternTyContext::FunctionReturn | ExternTyContext::FunctionPointerReturn
        ) && self.is_unit(ty)
        {
            return;
        }
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
            Some(TyKind::Callable { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use nia callable view directly"),
            )),
            Some(TyKind::CallablePointee { .. }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::STATIC_CHECK,
                    span,
                    format!("{context_desc} cannot use unsized callable interface directly"),
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
                    self.check_extern_ty_inner(
                        span,
                        *param,
                        ExternTyContext::FunctionPointerParameter,
                        nominal_stack,
                    );
                }
                if !self.is_unit(*return_type) {
                    self.check_extern_ty_inner(
                        span,
                        *return_type,
                        ExternTyContext::FunctionPointerReturn,
                        nominal_stack,
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
                    self.check_extern_ty_inner(
                        span,
                        *elem,
                        ExternTyContext::StructField,
                        nominal_stack,
                    );
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
            Some(TyKind::ClosureState { .. }) => self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                span,
                format!("{context_desc} cannot use closure state directly"),
            )),
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                if let Some(alias) = self.type_alias_signature(*def_id).cloned() {
                    if nominal_stack.contains(def_id) {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::STATIC_CHECK,
                            span,
                            format!("recursive type alias cannot be used as {context_desc}"),
                        ));
                        return;
                    }
                    let Some((substitutions, const_substitutions)) =
                        generic_argument_substitutions(&alias.generic_params, args, const_args)
                    else {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::STATIC_CHECK,
                            span,
                            format!("{context_desc} has invalid type alias arguments"),
                        ));
                        return;
                    };
                    // Signature ABI checks precede general normalization. Expand aliases here so
                    // an alias to `bool`, a Nia aggregate, or another forbidden representation
                    // cannot be mistaken for an ABI-safe nominal type.
                    let append = self.type_store.append_for_module(self.defs.module_id);
                    let target = nia_ty::substitute_ty(
                        self.type_store,
                        &append,
                        alias.target,
                        &|name| substitutions.get(name).copied(),
                        &|name| const_substitutions.get(name).cloned(),
                        None,
                    );
                    nominal_stack.push(*def_id);
                    self.check_extern_ty_inner(span, target, context, nominal_stack);
                    nominal_stack.pop();
                    return;
                }
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
                    } else if def_id.module_id != self.defs.module_id
                        && !nominal_stack.contains(def_id)
                    {
                        // Local extern structs are checked by `check`, but an
                        // imported signature has no local declaration pass.
                        // Walk its fields here so a foreign `extern struct`
                        // cannot smuggle bools, Nia aggregates, or other
                        // forbidden representations through one nominal value.
                        let Some((substitutions, const_substitutions)) =
                            generic_argument_substitutions(
                                &signature.generic_params,
                                args,
                                const_args,
                            )
                        else {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::STATIC_CHECK,
                                span,
                                format!("{context_desc} has invalid extern struct arguments"),
                            ));
                            return;
                        };
                        let append = self.type_store.append_for_module(self.defs.module_id);
                        let fields = signature
                            .fields
                            .iter()
                            .map(|field| {
                                let ty = nia_ty::substitute_ty(
                                    self.type_store,
                                    &append,
                                    field.ty,
                                    &|name| substitutions.get(name).copied(),
                                    &|name| const_substitutions.get(name).cloned(),
                                    None,
                                );
                                (field.span, ty)
                            })
                            .collect::<Vec<_>>();
                        nominal_stack.push(*def_id);
                        for (field_span, field_ty) in fields {
                            self.check_extern_ty_inner(
                                field_span,
                                field_ty,
                                ExternTyContext::StructField,
                                nominal_stack,
                            );
                        }
                        nominal_stack.pop();
                    }
                } else if !self.is_union_def(*def_id) && !self.is_enum_def(*def_id) {
                    self.diagnostics.push(Diagnostic::internal_error_at(
                        codes::STATIC_CHECK,
                        span,
                        format!("{context_desc} nominal type {def_id:?} has no ABI classification"),
                    ));
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

    fn type_alias_signature(&self, def_id: GlobalDefId) -> Option<&TypeAliasSignature> {
        if def_id.module_id == self.defs.module_id {
            self.signatures.type_aliases.get(&def_id.def_id)
        } else {
            self.program_signatures.type_aliases.get(&def_id)
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
    use nia_item_signatures::{
        FieldSignature, ItemSignatureInput, ItemSignatureSource, ParamSignature,
        collect_item_signatures,
    };
    use nia_parser::parse_module;
    use nia_symbol::{SymbolId, stable_hash};
    use nia_type_lower::{
        ProgramDefsContext, TypeLoweringContext, lower_module_types_with_context,
    };
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

extern fn bad(flag: bool, ch: char, nothing: (), color: Color, xs: [u8; 2], pair: Pair, empty: Empty, extern_empty: ExternEmpty, cb: &fn(i32, ...) never, ...);
extern fn bad_never_return() never;
extern fn bad_variadic_definition(fmt: &u8, ...) {
}
extern static bad_global: bool;
union Bits { i: i32 }
extern fn bad_union(bits: Bits);
extern fn bad_callable_view(callback: &Fn(i32) i32);
extern fn bad_callable_pointee(callback: Fn(i32) i32);
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
            "nia callable view directly",
            "unsized callable interface directly",
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
    reserved: [u64; 4],
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

    #[test]
    fn checks_fields_of_imported_extern_structs() {
        let (module, errors) = parse_module("extern fn consume(value: Imported);");
        assert!(errors.is_empty(), "{errors:?}");
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let foreign_module_id = module_ids.allocate();
        let defs = collect_module_defs(module_id, &module);
        let type_store = TypeStore::new();
        let append = type_store.append_for_module(module_id);
        let type_name = SymbolId::from_stable_hash(stable_hash("T"));
        let const_name = SymbolId::from_stable_hash(stable_hash("N"));
        let usize_ty = append.primitive(PrimitiveTy::Usize);
        let imported_ty = append.intern(TyKind::Nominal {
            def_id: GlobalDefId {
                module_id: foreign_module_id,
                def_id: DefId(0),
            },
            args: vec![append.primitive(PrimitiveTy::Bool)],
            const_args: vec![nia_ty::ConstGenericArg {
                ty: usize_ty,
                value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(4)),
            }],
        });
        let mut signatures = ItemSignatures {
            functions: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            traits: HashMap::new(),
            trait_impls: Vec::new(),
            enums: HashMap::new(),
            type_aliases: HashMap::new(),
            globals: HashMap::new(),
            consts: HashMap::new(),
            diagnostics: Vec::new(),
        };
        signatures.functions.insert(
            DefId(0),
            FunctionSignature {
                name: SymbolId::from_stable_hash(stable_hash("consume")),
                generics: Vec::new(),
                generic_params: Vec::new(),
                where_predicates: Vec::new(),
                params: vec![ParamSignature {
                    name: None,
                    receiver: None,
                    ty: imported_ty,
                    span: Span::default(),
                }],
                return_type: append.intern(TyKind::Tuple(Vec::new())),
                is_extern: true,
                is_const: false,
                is_variadic: false,
                attributes: Vec::new(),
                has_body: false,
                span: Span::default(),
            },
        );
        let mut foreign_structs = HashMap::new();
        foreign_structs.insert(
            GlobalDefId {
                module_id: foreign_module_id,
                def_id: DefId(0),
            },
            StructSignature {
                generics: vec![type_name, const_name],
                generic_params: vec![
                    nia_item_signatures::GenericParamSignature {
                        name: type_name,
                        kind: nia_item_signatures::GenericParamSignatureKind::Type,
                    },
                    nia_item_signatures::GenericParamSignature {
                        name: const_name,
                        kind: nia_item_signatures::GenericParamSignatureKind::Const {
                            ty: usize_ty,
                        },
                    },
                ],
                where_predicates: Vec::new(),
                fields: vec![FieldSignature {
                    def_id: DefId(0),
                    name: SymbolId::from_stable_hash(stable_hash("flag")),
                    ty: append.intern(TyKind::Array {
                        elem: append.intern(TyKind::GenericParam(type_name)),
                        len: ArrayLenTy::GenericParam(const_name),
                    }),
                    span: Span::default(),
                }],
                is_tuple: false,
                is_extern: true,
                span: Span::default(),
            },
        );
        let empty_unions = HashMap::new();
        let empty_enums = HashMap::new();
        let empty_aliases = HashMap::new();
        let checked = check_module_abi_families_with_program_signatures(
            &defs,
            &type_store,
            ModuleAbiSignatures {
                functions: &signatures.functions,
                structs: &signatures.structs,
                unions: &signatures.unions,
                enums: &signatures.enums,
                type_aliases: &signatures.type_aliases,
                globals: &signatures.globals,
            },
            ProgramAbiSignatures {
                structs: &foreign_structs,
                unions: &empty_unions,
                enums: &empty_enums,
                type_aliases: &empty_aliases,
            },
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("`bool`")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn expands_type_aliases_before_classifying_extern_abi_types() {
        let (module, errors) = parse_module(
            r#"
type Flag = bool;
type Identity[T] = T;
type HeaderAlias = Header;
type Unit = ();

extern struct Header { value: i32 }
extern fn bad_flag(flag: Flag);
extern fn bad_generic(flag: Identity[bool]);
extern fn consume(header: HeaderAlias);
extern fn effect() Unit;
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

        assert_eq!(
            checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.summary.contains("cannot use `bool` directly"))
                .count(),
            2,
            "{:?}",
            checked.diagnostics
        );
        assert_eq!(checked.diagnostics.len(), 2, "{:?}", checked.diagnostics);
    }

    #[test]
    fn expands_const_generic_aliases_before_classifying_extern_struct_fields() {
        let (module, errors) = parse_module(
            r#"
type Repeat[T, N: usize] = [T; N];
extern struct Header { values: Repeat[bool, 4] }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let defs = collect_module_defs(module_id, &module);
        let resolved = resolve_module_types(&module, &defs);
        let type_store = TypeStore::new();
        let program_defs =
            |requested| (requested == module_id).then(|| std::sync::Arc::new(defs.clone()));
        let lowered = lower_module_types_with_context(
            module_id,
            &module,
            &resolved,
            TypeLoweringContext::from_program_defs(
                &type_store,
                ProgramDefsContext {
                    defs: Some(&program_defs),
                },
            ),
        );
        let signatures = collect_item_signatures(ItemSignatureInput {
            source: ItemSignatureSource::Module(&module),
            defs: &defs,
            lowered: &lowered,
            type_store: &type_store,
            symbols: None,
        });
        let checked = check_module_abi(&defs, &type_store, &signatures);

        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains("cannot use `bool` directly")),
            "{:?}",
            checked.diagnostics
        );
        assert!(
            checked
                .diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.summary.contains("invalid type alias arguments")),
            "{:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn allows_unit_returns_and_rejects_tuple_values_at_c_boundaries() {
        let (module, errors) = parse_module(
            r#"
extern fn effect();
extern fn bad_param(value: (i32, bool));
extern fn bad_return() (i32, bool);
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
        assert_eq!(
            checked
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.summary.contains("cannot use tuple by value"))
                .count(),
            2,
            "{:?}",
            checked.diagnostics
        );
    }
}
