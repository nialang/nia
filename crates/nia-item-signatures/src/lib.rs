// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{
    BindingItem, EnumItem, ExtendItem, FunctionItem, ItemKind, Module, Param, ReceiverKind,
    StructItem, TypeAliasItem, UnionItem,
};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::TyId;
use nia_span::Span;
use nia_ty::PrimitiveTy;
use nia_type_lower::TypeLowering;

#[derive(Debug, Clone, PartialEq)]
pub struct ItemSignatures {
    pub functions: HashMap<DefId, FunctionSignature>,
    pub structs: HashMap<DefId, StructSignature>,
    pub unions: HashMap<DefId, UnionSignature>,
    pub enums: HashMap<DefId, EnumSignature>,
    pub type_aliases: HashMap<DefId, TypeAliasSignature>,
    pub globals: HashMap<DefId, GlobalSignature>,
    pub comptimes: HashMap<DefId, ComptimeSignature>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSignature {
    pub generics: Vec<String>,
    pub params: Vec<ParamSignature>,
    pub return_type: TyId,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub has_body: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamSignature {
    pub name: Option<String>,
    pub receiver: Option<ReceiverKind>,
    pub ty: TyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructSignature {
    pub generics: Vec<String>,
    pub fields: Vec<FieldSignature>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnionSignature {
    pub generics: Vec<String>,
    pub fields: Vec<FieldSignature>,
    pub is_extern: bool,
    pub span: Span,
}

impl UnionSignature {
    pub fn as_struct_like(&self) -> StructSignature {
        StructSignature {
            generics: self.generics.clone(),
            fields: self.fields.clone(),
            is_extern: self.is_extern,
            span: self.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldSignature {
    pub def_id: DefId,
    pub name: String,
    pub ty: TyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumSignature {
    pub backing_type: TyId,
    pub is_open: bool,
    pub variants: Vec<EnumVariantSignature>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariantSignature {
    pub def_id: DefId,
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasSignature {
    pub generics: Vec<String>,
    pub target: TyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlobalSignature {
    pub explicit_type: Option<TyId>,
    pub is_const: bool,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeSignature {
    pub explicit_type: Option<TyId>,
    pub span: Span,
}

pub fn collect_item_signatures(
    module: &Module,
    defs: &DefCollection,
    lowered: &TypeLowering,
) -> ItemSignatures {
    let mut collector = SignatureCollector {
        defs,
        lowered,
        diagnostics: Vec::new(),
    };
    let mut signatures = ItemSignatures {
        functions: HashMap::new(),
        structs: HashMap::new(),
        unions: HashMap::new(),
        enums: HashMap::new(),
        type_aliases: HashMap::new(),
        globals: HashMap::new(),
        comptimes: HashMap::new(),
        diagnostics: Vec::new(),
    };
    for item in &module.items {
        collector.collect_item_into(&mut signatures, item);
    }
    signatures.diagnostics = collector.diagnostics;
    signatures
}

struct SignatureCollector<'a> {
    defs: &'a DefCollection,
    lowered: &'a TypeLowering,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> SignatureCollector<'a> {
    fn collect_item_into(&mut self, signatures: &mut ItemSignatures, item: &nia_ast::Item) {
        match &item.kind {
            ItemKind::Import(_) | ItemKind::Using(_) => {}
            ItemKind::Struct(item_struct) => {
                self.collect_struct(signatures, item.span, item_struct);
            }
            ItemKind::Union(item_union) => {
                self.collect_union(signatures, item.span, item_union);
            }
            ItemKind::Extend(extend) => {
                self.collect_extend(signatures, extend);
            }
            ItemKind::Enum(item_enum) => {
                self.collect_enum(signatures, item.span, item_enum);
            }
            ItemKind::TypeAlias(alias) => {
                self.collect_type_alias(signatures, item.span, alias);
            }
            ItemKind::Function(function) => {
                self.collect_function(signatures, item.span, function);
            }
            ItemKind::Binding(binding) => {
                if binding.is_comptime {
                    self.collect_comptime(signatures, item.span, binding);
                } else {
                    self.collect_global(signatures, item.span, binding);
                }
            }
        }
    }

    fn collect_struct(
        &mut self,
        signatures: &mut ItemSignatures,
        item_span: Span,
        item_struct: &StructItem,
    ) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::Struct) else {
            return;
        };
        let mut fields = Vec::new();
        for field in &item_struct.fields {
            let Some(field_id) = self.def_id_for_span(field.span, DefKind::StructField) else {
                continue;
            };
            fields.push(FieldSignature {
                def_id: field_id,
                name: field.name.clone(),
                ty: self.ty_for_span(field.ty.span),
                span: field.span,
            });
        }
        signatures.structs.insert(
            def_id,
            StructSignature {
                generics: item_struct.generics.clone(),
                fields,
                is_extern: item_struct.is_extern,
                span: item_span,
            },
        );
    }

    fn collect_union(
        &mut self,
        signatures: &mut ItemSignatures,
        item_span: Span,
        item_union: &UnionItem,
    ) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::Union) else {
            return;
        };
        let mut fields = Vec::new();
        for field in &item_union.fields {
            let Some(field_id) = self.def_id_for_span(field.span, DefKind::UnionField) else {
                continue;
            };
            fields.push(FieldSignature {
                def_id: field_id,
                name: field.name.clone(),
                ty: self.ty_for_span(field.ty.span),
                span: field.span,
            });
        }
        signatures.unions.insert(
            def_id,
            UnionSignature {
                generics: item_union.generics.clone(),
                fields,
                is_extern: item_union.is_extern,
                span: item_span,
            },
        );
    }

    fn collect_extend(&mut self, signatures: &mut ItemSignatures, extend: &ExtendItem) {
        for method in &extend.methods {
            self.collect_method(signatures, &method.function);
        }
    }

    fn collect_method(&mut self, signatures: &mut ItemSignatures, method: &FunctionItem) {
        let Some(def_id) = self.def_id_for_span(method.span, DefKind::Method) else {
            return;
        };
        signatures
            .functions
            .insert(def_id, self.function_signature(method));
    }

    fn collect_enum(
        &mut self,
        signatures: &mut ItemSignatures,
        item_span: Span,
        item_enum: &EnumItem,
    ) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::Enum) else {
            return;
        };
        let backing_type = match &item_enum.backing_type {
            Some(ty) => self.ty_for_span(ty.span),
            None => self.primitive(PrimitiveTy::I32),
        };
        let mut variants = Vec::new();
        for variant in &item_enum.variants {
            let Some(variant_id) = self.def_id_for_span(variant.span, DefKind::EnumVariant) else {
                continue;
            };
            variants.push(EnumVariantSignature {
                def_id: variant_id,
                name: variant.name.clone(),
                span: variant.span,
            });
        }
        signatures.enums.insert(
            def_id,
            EnumSignature {
                backing_type,
                is_open: item_enum.is_open,
                variants,
                span: item_span,
            },
        );
    }

    fn collect_type_alias(
        &mut self,
        signatures: &mut ItemSignatures,
        item_span: Span,
        alias: &TypeAliasItem,
    ) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::TypeAlias) else {
            return;
        };
        signatures.type_aliases.insert(
            def_id,
            TypeAliasSignature {
                generics: alias.generics.clone(),
                target: self.ty_for_span(alias.ty.span),
                span: item_span,
            },
        );
    }

    fn collect_function(
        &mut self,
        signatures: &mut ItemSignatures,
        item_span: Span,
        function: &FunctionItem,
    ) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::Function) else {
            return;
        };
        signatures
            .functions
            .insert(def_id, self.function_signature(function));
    }

    fn collect_global(
        &mut self,
        signatures: &mut ItemSignatures,
        item_span: Span,
        binding: &BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::Global) else {
            return;
        };
        signatures.globals.insert(
            def_id,
            GlobalSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_span(ty.span)),
                is_const: binding.is_const,
                is_extern: binding.is_extern,
                span: item_span,
            },
        );
    }

    fn collect_comptime(
        &mut self,
        signatures: &mut ItemSignatures,
        item_span: Span,
        binding: &BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_span(item_span, DefKind::Comptime) else {
            return;
        };
        signatures.comptimes.insert(
            def_id,
            ComptimeSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_span(ty.span)),
                span: item_span,
            },
        );
    }

    fn function_signature(&mut self, function: &FunctionItem) -> FunctionSignature {
        let params = function
            .params
            .iter()
            .map(|param| self.param_signature(param))
            .collect();
        let return_type = match &function.return_type {
            Some(ty) => self.ty_for_span(ty.span),
            None => self.primitive(PrimitiveTy::Void),
        };
        FunctionSignature {
            generics: function.generics.clone(),
            params,
            return_type,
            is_extern: function.is_extern,
            is_variadic: function.is_variadic,
            has_body: function.body.is_some(),
            span: function.span,
        }
    }

    fn param_signature(&mut self, param: &Param) -> ParamSignature {
        let ty = match &param.ty {
            Some(ty) => self.ty_for_span(ty.span),
            None if param.receiver.is_some() => self.error(),
            None => {
                self.diagnostics.push(Diagnostic::error(
                    param.span,
                    "parameter requires an explicit type",
                ));
                self.error()
            }
        };
        ParamSignature {
            name: param.name.clone(),
            receiver: param.receiver,
            ty,
            span: param.span,
        }
    }

    fn def_id_for_span(&mut self, span: Span, expected: DefKind) -> Option<DefId> {
        let Some(def_id) = self.defs.def_spans.get(span) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("missing definition id for {:?}", expected),
            ));
            return None;
        };
        let Some(def) = self.defs.defs.get(def_id) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "definition id does not exist in definition map",
            ));
            return None;
        };
        if def.kind != expected {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("definition kind mismatch: expected {:?}", expected),
            ));
            return None;
        }
        Some(def_id)
    }

    fn ty_for_span(&mut self, span: Span) -> TyId {
        if let Some(ty) = self.lowered.type_uses.get(&span).copied() {
            ty
        } else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "missing lowered type for signature",
            ));
            self.lowered.interner.error()
        }
    }

    fn primitive(&self, primitive: PrimitiveTy) -> TyId {
        self.lowered.interner.primitive(primitive)
    }

    fn error(&self) -> TyId {
        self.lowered.interner.error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_parser::parse_module;
    use nia_type_lower::lower_module_types;
    use nia_type_resolve::resolve_module_types;

    #[test]
    fn collects_item_signatures_without_checking_bodies() {
        let (module, errors) = parse_module(
            r#"
extern fn printf(fmt: &u8, ...);
extern const errno: i32;

struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fn len2(&self) i32 { missing + self.x }
}

enum Color: u8 {
    Red,
    Green,
}

type Byte = u8;
var counter: i32 = 0;

fn add(a: i32, b: i32) i32 {
    a + b
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let lowered = lower_module_types(&module, &resolved);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
        let signatures = collect_item_signatures(&module, &defs, &lowered);
        assert!(
            signatures.diagnostics.is_empty(),
            "{:?}",
            signatures.diagnostics
        );
        assert_eq!(signatures.structs.len(), 1);
        assert_eq!(signatures.enums.len(), 1);
        assert_eq!(signatures.type_aliases.len(), 1);
        assert_eq!(signatures.globals.len(), 2);
        assert_eq!(signatures.functions.len(), 3);
        assert!(
            signatures
                .functions
                .values()
                .any(|signature| signature.is_variadic)
        );
    }
}
