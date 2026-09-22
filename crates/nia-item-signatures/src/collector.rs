//! AST and active-item-tree traversal that builds semantic signatures.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltinTypeDeclaration {
    Opaque(BuiltinType),
    Primitive(BuiltinTypeAnchor),
}

impl BuiltinTypeDeclaration {
    fn from_name(name: &str) -> Option<Self> {
        BuiltinType::from_name(name)
            .map(Self::Opaque)
            .or_else(|| BuiltinTypeAnchor::from_name(name).map(Self::Primitive))
    }
}

fn builtin_type_anchor_primitive(anchor: BuiltinTypeAnchor) -> PrimitiveTy {
    match anchor {
        BuiltinTypeAnchor::I8 => PrimitiveTy::I8,
        BuiltinTypeAnchor::I16 => PrimitiveTy::I16,
        BuiltinTypeAnchor::I32 => PrimitiveTy::I32,
        BuiltinTypeAnchor::I64 => PrimitiveTy::I64,
        BuiltinTypeAnchor::I128 => PrimitiveTy::I128,
        BuiltinTypeAnchor::Isize => PrimitiveTy::Isize,
        BuiltinTypeAnchor::U8 => PrimitiveTy::U8,
        BuiltinTypeAnchor::U16 => PrimitiveTy::U16,
        BuiltinTypeAnchor::U32 => PrimitiveTy::U32,
        BuiltinTypeAnchor::U64 => PrimitiveTy::U64,
        BuiltinTypeAnchor::U128 => PrimitiveTy::U128,
        BuiltinTypeAnchor::Usize => PrimitiveTy::Usize,
        BuiltinTypeAnchor::F32 => PrimitiveTy::F32,
        BuiltinTypeAnchor::F64 => PrimitiveTy::F64,
        BuiltinTypeAnchor::Bool => PrimitiveTy::Bool,
        BuiltinTypeAnchor::Char => PrimitiveTy::Char,
        BuiltinTypeAnchor::Never => PrimitiveTy::Never,
    }
}

/// Source representation from which signatures are collected.
#[derive(Debug, Clone, Copy)]
pub enum ItemSignatureSource<'a> {
    /// Parsed AST module.
    Module(&'a Module),
    /// Active item tree after conditional selection.
    ActiveItemTree(&'a ActiveModuleItemTree),
}

/// Inputs required to lower one module's declaration signatures.
#[derive(Clone, Copy)]
pub struct ItemSignatureInput<'a> {
    /// Source syntax or active item tree.
    pub source: ItemSignatureSource<'a>,
    /// Definition collection for the source module.
    pub defs: &'a DefCollection,
    /// Lowered source types.
    pub lowered: &'a TypeLowering,
    /// Session type store receiving signature types.
    pub type_store: &'a TypeStore,
    /// Optional symbol resolver used for diagnostic text.
    pub symbols: Option<&'a dyn SymbolText>,
}

impl std::fmt::Debug for ItemSignatureInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemSignatureInput")
            .field("source", &self.source)
            .field("module_id", &self.defs.module_id)
            .field("type_store", &self.type_store.id())
            .field("symbols", &self.symbols.is_some())
            .finish_non_exhaustive()
    }
}

/// Collects declaration signatures from the selected source representation.
pub fn collect_item_signatures(
    input: ItemSignatureInput<'_>,
) -> nia_ice::IceResult<ItemSignatures> {
    let append = input.type_store.append_for_module(input.defs.module_id);
    let collect = |items| {
        collect_item_signatures_from_items(
            items,
            input.defs,
            input.lowered,
            input.type_store,
            &append,
            input.symbols,
        )
    };
    match input.source {
        ItemSignatureSource::Module(module) => {
            let item_tree = ModuleItemTree::from_module(module);
            collect(&item_tree.items)
        }
        ItemSignatureSource::ActiveItemTree(item_tree) => collect(&item_tree.items),
    }
}

fn collect_item_signatures_from_items(
    items: &ItemTreeItems,
    defs: &DefCollection,
    lowered: &TypeLowering,
    type_store: &TypeStore,
    append: &TypeStoreAppend,
    symbols: Option<&dyn SymbolText>,
) -> nia_ice::IceResult<ItemSignatures> {
    let mut collector = SignatureCollector {
        defs,
        lowered,
        type_store,
        append,
        symbols,
        diagnostics: Vec::new(),
        internal_error: None,
        duplicate_impl_identities: HashMap::new(),
        impl_identities_by_id: HashMap::new(),
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
        consts: HashMap::new(),
        diagnostics: Vec::new(),
    };
    for item in items.iter() {
        collector.collect_item_into(&mut signatures, item);
    }
    if let Some(error) = collector.internal_error {
        return Err(error);
    }
    signatures.diagnostics = collector.diagnostics;
    Ok(signatures)
}

struct SignatureCollector<'a> {
    defs: &'a DefCollection,
    lowered: &'a TypeLowering,
    type_store: &'a TypeStore,
    append: &'a TypeStoreAppend,
    symbols: Option<&'a dyn SymbolText>,
    diagnostics: Vec<Diagnostic>,
    internal_error: Option<nia_ice::Ice>,
    duplicate_impl_identities: HashMap<TraitImplIdentity, u32>,
    impl_identities_by_id: HashMap<TraitImplId, TraitImplIdentity>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FunctionAttributeContext {
    TopLevel,
    TraitMethod,
    ExtensionMethod,
}

impl<'a> SignatureCollector<'a> {
    fn symbol_debug_text(&self, symbol: SymbolId) -> String {
        symbol_debug_text_with_symbols(self.symbols, symbol)
    }

    fn attribute_path_text(&self, path: &[SymbolId]) -> String {
        attribute_path_text_with_symbols(path, self.symbols)
    }

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
                self.collect_extend(signatures, item, extend);
            }
            ItemTreeNodeKind::Enum(item_enum) => {
                self.collect_enum(signatures, item, item_enum);
            }
            ItemTreeNodeKind::TypeAlias(alias) => {
                self.collect_type_alias(signatures, item, alias);
            }
            ItemTreeNodeKind::Function(function) => {
                self.collect_function(signatures, item);
                self.collect_function_local_static_signatures(signatures, function);
            }
            ItemTreeNodeKind::Binding(binding) => {
                if binding.is_const() {
                    self.collect_const(signatures, item, binding);
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
                name: field.name,
                ty: self.ty_for_type(&field.ty),
                span: field.span,
            });
        }
        signatures.structs.insert(
            def_id,
            StructSignature {
                generics: generic_signature_names(&item_struct.generics),
                generic_params: self.generic_param_signatures(&item_struct.generics),
                where_predicates: self.where_predicate_signatures(&item_struct.where_clause),
                fields,
                is_tuple: item_struct.is_tuple,
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
                name: field.name,
                ty: self.ty_for_type(&field.ty),
                span: field.span,
            });
        }
        signatures.unions.insert(
            def_id,
            UnionSignature {
                generics: generic_signature_names(&item_union.generics),
                generic_params: self.generic_param_signatures(&item_union.generics),
                where_predicates: self.where_predicate_signatures(&item_union.where_clause),
                fields,
                is_extern: item_union.is_extern,
                span: item.span,
            },
        );
    }

    fn collect_extend(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        extend: &ExtendItem,
    ) {
        let methods = extend
            .methods
            .iter()
            .filter_map(|method| {
                self.check_method_generic_shadowing(&extend.generics, &method.function);
                self.collect_method(signatures, &method.attributes, &method.function)
                    .map(|def_id| TraitImplMethodSignature {
                        def_id,
                        name: method.function.name,
                        visibility: method.vis,
                        span: method.function.span,
                    })
            })
            .collect();
        let associated_values = extend
            .associated_values
            .iter()
            .filter_map(|associated_value| {
                self.collect_associated_const(signatures, associated_value)
                    .map(|def_id| TraitImplAssociatedValueSignature {
                        def_id,
                        name: associated_value.binding.name,
                        visibility: associated_value.vis,
                        span: associated_value.span,
                    })
            })
            .collect();
        let Some(impl_id) = self.trait_impl_id(extend) else {
            return;
        };
        signatures.trait_impls.push(TraitImplSignature {
            impl_id,
            builtin: self.builtin_extend_attribute(&item.attributes),
            generics: generic_signature_names(&extend.generics),
            generic_params: self.generic_param_signatures(&extend.generics),
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
                    name: associated_type.name,
                    ty: self.ty_for_type(&associated_type.ty),
                    span: associated_type.span,
                })
                .collect(),
            associated_values,
            methods,
            span: extend.target.span,
        });
    }

    fn trait_impl_id(&mut self, extend: &ExtendItem) -> Option<TraitImplId> {
        let identity = TraitImplIdentity::from_extend(extend);
        let ordinal = {
            let next = self
                .duplicate_impl_identities
                .entry(identity.clone())
                .or_default();
            let ordinal = *next;
            let Some(updated) = ordinal.checked_add(1) else {
                self.record_internal(nia_ice::Ice::new(format!(
                    "trait implementation duplicate ordinal exhausted for `{}`",
                    identity.display()
                )));
                return None;
            };
            *next = updated;
            ordinal
        };
        let resolved = if ordinal == 0 {
            identity
        } else {
            identity.duplicate(ordinal)
        };
        let impl_id = TraitImplId(stable_trait_impl_id(&resolved));
        if let Some(existing) = self.impl_identities_by_id.get(&impl_id) {
            self.record_internal(nia_ice::Ice::new(format!(
                "stable trait implementation ID collision between `{}` and `{}`",
                existing.display(),
                resolved.display()
            )));
            return None;
        }
        self.impl_identities_by_id.insert(impl_id, resolved);
        Some(impl_id)
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
                name: associated_type.name,
                span: associated_type.span,
            });
        }
        let mut associated_values = Vec::new();
        for associated_value in &item_trait.associated_values {
            let Some(associated_value_id) = self.def_id_for_node(
                &associated_value.node_key,
                associated_value.span,
                DefKind::Const,
            ) else {
                continue;
            };
            associated_values.push(TraitAssociatedValueSignature {
                def_id: associated_value_id,
                name: associated_value.name,
                ty: self.ty_for_type(&associated_value.ty),
                span: associated_value.span,
            });
        }
        let mut methods = Vec::new();
        for method in &item_trait.methods {
            self.check_method_generic_shadowing(&item_trait.generics, &method.function);
            let Some(method_id) = self.def_id_for_node(
                &method.function.node_key,
                method.function.span,
                DefKind::TraitMethod,
            ) else {
                continue;
            };
            let attributes = self.function_attributes(
                &method.attributes,
                &method.function,
                FunctionAttributeContext::TraitMethod,
            );
            let signature = self.function_signature_with_attributes(
                &method.function,
                &method.attributes,
                attributes,
            );
            methods.push(TraitMethodSignature {
                def_id: method_id,
                name: method.function.name,
                signature: signature.clone(),
                has_default: method.function.body.is_some(),
                span: method.function.span,
            });
            signatures.functions.insert(method_id, signature);
        }
        signatures.traits.insert(
            def_id,
            TraitSignature {
                generics: generic_signature_names(&item_trait.generics),
                generic_params: self.generic_param_signatures(&item_trait.generics),
                where_predicates: self.where_predicate_signatures(&item_trait.where_clause),
                supertraits: item_trait
                    .supertraits
                    .iter()
                    .map(|supertrait| TraitSupertraitSignature {
                        ty: self.ty_for_type(supertrait),
                        associated_type_bindings: self
                            .associated_type_binding_signatures(supertrait),
                        span: supertrait.span,
                    })
                    .collect(),
                associated_types,
                associated_values,
                methods,
                builtin: self.builtin_trait_attribute(&item.attributes),
                span: item.span,
            },
        );
    }

    fn collect_method(
        &mut self,
        signatures: &mut ItemSignatures,
        source_attributes: &[Attribute],
        method: &FunctionItem,
    ) -> Option<DefId> {
        let def_id = self.def_id_for_node(&method.node_key, method.span, DefKind::Method)?;
        let parsed_attributes = self.function_attributes(
            source_attributes,
            method,
            FunctionAttributeContext::ExtensionMethod,
        );
        signatures.functions.insert(
            def_id,
            self.function_signature_with_attributes(method, source_attributes, parsed_attributes),
        );
        Some(def_id)
    }

    fn check_method_generic_shadowing(
        &mut self,
        enclosing_generics: &[GenericParam],
        method: &FunctionItem,
    ) {
        let enclosing_names = enclosing_generics
            .iter()
            .map(|generic| generic.name)
            .collect::<std::collections::HashSet<_>>();
        for generic in &method.generics {
            if enclosing_names.contains(&generic.name) {
                let name = self.symbol_debug_text(generic.name);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::ITEM_SIGNATURE,
                    generic.name_span,
                    format!(
                        "method generic parameter cannot shadow enclosing generic parameter `{name}`"
                    ),
                ));
            }
        }
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
            None => self.primitive(PrimitiveTy::U8),
        };
        let mut variants = Vec::new();
        for variant in &item_enum.variants {
            let Some(variant_id) =
                self.def_id_for_node(&variant.node_key, variant.span, DefKind::EnumVariant)
            else {
                continue;
            };
            let payload = match &variant.payload {
                EnumVariantPayload::Unit => EnumVariantPayloadSignature::Unit,
                EnumVariantPayload::Tuple(fields) => EnumVariantPayloadSignature::Tuple(
                    fields.iter().map(|field| self.ty_for_type(field)).collect(),
                ),
                EnumVariantPayload::Named(fields) => EnumVariantPayloadSignature::Named(
                    fields
                        .iter()
                        .filter_map(|field| {
                            let def_id = self.def_id_for_node(
                                &field.node_key,
                                field.span,
                                DefKind::EnumVariantField,
                            )?;
                            Some(FieldSignature {
                                def_id,
                                name: field.name,
                                ty: self.ty_for_type(&field.ty),
                                span: field.span,
                            })
                        })
                        .collect(),
                ),
            };
            variants.push(EnumVariantSignature {
                def_id: variant_id,
                name: variant.name,
                payload,
                span: variant.span,
            });
        }
        if item_enum.is_open
            && variants
                .iter()
                .any(|variant| !matches!(variant.payload, EnumVariantPayloadSignature::Unit))
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::STATIC_CHECK,
                item.span,
                "payload enum cannot use the open enum marker",
            ));
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
                generics: generic_signature_names(&alias.generics),
                generic_params: self.generic_param_signatures(&alias.generics),
                target: self.type_alias_target(item, alias),
                span: item.span,
            },
        );
    }

    fn type_alias_target(&mut self, item: &ItemTreeNode, alias: &TypeAliasItem) -> InternedTyId {
        if let Some(ty) = &alias.ty {
            return self.ty_for_type(ty);
        }
        let Some(builtin) = self.builtin_type_attribute(&item.attributes) else {
            self.record_internal(nia_ice::Ice::new(format!(
                "bodyless type alias without valid builtin attribute reached item signatures at {:?}",
                item.span
            )));
            return self.error();
        };
        match builtin {
            BuiltinTypeDeclaration::Opaque(builtin) => self.intern(TyKind::BuiltinType(builtin)),
            BuiltinTypeDeclaration::Primitive(anchor) => {
                self.primitive(builtin_type_anchor_primitive(anchor))
            }
        }
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
        let attributes = self.function_attributes(
            &item.attributes,
            function,
            FunctionAttributeContext::TopLevel,
        );
        signatures.functions.insert(
            def_id,
            self.function_signature_with_attributes(function, &item.attributes, attributes),
        );
    }

    fn collect_function_local_static_signatures(
        &mut self,
        signatures: &mut ItemSignatures,
        function: &FunctionItem,
    ) {
        let Some(body) = &function.body else {
            return;
        };
        self.collect_block_static_signatures(signatures, body);
    }

    fn collect_block_static_signatures(&mut self, signatures: &mut ItemSignatures, block: &Block) {
        nia_ast_walk::walk_static_bindings(block, &mut |stmt| {
            let StmtKind::Static(binding) = &stmt.kind else {
                return;
            };
            let Some(def_id) = self.def_id_for_node(&binding.node_key, stmt.span, DefKind::Global)
            else {
                return;
            };
            let explicit_type = binding.ty.as_ref().map(|ty| self.ty_for_type(ty));
            signatures.globals.insert(
                def_id,
                GlobalSignature {
                    explicit_type,
                    is_mutable: binding.is_mutable(),
                    is_extern: false,
                    external_name: None,
                    span: stmt.span,
                },
            );
        });
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
        self.validate_global_attributes(&item.attributes);
        signatures.globals.insert(
            def_id,
            GlobalSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_type(ty)),
                is_mutable: binding.is_mutable(),
                is_extern: binding.is_extern(),
                external_name: self.external_name_attribute(
                    &item.attributes,
                    binding.is_extern(),
                    false,
                    "static",
                ),
                span: item.span,
            },
        );
    }

    fn validate_global_attributes(&mut self, attributes: &[Attribute]) {
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            match meta.path.as_slice() {
                [name] if *name == known::LINK_NAME || *name == known::EXPORT_NAME => {}
                [name] if *name == known::NO_MANGLE => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        "`@[noMangle]` is not supported; use an `extern static` declaration and optional `@[linkName]`",
                    ));
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        format!(
                            "unknown static attribute `@[{}]`",
                            self.attribute_path_text(&meta.path)
                        ),
                    ));
                }
            }
        }
    }

    fn collect_const(
        &mut self,
        signatures: &mut ItemSignatures,
        item: &ItemTreeNode,
        binding: &BindingItem,
    ) {
        let Some(def_id) = self.def_id_for_node(&binding.node_key, item.span, DefKind::Const)
        else {
            return;
        };
        signatures.consts.insert(
            def_id,
            ConstSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_type(ty)),
                builtin: self.builtin_const_attribute(&item.attributes, binding),
                span: item.span,
            },
        );
    }

    fn collect_associated_const(
        &mut self,
        signatures: &mut ItemSignatures,
        associated_value: &nia_ast::ExtendAssociatedValue,
    ) -> Option<DefId> {
        let binding = &associated_value.binding;
        let def_id =
            self.def_id_for_node(&binding.node_key, associated_value.span, DefKind::Const)?;
        signatures.consts.insert(
            def_id,
            ConstSignature {
                explicit_type: binding.ty.as_ref().map(|ty| self.ty_for_type(ty)),
                builtin: None,
                span: associated_value.span,
            },
        );
        Some(def_id)
    }

    fn builtin_const_attribute(
        &mut self,
        attributes: &[Attribute],
        binding: &BindingItem,
    ) -> Option<BuiltinConstValue> {
        let mut out = None;
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            match meta.path.as_slice() {
                [name] if *name == known::BUILTIN => {
                    let builtin_name =
                        self.parse_builtin_attribute_name(attribute, meta.args.as_slice());
                    if let Some(builtin_name) = builtin_name {
                        if let Some(builtin) = BuiltinConstValue::from_name(builtin_name.as_str()) {
                            if out.replace(builtin).is_some() {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::ITEM_SIGNATURE,
                                    attribute.span,
                                    "duplicate `@[builtin]` const attribute",
                                ));
                            }
                            if builtin_const_item_symbol(builtin) != binding.name {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::ITEM_SIGNATURE,
                                    attribute.span,
                                    format!(
                                        "builtin const source item `{}` must match descriptor item `{}`",
                                        self.symbol_debug_text(binding.name),
                                        builtin.item_name()
                                    ),
                                ));
                            }
                        } else {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::ITEM_SIGNATURE,
                                attribute.span,
                                format!("unknown builtin const `{builtin_name}`"),
                            ));
                        }
                    }
                    if binding.is_extern() || binding.value.is_some() || binding.ty.is_none() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "`@[builtin]` is only valid on bodyless non-extern const declarations with an explicit type",
                        ));
                    }
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        format!(
                            "unknown const attribute `@[{}]`",
                            self.attribute_path_text(&meta.path)
                        ),
                    ));
                }
            }
        }
        out
    }

    fn function_signature(&mut self, function: &FunctionItem) -> FunctionSignature {
        let params = function
            .params
            .iter()
            .map(|param| self.param_signature(param))
            .collect();
        let return_type = match &function.return_type {
            Some(ty) => self.ty_for_type(ty),
            None => self.intern(TyKind::Tuple(Vec::new())),
        };
        FunctionSignature {
            name: function.name,
            generics: generic_signature_names(&function.generics),
            generic_params: self.generic_param_signatures(&function.generics),
            where_predicates: self.where_predicate_signatures(&function.where_clause),
            params,
            return_type,
            is_extern: function.is_extern,
            external_name: None,
            is_const: function.is_const,
            is_variadic: function.is_variadic,
            attributes: Vec::new(),
            has_body: function.body.is_some(),
            span: function.span,
        }
    }

    fn generic_param_signatures(
        &mut self,
        generics: &[GenericParam],
    ) -> Vec<GenericParamSignature> {
        generics
            .iter()
            .map(|generic| GenericParamSignature {
                name: generic.name,
                kind: match &generic.kind {
                    GenericParamKind::Type => GenericParamSignatureKind::Type,
                    GenericParamKind::Const { ty } => GenericParamSignatureKind::Const {
                        ty: self.ty_for_type(ty),
                    },
                },
            })
            .collect()
    }

    fn function_signature_with_attributes(
        &mut self,
        function: &FunctionItem,
        source_attributes: &[Attribute],
        attributes: Vec<FunctionAttribute>,
    ) -> FunctionSignature {
        let mut signature = self.function_signature(function);
        signature.attributes = attributes;
        signature.external_name = self.external_name_attribute(
            source_attributes,
            function.is_extern,
            function.body.is_some(),
            "function",
        );
        signature
    }

    fn function_attributes(
        &mut self,
        attributes: &[Attribute],
        function: &FunctionItem,
        context: FunctionAttributeContext,
    ) -> Vec<FunctionAttribute> {
        let mut out = Vec::new();
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            match meta.path.as_slice() {
                [name] if *name == known::NAKED => {
                    if !meta.args.is_empty() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "`@[naked]` does not take arguments",
                        ));
                    }
                    if !function.is_extern || function.body.is_none() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "`@[naked]` is only valid on `extern fn` definitions",
                        ));
                    }
                    out.push(FunctionAttribute::Naked);
                }
                [name] if *name == known::BUILTIN => {
                    let builtin_name =
                        self.parse_builtin_attribute_name(attribute, meta.args.as_slice());
                    if let Some(builtin_name) = builtin_name {
                        if let Some(builtin) = BuiltinFunction::from_name(builtin_name.as_str()) {
                            out.push(FunctionAttribute::Builtin(builtin));
                        } else {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::ITEM_SIGNATURE,
                                attribute.span,
                                format!("unknown builtin function `{builtin_name}`"),
                            ));
                        }
                    }
                    if function.is_extern || function.body.is_some() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "`@[builtin]` is only valid on bodyless non-extern function declarations",
                        ));
                    }
                }
                [name] if *name == known::TRACK_CALLER => {
                    if !meta.args.is_empty() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "`@[trackCaller]` does not take arguments",
                        ));
                    }
                    if function.is_extern {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "`@[trackCaller]` is not valid on `extern fn`",
                        ));
                    }
                    if out
                        .iter()
                        .any(|attribute| matches!(attribute, FunctionAttribute::TrackCaller))
                    {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            attribute.span,
                            "duplicate `@[trackCaller]` function attribute",
                        ));
                    } else {
                        out.push(FunctionAttribute::TrackCaller);
                    }
                }
                [name] if *name == known::LINK_NAME || *name == known::EXPORT_NAME => {}
                [name] if *name == known::NO_MANGLE => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        "`@[noMangle]` is not supported; use an `extern fn` definition and optional `@[exportName]`",
                    ));
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        format!(
                            "unknown function attribute `@[{}]`",
                            self.attribute_path_text(&meta.path)
                        ),
                    ));
                }
            }
        }
        if context == FunctionAttributeContext::TopLevel
            && !function.is_extern
            && function.body.is_none()
            && !out
                .iter()
                .any(|attribute| matches!(attribute, FunctionAttribute::Builtin(_)))
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::ITEM_SIGNATURE,
                function.span,
                "bodyless non-extern functions require `@[builtin]`",
            ));
        }
        out
    }

    fn external_name_attribute(
        &mut self,
        attributes: &[Attribute],
        is_extern: bool,
        has_body: bool,
        kind: &'static str,
    ) -> Option<String> {
        let mut name = None;
        let mut seen = false;
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            let (attribute_name, is_link) = if meta.path.as_slice() == [known::LINK_NAME] {
                ("linkName", true)
            } else if meta.path.as_slice() == [known::EXPORT_NAME] {
                ("exportName", false)
            } else {
                continue;
            };
            let duplicate = seen;
            if duplicate {
                self.diagnostics.push(
                    Diagnostic::user_error(
                        codes::ITEM_SIGNATURE,
                        format!("duplicate external symbol attribute on {kind}"),
                    )
                    .primary(
                        attribute.span,
                        format!("duplicate `@[{attribute_name}]` attribute"),
                    )
                    .help(format!(
                        "keep only one external symbol attribute on this {kind}"
                    ))
                    .finish(),
                );
            }
            seen = true;
            if !is_extern {
                self.diagnostics.push(
                    Diagnostic::user_error(
                        codes::ITEM_SIGNATURE,
                        format!("`@[{attribute_name}]` requires an `extern` {kind}"),
                    )
                    .primary(
                        attribute.span,
                        format!("this {kind} is not declared `extern`"),
                    )
                    .help(format!(
                        "add `extern` to this {kind}, or remove `@[{attribute_name}]`"
                    ))
                    .finish(),
                );
            } else if is_link && has_body {
                self.diagnostics.push(
                    Diagnostic::user_error(
                        codes::ITEM_SIGNATURE,
                        format!("`@[linkName]` is only valid on an external {kind} declaration"),
                    )
                    .primary(attribute.span, "this declaration has a body".to_string())
                    .help("use `@[exportName]` for a definition with a body")
                    .finish(),
                );
            } else if !is_link && !has_body {
                self.diagnostics.push(
                    Diagnostic::user_error(
                        codes::ITEM_SIGNATURE,
                        format!("`@[exportName]` requires an extern {kind} definition"),
                    )
                    .primary(attribute.span, "this declaration has no body".to_string())
                    .help("add a body to define this symbol, or use `@[linkName]` for an import")
                    .finish(),
                );
            }
            if let Some(value) = self.parse_external_name(attribute, meta.args.as_slice())
                && !duplicate
            {
                name = Some(value);
            }
        }
        name
    }

    fn parse_external_name(
        &mut self,
        attribute: &Attribute,
        args: &[nia_ast::Expr],
    ) -> Option<String> {
        match args {
            [arg] => match &arg.kind {
                nia_ast::ExprKind::String(text) => {
                    let value = nia_literals::eval_string_literal_parts(
                        text.parts.iter().map(String::as_str),
                    );
                    if value
                        .as_deref()
                        .is_none_or(|value| value.is_empty() || value.contains('\0'))
                    {
                        self.diagnostics.push(
                            Diagnostic::user_error(
                                codes::ITEM_SIGNATURE,
                                "external symbol name must be non-empty and contain no NUL bytes",
                            )
                            .primary(arg.span, "invalid external symbol name")
                            .help("use a non-empty string without NUL bytes")
                            .finish(),
                        );
                        None
                    } else {
                        value
                    }
                }
                _ => {
                    self.diagnostics.push(
                        Diagnostic::user_error(
                            codes::ITEM_SIGNATURE,
                            "external symbol attribute expects one string literal",
                        )
                        .primary(arg.span, "external symbol name is not a string literal")
                        .help("pass exactly one string literal to this attribute")
                        .finish(),
                    );
                    None
                }
            },
            _ => {
                self.diagnostics.push(
                    Diagnostic::user_error(
                        codes::ITEM_SIGNATURE,
                        "external symbol attribute expects exactly one string literal",
                    )
                    .primary(
                        attribute.span,
                        "external symbol attribute arguments are invalid",
                    )
                    .help("pass exactly one string literal to this attribute")
                    .finish(),
                );
                None
            }
        }
    }

    fn builtin_trait_attribute(&mut self, attributes: &[Attribute]) -> Option<BuiltinTrait> {
        let mut out = None;
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            match meta.path.as_slice() {
                [name] if *name == known::BUILTIN => {
                    let builtin_name =
                        self.parse_builtin_attribute_name(attribute, meta.args.as_slice());
                    if let Some(builtin_name) = builtin_name {
                        if let Some(builtin) = BuiltinTrait::from_name(builtin_name.as_str()) {
                            if out.replace(builtin).is_some() {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::ITEM_SIGNATURE,
                                    attribute.span,
                                    "duplicate `@[builtin]` trait attribute",
                                ));
                            }
                        } else {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::ITEM_SIGNATURE,
                                attribute.span,
                                format!("unknown builtin trait `{builtin_name}`"),
                            ));
                        }
                    }
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        format!(
                            "unknown trait attribute `@[{}]`",
                            self.attribute_path_text(&meta.path)
                        ),
                    ));
                }
            }
        }
        out
    }

    fn builtin_type_attribute(
        &mut self,
        attributes: &[Attribute],
    ) -> Option<BuiltinTypeDeclaration> {
        let mut out = None;
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            match meta.path.as_slice() {
                [name] if *name == known::BUILTIN => {
                    let builtin_name =
                        self.parse_builtin_attribute_name(attribute, meta.args.as_slice());
                    if let Some(builtin_name) = builtin_name {
                        if let Some(builtin) =
                            BuiltinTypeDeclaration::from_name(builtin_name.as_str())
                        {
                            if out.replace(builtin).is_some() {
                                self.diagnostics.push(Diagnostic::user_error_at(
                                    codes::ITEM_SIGNATURE,
                                    attribute.span,
                                    "duplicate `@[builtin]` type attribute",
                                ));
                            }
                        } else {
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::ITEM_SIGNATURE,
                                attribute.span,
                                format!("unknown builtin type `{builtin_name}`"),
                            ));
                        }
                    }
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        attribute.span,
                        format!(
                            "unknown type attribute `@[{}]`",
                            self.attribute_path_text(&meta.path)
                        ),
                    ));
                }
            }
        }
        out
    }

    fn builtin_extend_attribute(&mut self, attributes: &[Attribute]) -> Option<String> {
        let mut out = None;
        for attribute in attributes {
            let AttributeKind::Meta(meta) = &attribute.kind else {
                continue;
            };
            if meta.path.as_slice() != [known::BUILTIN] {
                continue;
            }
            if let Some(builtin_name) =
                self.parse_builtin_attribute_name(attribute, meta.args.as_slice())
                && out.replace(builtin_name).is_some()
            {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::ITEM_SIGNATURE,
                    attribute.span,
                    "duplicate `@[builtin]` extend attribute",
                ));
            }
        }
        out
    }

    fn parse_builtin_attribute_name(
        &mut self,
        attribute: &Attribute,
        args: &[nia_ast::Expr],
    ) -> Option<String> {
        match args {
            [arg] => match &arg.kind {
                nia_ast::ExprKind::String(text) => {
                    let name = nia_literals::eval_string_literal_parts(
                        text.parts.iter().map(String::as_str),
                    );
                    if name.is_none() {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::ITEM_SIGNATURE,
                            arg.span,
                            "`@[builtin]` expects a valid string literal name",
                        ));
                    }
                    name
                }
                _ => {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::ITEM_SIGNATURE,
                        arg.span,
                        "`@[builtin]` expects a single string literal name",
                    ));
                    None
                }
            },
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::ITEM_SIGNATURE,
                    attribute.span,
                    "`@[builtin]` expects exactly one string literal name",
                ));
                None
            }
        }
    }

    fn param_signature(&mut self, param: &Param) -> ParamSignature {
        let ty = match &param.ty {
            Some(ty) => self.ty_for_type(ty),
            None if param.receiver.is_some() => self.error(),
            None => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::ITEM_SIGNATURE,
                    param.span,
                    "parameter requires an explicit type",
                ));
                self.error()
            }
        };
        ParamSignature {
            name: param.name,
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
                        nia_ast::AssocBindingKey::Name(name) => *name,
                        nia_ast::AssocBindingKey::Projection(projection) => {
                            let nia_ast::TypeKind::Projection { name, .. } = &projection.kind
                            else {
                                return None;
                            };
                            *name
                        }
                    };
                    Some(AssociatedTypeBindingSignature {
                        name,
                        ty: self.ty_for_type(ty),
                        span: *span,
                    })
                }
                nia_ast::TypeArg::Type(_)
                | nia_ast::TypeArg::Const(_)
                | nia_ast::TypeArg::TypeOrConst { .. } => None,
            })
            .collect()
    }

    fn def_id_for_node(
        &mut self,
        node_key: &VersionedNodeKey,
        diagnostic_span: Span,
        expected: DefKind,
    ) -> Option<DefId> {
        let Some(def_id) = self.defs.def_nodes.get(node_key) else {
            self.record_internal(nia_ice::Ice::new(format!(
                "missing definition ID while collecting item signature at {diagnostic_span:?}: node {node_key:?}, expected {expected:?}"
            )));
            return None;
        };
        let Some(def) = self.defs.defs.get(def_id) else {
            self.record_internal(nia_ice::Ice::new(format!(
                "definition ID {def_id:?} does not exist while collecting item signature at {diagnostic_span:?}: node {node_key:?}, expected {expected:?}"
            )));
            return None;
        };
        if def.kind != expected {
            let actual = def.kind;
            let span = def.span;
            self.record_internal(nia_ice::Ice::new(format!(
                "definition kind mismatch while collecting item signature at {span:?}: node {node_key:?}, definition {def_id:?}, expected {expected:?}, found {actual:?}"
            )));
            return None;
        }
        Some(def_id)
    }

    fn ty_for_type(&mut self, ty_ref: &TypeRef) -> InternedTyId {
        if let Some(ty) = self.lowered.ty_for_key(&ty_ref.node_key) {
            if self.type_store.get(ty).is_some() {
                ty
            } else {
                self.record_internal(nia_ice::Ice::new(format!(
                    "lowered type {ty:?} is outside the session type store at {:?}: node {:?}",
                    ty_ref.span, ty_ref.node_key
                )));
                self.error()
            }
        } else {
            self.record_internal(nia_ice::Ice::new(format!(
                "missing lowered type while collecting item signature at {:?}: node {:?}",
                ty_ref.span, ty_ref.node_key
            )));
            self.error()
        }
    }

    fn record_internal(&mut self, error: nia_ice::Ice) {
        if self.internal_error.is_none() {
            self.internal_error = Some(error);
        }
    }

    fn intern(&mut self, kind: TyKind) -> InternedTyId {
        if self.internal_error.is_some() {
            return self.type_store.error();
        }
        match self.append.intern(kind) {
            Ok(ty) => ty,
            Err(error) => {
                self.record_internal(error);
                self.type_store.error()
            }
        }
    }

    fn primitive(&mut self, primitive: PrimitiveTy) -> InternedTyId {
        self.intern(TyKind::Primitive(primitive))
    }

    fn error(&self) -> InternedTyId {
        self.type_store.error()
    }
}
