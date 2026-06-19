// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{
    Attribute, AttributeKind, BindingItem, EnumItem, ExtendItem, FunctionItem, Module, Param,
    ReceiverKind, StructItem, TraitItem, TypeAliasItem, TypeRef, UnionItem, WhereClause,
};
pub use nia_defs::{AssociatedTypeBindingSignature, WhereBoundSignature, WherePredicateSignature};
use nia_defs::{DefCollection, DefId, DefKind};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNode, ItemTreeNodeKind, ModuleItemTree};
use nia_node_id::NodeKey;
use nia_span::Span;
use nia_ty::PrimitiveTy;
use nia_type_lower::TypeLowering;

#[derive(Debug, Clone, PartialEq)]
pub struct ItemSignatures {
    pub functions: HashMap<DefId, FunctionSignature>,
    pub structs: HashMap<DefId, StructSignature>,
    pub unions: HashMap<DefId, UnionSignature>,
    pub traits: HashMap<DefId, TraitSignature>,
    pub trait_impls: Vec<TraitImplSignature>,
    pub enums: HashMap<DefId, EnumSignature>,
    pub type_aliases: HashMap<DefId, TypeAliasSignature>,
    pub globals: HashMap<DefId, GlobalSignature>,
    pub comptimes: HashMap<DefId, ComptimeSignature>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramFunctionSignature {
    pub name: String,
    pub signature: FunctionSignature,
    pub interner: nia_ty::TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramGlobalSignature {
    pub signature: GlobalSignature,
    pub interner: nia_ty::TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramComptimeSignature {
    pub signature: ComptimeSignature,
    pub interner: nia_ty::TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramStructSignature {
    pub signature: StructSignature,
    pub interner: nia_ty::TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramUnionSignature {
    pub signature: UnionSignature,
    pub interner: nia_ty::TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramEnumSignature {
    pub signature: EnumSignature,
    pub interner: nia_ty::TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramTraitSignature {
    pub signature: TraitSignature,
    pub interner: nia_ty::TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramTypeAliasSignature {
    pub signature: TypeAliasSignature,
    pub interner: nia_ty::TyInterner,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgramTraitImplSignature {
    pub module_id: nia_ids::ModuleId,
    pub local_index: usize,
    pub generics: Vec<String>,
    pub target_ty: InternedTyId,
    pub trait_id: nia_ty::TraitId,
    pub trait_args: Vec<InternedTyId>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub associated_types: Vec<TraitImplAssociatedTypeSignature>,
    pub interner: nia_ty::TyInterner,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramSignatureMaps<'a> {
    pub functions: &'a HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub globals: &'a HashMap<GlobalDefId, ProgramGlobalSignature>,
    pub comptimes: &'a HashMap<GlobalDefId, ProgramComptimeSignature>,
    pub structs: &'a HashMap<GlobalDefId, ProgramStructSignature>,
    pub unions: &'a HashMap<GlobalDefId, ProgramUnionSignature>,
    pub enums: &'a HashMap<GlobalDefId, ProgramEnumSignature>,
    pub traits: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
    pub type_aliases: &'a HashMap<GlobalDefId, ProgramTypeAliasSignature>,
    pub trait_impls: &'a [ProgramTraitImplSignature],
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSignature {
    pub generics: Vec<String>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub params: Vec<ParamSignature>,
    pub return_type: InternedTyId,
    pub is_extern: bool,
    pub is_comptime: bool,
    pub is_variadic: bool,
    pub attributes: Vec<FunctionAttribute>,
    pub has_body: bool,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionAttribute {
    Naked,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamSignature {
    pub name: Option<String>,
    pub receiver: Option<ReceiverKind>,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructSignature {
    pub generics: Vec<String>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub fields: Vec<FieldSignature>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnionSignature {
    pub generics: Vec<String>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub fields: Vec<FieldSignature>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitSignature {
    pub generics: Vec<String>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub supertraits: Vec<TraitSupertraitSignature>,
    pub associated_types: Vec<TraitAssociatedTypeSignature>,
    pub methods: Vec<TraitMethodSignature>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitSupertraitSignature {
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitAssociatedTypeSignature {
    pub def_id: DefId,
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitMethodSignature {
    pub def_id: DefId,
    pub name: String,
    pub signature: FunctionSignature,
    pub has_default: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplSignature {
    pub generics: Vec<String>,
    pub target_ty: InternedTyId,
    pub trait_ty: Option<InternedTyId>,
    pub trait_span: Option<Span>,
    pub where_predicates: Vec<WherePredicateSignature>,
    pub associated_types: Vec<TraitImplAssociatedTypeSignature>,
    pub associated_values: Vec<TraitImplAssociatedValueSignature>,
    pub methods: Vec<TraitImplMethodSignature>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplAssociatedTypeSignature {
    pub name: String,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplAssociatedValueSignature {
    pub def_id: DefId,
    pub name: String,
    pub visibility: nia_ast::Visibility,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitImplMethodSignature {
    pub def_id: DefId,
    pub name: String,
    pub visibility: nia_ast::Visibility,
    pub span: Span,
}

impl UnionSignature {
    pub fn as_struct_like(&self) -> StructSignature {
        StructSignature {
            generics: self.generics.clone(),
            where_predicates: self.where_predicates.clone(),
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
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumSignature {
    pub backing_type: InternedTyId,
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
    pub target: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlobalSignature {
    pub explicit_type: Option<InternedTyId>,
    pub is_let: bool,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeSignature {
    pub explicit_type: Option<InternedTyId>,
    pub span: Span,
}

pub fn collect_item_signatures(
    module: &Module,
    defs: &DefCollection,
    lowered: &TypeLowering,
) -> ItemSignatures {
    let item_tree = ModuleItemTree::from_module(module);
    collect_item_signatures_from_item_tree(&item_tree, defs, lowered)
}

pub fn collect_item_signatures_from_item_tree(
    item_tree: &ModuleItemTree,
    defs: &DefCollection,
    lowered: &TypeLowering,
) -> ItemSignatures {
    collect_item_signatures_from_items(&item_tree.items, defs, lowered)
}

pub fn collect_item_signatures_from_active_item_tree(
    item_tree: &ActiveModuleItemTree,
    defs: &DefCollection,
    lowered: &TypeLowering,
) -> ItemSignatures {
    collect_item_signatures_from_items(&item_tree.items, defs, lowered)
}

fn collect_item_signatures_from_items(
    items: &[ItemTreeNode],
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
        traits: HashMap::new(),
        trait_impls: Vec::new(),
        enums: HashMap::new(),
        type_aliases: HashMap::new(),
        globals: HashMap::new(),
        comptimes: HashMap::new(),
        diagnostics: Vec::new(),
    };
    for item in items {
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
    fn collect_item_into(&mut self, signatures: &mut ItemSignatures, item: &ItemTreeNode) {
        match &item.kind {
            ItemTreeNodeKind::Module(_) | ItemTreeNodeKind::Using(_) => {}
            ItemTreeNodeKind::Struct(item_struct) => {
                self.collect_struct(signatures, item, item_struct);
            }
            ItemTreeNodeKind::Union(item_union) => {
                self.collect_union(signatures, item, item_union);
            }
            ItemTreeNodeKind::Trait(item_trait) => {
                self.collect_trait(signatures, item, item_trait);
            }
            ItemTreeNodeKind::Extend(extend) => {
                self.collect_extend(signatures, extend);
            }
            ItemTreeNodeKind::Enum(item_enum) => {
                self.collect_enum(signatures, item, item_enum);
            }
            ItemTreeNodeKind::TypeAlias(alias) => {
                self.collect_type_alias(signatures, item, alias);
            }
            ItemTreeNodeKind::Function(_) => {
                self.collect_function(signatures, item);
            }
            ItemTreeNodeKind::Binding(binding) => {
                if binding.is_comptime {
                    self.collect_comptime(signatures, item, binding);
                } else {
                    self.collect_global(signatures, item, binding);
                }
            }
        }
    }

    fn collect_struct(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        item_struct: &StructItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::Struct) else {
            return;
        };
        let mut fields = Vec::new();
        for field in &item_struct.fields {
            let Some(field_id) =
                self.def_id_for_node(&field.node_key, field.span, DefKind::StructField)
            else {
                continue;
            };
            fields.push(FieldSignature {
                def_id: field_id,
                name: field.name.clone(),
                ty: self.ty_for_type(&field.ty),
                span: field.span,
            });
        }
        signatures.structs.insert(
            def_id,
            StructSignature {
                generics: item_struct.generics.clone(),
                where_predicates: self.where_predicate_signatures(&item_struct.where_clause),
                fields,
                is_extern: item_struct.is_extern,
                span: item.span,
            },
        );
    }

    fn collect_union(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        item_union: &UnionItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::Union) else {
            return;
        };
        let mut fields = Vec::new();
        for field in &item_union.fields {
            let Some(field_id) =
                self.def_id_for_node(&field.node_key, field.span, DefKind::UnionField)
            else {
                continue;
            };
            fields.push(FieldSignature {
                def_id: field_id,
                name: field.name.clone(),
                ty: self.ty_for_type(&field.ty),
                span: field.span,
            });
        }
        signatures.unions.insert(
            def_id,
            UnionSignature {
                generics: item_union.generics.clone(),
                where_predicates: self.where_predicate_signatures(&item_union.where_clause),
                fields,
                is_extern: item_union.is_extern,
                span: item.span,
            },
        );
    }

    fn collect_extend(&mut self, signatures: &mut ItemSignatures, extend: &ExtendItem) {
        let methods = extend
            .methods
            .iter()
            .filter_map(|method| {
                self.collect_method(signatures, &method.function)
                    .map(|def_id| TraitImplMethodSignature {
                        def_id,
                        name: method.function.name.clone(),
                        visibility: method.vis,
                        span: method.function.span,
                    })
            })
            .collect();
        let associated_values = extend
            .associated_values
            .iter()
            .filter_map(|associated_value| {
                self.collect_associated_comptime(signatures, associated_value)
                    .map(|def_id| TraitImplAssociatedValueSignature {
                        def_id,
                        name: associated_value.binding.name.clone(),
                        visibility: associated_value.vis,
                        span: associated_value.span,
                    })
            })
            .collect();
        signatures.trait_impls.push(TraitImplSignature {
            generics: extend.generics.clone(),
            target_ty: self.ty_for_type(&extend.target),
            trait_ty: extend
                .trait_ref
                .as_ref()
                .map(|trait_ref| self.ty_for_type(trait_ref)),
            trait_span: extend.trait_ref.as_ref().map(|trait_ref| trait_ref.span),
            where_predicates: self.where_predicate_signatures(&extend.where_clause),
            associated_types: extend
                .associated_types
                .iter()
                .map(|associated_type| TraitImplAssociatedTypeSignature {
                    name: associated_type.name.clone(),
                    ty: self.ty_for_type(&associated_type.ty),
                    span: associated_type.span,
                })
                .collect(),
            associated_values,
            methods,
            span: extend.target.span,
        });
    }

    fn collect_trait(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        item_trait: &TraitItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::Trait) else {
            return;
        };
        let mut associated_types = Vec::new();
        for associated_type in &item_trait.associated_types {
            let Some(associated_type_id) = self.def_id_for_node(
                &associated_type.node_key,
                associated_type.span,
                DefKind::TraitAssociatedType,
            ) else {
                continue;
            };
            associated_types.push(TraitAssociatedTypeSignature {
                def_id: associated_type_id,
                name: associated_type.name.clone(),
                span: associated_type.span,
            });
        }
        let mut methods = Vec::new();
        for method in &item_trait.methods {
            let Some(method_id) = self.def_id_for_node(
                &method.function.node_key,
                method.function.span,
                DefKind::TraitMethod,
            ) else {
                continue;
            };
            let signature = self.function_signature(&method.function);
            methods.push(TraitMethodSignature {
                def_id: method_id,
                name: method.function.name.clone(),
                signature: signature.clone(),
                has_default: method.function.body.is_some(),
                span: method.function.span,
            });
            signatures.functions.insert(method_id, signature);
        }
        signatures.traits.insert(
            def_id,
            TraitSignature {
                generics: item_trait.generics.clone(),
                where_predicates: self.where_predicate_signatures(&item_trait.where_clause),
                supertraits: item_trait
                    .supertraits
                    .iter()
                    .map(|supertrait| TraitSupertraitSignature {
                        ty: self.ty_for_type(supertrait),
                        span: supertrait.span,
                    })
                    .collect(),
                associated_types,
                methods,
                span: item.span,
            },
        );
    }

    fn collect_method(
        &mut self,
        signatures: &mut ItemSignatures,
        method: &FunctionItem,
    ) -> Option<DefId> {
        let Some(def_id) = self.def_id_for_node(&method.node_key, method.span, DefKind::Method)
        else {
            return None;
        };
        signatures
            .functions
            .insert(def_id, self.function_signature(method));
        Some(def_id)
    }

    fn collect_enum(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        item_enum: &EnumItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::Enum) else {
            return;
        };
        let backing_type = match &item_enum.backing_type {
            Some(ty) => self.ty_for_type(ty),
            None => self.primitive(PrimitiveTy::I32),
        };
        let mut variants = Vec::new();
        for variant in &item_enum.variants {
            let Some(variant_id) =
                self.def_id_for_node(&variant.node_key, variant.span, DefKind::EnumVariant)
            else {
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
                span: item.span,
            },
        );
    }

    fn collect_type_alias(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        alias: &TypeAliasItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&item.node_key, item.span, DefKind::TypeAlias)
        else {
            return;
        };
        signatures.type_aliases.insert(
            def_id,
            TypeAliasSignature {
                generics: alias.generics.clone(),
                target: self.ty_for_type(&alias.ty),
                span: item.span,
            },
        );
    }

    fn collect_function(&mut self, signatures: &mut ItemSignatures, item: &ItemTreeNode) {
        let ItemTreeNodeKind::Function(function) = &item.kind else {
            return;
        };
        let Some(def_id) =
            self.def_id_for_node(&function.node_key, function.span, DefKind::Function)
        else {
            return;
        };
        let attributes = self.function_attributes(&item.attributes, function);
        signatures.functions.insert(
            def_id,
            self.function_signature_with_attributes(function, attributes),
        );
    }

    fn collect_global(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        binding: &BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item.span, DefKind::Global)
        else {
            return;
        };
        signatures.globals.insert(
            def_id,
            GlobalSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_type(ty)),
                is_let: binding.is_let,
                is_extern: binding.is_extern,
                span: item.span,
            },
        );
    }

    fn collect_comptime(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        binding: &BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item.span, DefKind::Comptime)
        else {
            return;
        };
        signatures.comptimes.insert(
            def_id,
            ComptimeSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_type(ty)),
                span: item.span,
            },
        );
    }

    fn collect_associated_comptime(
        &mut self,
        signatures: &mut ItemSignatures,
        associated_value: &nia_ast::ExtendAssociatedValue,
    ) -> Option<DefId> {
        let binding = &associated_value.binding;
        let Some(def_id) =
            self.def_id_for_node(&binding.node_key, associated_value.span, DefKind::Comptime)
        else {
            return None;
        };
        signatures.comptimes.insert(
            def_id,
            ComptimeSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_type(ty)),
                span: associated_value.span,
            },
        );
        Some(def_id)
    }

    fn function_signature(&mut self, function: &FunctionItem) -> FunctionSignature {
        let params = function
            .params
            .iter()
            .map(|param| self.param_signature(param))
            .collect();
        let return_type = match &function.return_type {
            Some(ty) => self.ty_for_type(ty),
            None => self.primitive(PrimitiveTy::Void),
        };
        FunctionSignature {
            generics: function.generics.clone(),
            where_predicates: self.where_predicate_signatures(&function.where_clause),
            params,
            return_type,
            is_extern: function.is_extern,
            is_comptime: function.is_comptime,
            is_variadic: function.is_variadic,
            attributes: self.function_attributes(&[], function),
            has_body: function.body.is_some(),
            span: function.span,
        }
    }

    fn function_signature_with_attributes(
        &mut self,
        function: &FunctionItem,
        attributes: Vec<FunctionAttribute>,
    ) -> FunctionSignature {
        let mut signature = self.function_signature(function);
        signature.attributes = attributes;
        signature
    }

    fn function_attributes(
        &mut self,
        attributes: &[Attribute],
        function: &FunctionItem,
    ) -> Vec<FunctionAttribute> {
        let mut out = Vec::new();
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            match meta.path.as_slice() {
                [name] if name == "naked" => {
                    if !meta.args.is_empty() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            "E0203",
                            attribute.span,
                            "`@[naked]` does not take arguments",
                        ));
                    }
                    if !function.is_extern || function.body.is_none() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            "E0203",
                            attribute.span,
                            "`@[naked]` is only valid on `extern fn` definitions",
                        ));
                    }
                    out.push(FunctionAttribute::Naked);
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        "E0203",
                        attribute.span,
                        format!("unknown function attribute `@[{}]`", meta.path.join(".")),
                    ));
                }
            }
        }
        out
    }

    fn param_signature(&mut self, param: &Param) -> ParamSignature {
        let ty = match &param.ty {
            Some(ty) => self.ty_for_type(ty),
            None if param.receiver.is_some() => self.error(),
            None => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0203",
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

    fn where_predicate_signatures(&mut self, clause: &WhereClause) -> Vec<WherePredicateSignature> {
        clause
            .predicates
            .iter()
            .map(|predicate| WherePredicateSignature {
                ty: self.ty_for_type(&predicate.ty),
                bounds: predicate
                    .bounds
                    .iter()
                    .map(|bound| WhereBoundSignature {
                        trait_ty: self.ty_for_type(bound),
                        associated_type_bindings: self.associated_type_binding_signatures(bound),
                        span: bound.span,
                    })
                    .collect(),
                span: predicate.span,
            })
            .collect()
    }

    fn associated_type_binding_signatures(
        &mut self,
        bound: &nia_ast::TypeRef,
    ) -> Vec<AssociatedTypeBindingSignature> {
        let nia_ast::TypeKind::Path { segments } = &bound.kind else {
            return Vec::new();
        };
        let Some(segment) = segments.last() else {
            return Vec::new();
        };
        segment
            .args
            .iter()
            .filter_map(|arg| match arg {
                nia_ast::TypeArg::AssocBinding { key, ty, span } => {
                    let name = match key {
                        nia_ast::AssocBindingKey::Name(name) => name.clone(),
                        nia_ast::AssocBindingKey::Projection(projection) => {
                            let nia_ast::TypeKind::Projection { name, .. } = &projection.kind
                            else {
                                return None;
                            };
                            name.clone()
                        }
                    };
                    Some(AssociatedTypeBindingSignature {
                        name,
                        ty: self.ty_for_type(ty),
                        span: *span,
                    })
                }
                nia_ast::TypeArg::Type(_) | nia_ast::TypeArg::Const(_) => None,
            })
            .collect()
    }

    fn def_id_for_node(
        &mut self,
        node_key: &NodeKey,
        diagnostic_span: Span,
        expected: DefKind,
    ) -> Option<DefId> {
        let Some(def_id) = self.defs.def_nodes.get(node_key) else {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    "I0101",
                    "missing definition id while collecting item signature",
                )
                .primary(diagnostic_span, "this syntax node has no definition id")
                .debug("node_key", node_key)
                .debug("expected_def_kind", expected)
                .finish(),
            );
            return None;
        };
        let Some(def) = self.defs.defs.get(def_id) else {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    "I0102",
                    "definition id does not exist in definition map",
                )
                .primary(diagnostic_span, "definition map lookup failed here")
                .debug("node_key", node_key)
                .debug("def_id", def_id)
                .debug("expected_def_kind", expected)
                .finish(),
            );
            return None;
        };
        if def.kind != expected {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    "I0103",
                    "definition kind mismatch while collecting item signature",
                )
                .primary(def.span, "definition has an unexpected kind")
                .debug("node_key", node_key)
                .debug("def_id", def_id)
                .debug("expected_def_kind", expected)
                .debug("actual_def_kind", def.kind)
                .finish(),
            );
            return None;
        }
        Some(def_id)
    }

    fn ty_for_type(&mut self, ty_ref: &TypeRef) -> InternedTyId {
        if let Some(ty) = self.lowered.node_type_uses.get(&ty_ref.node_key).copied() {
            ty
        } else {
            self.diagnostics.push(
                Diagnostic::internal_error(
                    "I0104",
                    "missing lowered type while collecting item signature",
                )
                .primary(ty_ref.span, "this type reference was not lowered")
                .debug("node_key", &ty_ref.node_key)
                .finish(),
            );
            self.lowered.interner.error()
        }
    }

    fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.lowered.interner.primitive(primitive)
    }

    fn error(&self) -> InternedTyId {
        self.lowered.interner.error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs, collect_module_defs_from_active_item_tree};
    use nia_item_tree::ModuleItemTree;
    use nia_parser::parse_module;
    use nia_type_lower::lower_module_types;
    use nia_type_resolve::resolve_module_types;

    #[test]
    fn collects_item_signatures_without_checking_bodies() {
        let (module, errors) = parse_module(
            r#"
extern fn printf(fmt: &u8, ...);
extern let errno: i32;

struct Point {
    x: i32,
    y: i32,
}

extend Point {
    pub comptime let Origin: i32 = 0;
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
        assert_eq!(signatures.trait_impls.len(), 1);
        let impl_signature = &signatures.trait_impls[0];
        assert_eq!(impl_signature.methods.len(), 1);
        assert_eq!(impl_signature.methods[0].name, "len2");
        assert_eq!(
            impl_signature.methods[0].visibility,
            nia_ast::Visibility::Private
        );
        assert!(
            signatures
                .functions
                .contains_key(&impl_signature.methods[0].def_id)
        );
        assert_eq!(impl_signature.associated_values.len(), 1);
        assert_eq!(impl_signature.associated_values[0].name, "Origin");
        assert_eq!(
            impl_signature.associated_values[0].visibility,
            nia_ast::Visibility::Public
        );
        assert!(
            signatures
                .comptimes
                .contains_key(&impl_signature.associated_values[0].def_id)
        );
    }

    #[test]
    fn collects_item_signatures_from_active_item_tree_only() {
        let (module, errors) = parse_module(
            r#"
@[if false]
fn skipped() i32 { 0 }
@[if true]
fn selected() i32 { 1 }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let tree = ModuleItemTree::from_module(&module);
        let active = tree.active_items(&mut BoolResolver(false)).unwrap();
        let active_module = active.to_module();
        let defs = collect_module_defs_from_active_item_tree(ModuleId(0), &active);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_types(&active_module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        let lowered = lower_module_types(&active_module, &resolved);
        assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);

        let signatures = collect_item_signatures_from_active_item_tree(&active, &defs, &lowered);
        assert!(
            signatures.diagnostics.is_empty(),
            "{:?}",
            signatures.diagnostics
        );
        assert_eq!(signatures.functions.len(), 1);
        assert_eq!(active.items.len(), 1);
        assert!(matches!(
            &active_module.items[0].kind,
            nia_ast::ItemKind::Function(function) if function.name == "selected"
        ));
    }

    struct BoolResolver(bool);

    impl nia_item_tree::ConditionResolver for BoolResolver {
        fn resolve_condition(
            &mut self,
            cond: &nia_ast::ConditionExpr,
        ) -> Result<bool, nia_item_tree::ItemTreeError> {
            match &cond.kind {
                nia_ast::ConditionExprKind::Bool(value) => Ok(*value),
                _ => Ok(self.0),
            }
        }
    }
}
