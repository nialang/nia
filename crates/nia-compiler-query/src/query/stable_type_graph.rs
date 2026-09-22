pub(super) fn stable_primitive_from_tag(tag: u8) -> Option<nia_ty::PrimitiveTy> {
    use nia_ty::PrimitiveTy;
    Some(match tag {
        1 => PrimitiveTy::I8,
        2 => PrimitiveTy::I16,
        3 => PrimitiveTy::I32,
        4 => PrimitiveTy::I64,
        5 => PrimitiveTy::I128,
        6 => PrimitiveTy::Isize,
        7 => PrimitiveTy::U8,
        8 => PrimitiveTy::U16,
        9 => PrimitiveTy::U32,
        10 => PrimitiveTy::U64,
        11 => PrimitiveTy::U128,
        12 => PrimitiveTy::Usize,
        13 => PrimitiveTy::F32,
        14 => PrimitiveTy::F64,
        15 => PrimitiveTy::Bool,
        16 => PrimitiveTy::Char,
        _ => return None,
    })
}
use super::*;

pub(super) struct StableTypeGraphEncoder<'db> {
    pub(super) db: &'db QueryDb<CompilerContext>,
    pub(super) graph: &'db ModuleGraphSnapshot,
    pub(super) symbols: &'db nia_symbol_table::SymbolTable,
    pub(super) resolver: &'db dyn StableDefinitionPackageResolver,
    pub(super) indexes: HashMap<nia_ids::InternedTyId, u32>,
    pub(super) visiting: HashSet<nia_ids::InternedTyId>,
    pub(super) key_cache: HashMap<nia_ids::InternedTyId, Vec<u8>>,
    pub(super) key_visiting: HashSet<nia_ids::InternedTyId>,
    pub(super) canonical_indexes: HashMap<Vec<u8>, u32>,
    pub(super) nodes: Vec<StableTypeNode>,
}

impl StableTypeGraphEncoder<'_> {
    pub(super) fn encode(&mut self, ty: nia_ids::InternedTyId) -> QueryResult<u32> {
        if let Some(index) = self.indexes.get(&ty) {
            return Ok(*index);
        }
        let key = self.canonical_key(ty)?;
        if !self.visiting.insert(ty) {
            return Err(self.db.invalid_input(
                &ModuleGraphQuery,
                "recursive session type graph has no stable package representation".to_string(),
            ));
        }
        let kind = self
            .db
            .context()
            .type_store
            .get(ty)
            .cloned()
            .ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "type root belongs to a different compiler session".to_string(),
                )
            })?;
        let node = match kind {
            nia_ty::TyKind::Primitive(primitive) => primitive_type_node(primitive)?,
            nia_ty::TyKind::Tuple(elements) if elements.is_empty() => StableTypeNode::Unit,
            nia_ty::TyKind::Tuple(elements) => StableTypeNode::Tuple(
                elements
                    .into_iter()
                    .map(|element| self.encode(element))
                    .collect::<QueryResult<Vec<_>>>()?,
            ),
            nia_ty::TyKind::Pointer {
                is_readonly, elem, ..
            } => StableTypeNode::Pointer {
                target: self.encode(elem)?,
                readonly: is_readonly,
            },
            nia_ty::TyKind::Error => StableTypeNode::Error,
            nia_ty::TyKind::ConstOnly => StableTypeNode::ConstOnly,
            nia_ty::TyKind::Opaque => StableTypeNode::Opaque,
            nia_ty::TyKind::VolatilePointer { is_readonly, elem } => {
                StableTypeNode::VolatilePointer {
                    target: self.encode(elem)?,
                    readonly: is_readonly,
                }
            }
            nia_ty::TyKind::Slice { is_readonly, elem } => StableTypeNode::Slice {
                target: self.encode(elem)?,
                readonly: is_readonly,
            },
            nia_ty::TyKind::SlicePointee { elem } => StableTypeNode::SlicePointee {
                target: self.encode(elem)?,
            },
            nia_ty::TyKind::Vector { elem, lanes } => StableTypeNode::Vector {
                element: primitive_tag(elem)
                    .ok_or_else(|| self.unsupported("invalid vector lane type"))?,
                lanes,
            },
            nia_ty::TyKind::Range { kind, bound } => StableTypeNode::Range {
                kind: range_kind_tag(kind),
                bound: bound.map(|bound| self.encode(bound)).transpose()?,
            },
            nia_ty::TyKind::Optional { elem } => StableTypeNode::Optional {
                element: self.encode(elem)?,
            },
            nia_ty::TyKind::ErrorUnion { error, value } => StableTypeNode::ErrorUnion {
                error: self.encode(error)?,
                value: self.encode(value)?,
            },
            nia_ty::TyKind::Callable {
                is_readonly,
                params,
                return_type,
            } => StableTypeNode::Callable {
                parameters: params
                    .into_iter()
                    .map(|param| self.encode(param))
                    .collect::<QueryResult<Vec<_>>>()?,
                result: self.encode(return_type)?,
                readonly: is_readonly,
            },
            nia_ty::TyKind::CallablePointee {
                params,
                return_type,
            } => StableTypeNode::CallablePointee {
                parameters: params
                    .into_iter()
                    .map(|param| self.encode(param))
                    .collect::<QueryResult<Vec<_>>>()?,
                result: self.encode(return_type)?,
            },
            nia_ty::TyKind::SelfParam => StableTypeNode::SelfParam,
            nia_ty::TyKind::BuiltinType(builtin) => {
                StableTypeNode::BuiltinType(builtin_type_tag(builtin))
            }
            nia_ty::TyKind::BuiltinTrait { trait_id, args } => StableTypeNode::BuiltinTrait {
                trait_id: builtin_trait_tag(trait_id)?,
                arguments: args
                    .into_iter()
                    .map(|arg| self.encode(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
            },
            nia_ty::TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => StableTypeNode::TraitObject {
                readonly: is_readonly,
                trait_id: self.stable_trait_id(trait_id)?,
                trait_arguments: trait_args
                    .into_iter()
                    .map(|arg| self.encode(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                trait_const_arguments: trait_const_args
                    .iter()
                    .map(|arg| self.encode_const_argument(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                associated_type_bindings: associated_type_bindings
                    .iter()
                    .map(|binding| self.stable_binding(binding))
                    .collect::<QueryResult<Vec<_>>>()?,
            },
            nia_ty::TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => StableTypeNode::TraitObjectPointee {
                trait_id: self.stable_trait_id(trait_id)?,
                trait_arguments: trait_args
                    .into_iter()
                    .map(|arg| self.encode(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                trait_const_arguments: trait_const_args
                    .iter()
                    .map(|arg| self.encode_const_argument(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                associated_type_bindings: associated_type_bindings
                    .iter()
                    .map(|binding| self.stable_binding(binding))
                    .collect::<QueryResult<Vec<_>>>()?,
            },
            nia_ty::TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            } => StableTypeNode::Projection {
                self_ty: self.encode(self_ty)?,
                trait_id: self.stable_trait_id(trait_id)?,
                trait_arguments: trait_args
                    .into_iter()
                    .map(|arg| self.encode(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                trait_const_arguments: trait_const_args
                    .iter()
                    .map(|arg| self.encode_const_argument(arg))
                    .collect::<QueryResult<Vec<_>>>()?,
                name: name.raw(),
            },
            nia_ty::TyKind::Array { len, elem } => StableTypeNode::Array {
                element: self.encode(elem)?,
                length: self.encode_array_length(&len)?,
            },
            nia_ty::TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: false,
            } => StableTypeNode::Function {
                parameters: params
                    .into_iter()
                    .map(|parameter| self.encode(parameter))
                    .collect::<QueryResult<Vec<_>>>()?,
                result: self.encode(return_type)?,
            },
            nia_ty::TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => {
                if let Some(trait_id) = self.builtin_trait_for_definition(def_id)? {
                    if !const_args.is_empty() {
                        return Err(self.unsupported(
                            "builtin trait applications with const arguments cannot cross package metadata",
                        ));
                    }
                    StableTypeNode::BuiltinTrait {
                        trait_id: builtin_trait_tag(trait_id)?,
                        arguments: args
                            .into_iter()
                            .map(|argument| self.encode(argument))
                            .collect::<QueryResult<Vec<_>>>()?,
                    }
                } else {
                    let definition = self.definition(def_id)?;
                    if args.is_empty() && const_args.is_empty() {
                        StableTypeNode::Named(definition)
                    } else {
                        StableTypeNode::NamedApplied {
                            definition,
                            arguments: args
                                .into_iter()
                                .map(|argument| self.encode(argument))
                                .collect::<QueryResult<Vec<_>>>()?,
                            const_arguments: const_args
                                .iter()
                                .map(|argument| self.encode_const_argument(argument))
                                .collect::<QueryResult<Vec<_>>>()?,
                        }
                    }
                }
            }
            nia_ty::TyKind::GenericParam(name) => StableTypeNode::GenericParam(name.raw()),
            nia_ty::TyKind::ClosureState {
                closure_id,
                captures,
                params,
                return_type,
            } => StableTypeNode::ClosureState {
                owner: self.definition(closure_id.owner)?,
                ordinal: closure_id.ordinal,
                captures: captures
                    .into_iter()
                    .map(|capture| self.encode(capture))
                    .collect::<QueryResult<Vec<_>>>()?,
                parameters: params
                    .into_iter()
                    .map(|param| self.encode(param))
                    .collect::<QueryResult<Vec<_>>>()?,
                result: self.encode(return_type)?,
            },
            unsupported => {
                return Err(self.unsupported(&format!(
                    "type form has no stable package encoding: {unsupported:?}"
                )));
            }
        };
        self.visiting.remove(&ty);
        if let Some(index) = self.canonical_indexes.get(&key) {
            self.indexes.insert(ty, *index);
            return Ok(*index);
        }
        let index = u32::try_from(self.nodes.len()).map_err(|_| {
            self.db.invalid_input(
                &ModuleGraphQuery,
                "stable type graph exceeds u32 nodes".to_string(),
            )
        })?;
        self.nodes.push(node);
        self.indexes.insert(ty, index);
        self.canonical_indexes.insert(key, index);
        Ok(index)
    }

    /// Computes a relocation-independent structural key.  This key is used
    /// solely for graph ordering/deduplication; it never contains session
    /// handles such as `InternedTyId` or `DefId`.
    pub(super) fn canonical_key(&mut self, ty: nia_ids::InternedTyId) -> QueryResult<Vec<u8>> {
        if let Some(key) = self.key_cache.get(&ty) {
            return Ok(key.clone());
        }
        if !self.key_visiting.insert(ty) {
            return Err(self.db.invalid_input(
                &ModuleGraphQuery,
                "recursive session type graph has no stable package representation".to_string(),
            ));
        }
        let kind = self
            .db
            .context()
            .type_store
            .get(ty)
            .cloned()
            .ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "type root belongs to a different compiler session".to_string(),
                )
            })?;
        let mut key = Vec::new();
        match kind {
            nia_ty::TyKind::Primitive(primitive) => {
                let stable = primitive_type_node(primitive)?;
                match stable {
                    StableTypeNode::Primitive(tag) => key.extend_from_slice(&[1, tag]),
                    StableTypeNode::Never => key.push(10),
                    _ => return Err(self.unsupported("primitive has no stable package tag")),
                }
            }
            nia_ty::TyKind::Tuple(elements) if elements.is_empty() => key.push(2),
            nia_ty::TyKind::Tuple(elements) => {
                key.push(3);
                append_len(&mut key, elements.len());
                for element in elements {
                    append_bytes(&mut key, &self.canonical_key(element)?);
                }
            }
            nia_ty::TyKind::Pointer { is_readonly, elem } => {
                key.extend_from_slice(&[4, u8::from(is_readonly)]);
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::Error => key.push(11),
            nia_ty::TyKind::ConstOnly => key.push(12),
            nia_ty::TyKind::Opaque => key.push(13),
            nia_ty::TyKind::VolatilePointer { is_readonly, elem } => {
                key.extend_from_slice(&[14, u8::from(is_readonly)]);
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::Slice { is_readonly, elem } => {
                key.extend_from_slice(&[15, u8::from(is_readonly)]);
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::SlicePointee { elem } => {
                key.push(16);
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::Vector { elem, lanes } => {
                key.push(17);
                key.push(
                    primitive_tag(elem)
                        .ok_or_else(|| self.unsupported("invalid vector lane type"))?,
                );
                key.extend_from_slice(&lanes.to_le_bytes());
            }
            nia_ty::TyKind::Range { kind, bound } => {
                key.push(18);
                key.push(range_kind_tag(kind));
                if let Some(bound) = bound {
                    key.push(1);
                    append_bytes(&mut key, &self.canonical_key(bound)?);
                } else {
                    key.push(0);
                }
            }
            nia_ty::TyKind::Optional { elem } => {
                key.push(19);
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::ErrorUnion { error, value } => {
                key.push(20);
                append_bytes(&mut key, &self.canonical_key(error)?);
                append_bytes(&mut key, &self.canonical_key(value)?);
            }
            nia_ty::TyKind::Callable {
                is_readonly,
                params,
                return_type,
            } => {
                key.extend_from_slice(&[21, u8::from(is_readonly)]);
                append_len(&mut key, params.len());
                for param in params {
                    append_bytes(&mut key, &self.canonical_key(param)?);
                }
                append_bytes(&mut key, &self.canonical_key(return_type)?);
            }
            nia_ty::TyKind::CallablePointee {
                params,
                return_type,
            } => {
                key.push(22);
                append_len(&mut key, params.len());
                for param in params {
                    append_bytes(&mut key, &self.canonical_key(param)?);
                }
                append_bytes(&mut key, &self.canonical_key(return_type)?);
            }
            nia_ty::TyKind::SelfParam => key.push(23),
            nia_ty::TyKind::BuiltinType(builtin) => {
                key.push(24);
                key.push(builtin_type_tag(builtin));
            }
            nia_ty::TyKind::BuiltinTrait { trait_id, args } => {
                key.push(25);
                key.push(builtin_trait_tag(trait_id)?);
                append_len(&mut key, args.len());
                for arg in args {
                    append_bytes(&mut key, &self.canonical_key(arg)?);
                }
            }
            nia_ty::TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => {
                key.extend_from_slice(&[26, u8::from(is_readonly)]);
                self.append_trait_key(&mut key, trait_id)?;
                append_len(&mut key, trait_args.len());
                for arg in trait_args {
                    append_bytes(&mut key, &self.canonical_key(arg)?);
                }
                append_len(&mut key, trait_const_args.len());
                for arg in trait_const_args {
                    self.append_stable_const_key(&mut key, &arg)?;
                }
                self.append_binding_keys(&mut key, &associated_type_bindings)?;
            }
            nia_ty::TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => {
                key.push(27);
                self.append_trait_key(&mut key, trait_id)?;
                append_len(&mut key, trait_args.len());
                for arg in trait_args {
                    append_bytes(&mut key, &self.canonical_key(arg)?);
                }
                append_len(&mut key, trait_const_args.len());
                for arg in trait_const_args {
                    self.append_stable_const_key(&mut key, &arg)?;
                }
                self.append_binding_keys(&mut key, &associated_type_bindings)?;
            }
            nia_ty::TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            } => {
                key.push(28);
                append_bytes(&mut key, &self.canonical_key(self_ty)?);
                self.append_trait_key(&mut key, trait_id)?;
                append_len(&mut key, trait_args.len());
                for arg in trait_args {
                    append_bytes(&mut key, &self.canonical_key(arg)?);
                }
                append_len(&mut key, trait_const_args.len());
                for arg in trait_const_args {
                    self.append_stable_const_key(&mut key, &arg)?;
                }
                key.extend_from_slice(&name.raw().to_le_bytes());
            }
            nia_ty::TyKind::Array { len, elem } => {
                key.push(5);
                match self.encode_array_length(&len)? {
                    StableArrayLength::ConstValue(length) => {
                        key.push(0);
                        key.extend_from_slice(&length.to_le_bytes());
                    }
                    StableArrayLength::GenericParam(hash) => {
                        key.push(1);
                        key.extend_from_slice(&hash.to_le_bytes());
                    }
                }
                append_bytes(&mut key, &self.canonical_key(elem)?);
            }
            nia_ty::TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: false,
            } => {
                key.push(6);
                append_len(&mut key, params.len());
                for parameter in params {
                    append_bytes(&mut key, &self.canonical_key(parameter)?);
                }
                append_bytes(&mut key, &self.canonical_key(return_type)?);
            }
            nia_ty::TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => {
                if let Some(trait_id) = self.builtin_trait_for_definition(def_id)? {
                    if !const_args.is_empty() {
                        return Err(self.unsupported(
                            "builtin trait applications with const arguments cannot cross package metadata",
                        ));
                    }
                    key.push(25);
                    key.push(builtin_trait_tag(trait_id)?);
                    append_len(&mut key, args.len());
                    for argument in args {
                        append_bytes(&mut key, &self.canonical_key(argument)?);
                    }
                } else {
                    let definition = self.definition(def_id)?;
                    key.push(if args.is_empty() && const_args.is_empty() {
                        7
                    } else {
                        8
                    });
                    append_definition_key(&mut key, &definition);
                    append_len(&mut key, args.len());
                    for argument in args {
                        append_bytes(&mut key, &self.canonical_key(argument)?);
                    }
                    append_len(&mut key, const_args.len());
                    for argument in &const_args {
                        self.append_stable_const_key(&mut key, argument)?;
                    }
                }
            }
            nia_ty::TyKind::GenericParam(name) => {
                key.push(9);
                key.extend_from_slice(&name.raw().to_le_bytes());
            }
            nia_ty::TyKind::ClosureState {
                closure_id,
                captures,
                params,
                return_type,
            } => {
                key.push(29);
                append_definition_key(&mut key, &self.definition(closure_id.owner)?);
                key.extend_from_slice(&closure_id.ordinal.to_le_bytes());
                append_len(&mut key, captures.len());
                for capture in captures {
                    append_bytes(&mut key, &self.canonical_key(capture)?);
                }
                append_len(&mut key, params.len());
                for param in params {
                    append_bytes(&mut key, &self.canonical_key(param)?);
                }
                append_bytes(&mut key, &self.canonical_key(return_type)?);
            }
            unsupported => {
                return Err(self.unsupported(&format!(
                    "type form has no stable package encoding: {unsupported:?}"
                )));
            }
        }
        self.key_visiting.remove(&ty);
        self.key_cache.insert(ty, key.clone());
        Ok(key)
    }

    fn append_trait_key(&self, key: &mut Vec<u8>, trait_id: nia_ty::TraitId) -> QueryResult<()> {
        match trait_id {
            nia_ty::TraitId::Source(def_id) => {
                key.push(0);
                append_definition_key(key, &self.definition(def_id)?);
            }
            nia_ty::TraitId::Builtin(builtin) => {
                key.push(1);
                key.push(builtin_trait_tag(builtin)?);
            }
        }
        Ok(())
    }

    fn append_stable_const_key(
        &mut self,
        key: &mut Vec<u8>,
        argument: &nia_ty::ConstGenericArg,
    ) -> QueryResult<()> {
        append_bytes(key, &self.canonical_key(argument.ty)?);
        match &argument.value {
            nia_ty::ConstGenericValue::GenericParam(name) => {
                key.push(0);
                key.extend_from_slice(&name.raw().to_le_bytes());
            }
            nia_ty::ConstGenericValue::Int(value) => {
                key.push(1);
                key.extend_from_slice(&value.bits().to_le_bytes());
                key.push(u8::from(value.is_signed()));
            }
            nia_ty::ConstGenericValue::Bool(value) => {
                key.push(2);
                key.push(u8::from(*value));
            }
            nia_ty::ConstGenericValue::Char(value) => {
                key.push(3);
                key.extend_from_slice(&u32::from(*value).to_le_bytes());
            }
            nia_ty::ConstGenericValue::ConstExpr(_) => {
                return Err(self.unsupported(
                    "const expression argument has no stable package representation",
                ));
            }
        }
        Ok(())
    }

    fn append_binding_keys(
        &mut self,
        key: &mut Vec<u8>,
        bindings: &[nia_ty::AssociatedTypeBindingTy],
    ) -> QueryResult<()> {
        append_len(key, bindings.len());
        for binding in bindings {
            match binding.trait_id {
                Some(trait_id) => {
                    key.push(1);
                    self.append_trait_key(key, trait_id)?;
                }
                None => key.push(0),
            }
            append_len(key, binding.trait_args.len());
            for arg in &binding.trait_args {
                append_bytes(key, &self.canonical_key(*arg)?);
            }
            append_len(key, binding.trait_const_args.len());
            for arg in &binding.trait_const_args {
                self.append_stable_const_key(key, arg)?;
            }
            key.extend_from_slice(&binding.name.raw().to_le_bytes());
            append_bytes(key, &self.canonical_key(binding.ty)?);
        }
        Ok(())
    }

    fn definition(&self, def_id: nia_ids::GlobalDefId) -> QueryResult<DefinitionId> {
        let module = self.graph.stable_key(def_id.module_id).ok_or_else(|| {
            self.db.invalid_input(
                &ModuleGraphQuery,
                "nominal type has no stable module identity".to_string(),
            )
        })?;
        let defs = self.db.get(FullModuleDefsQuery(def_id.module_id))?;
        let def = defs.semantic.defs.get(def_id.def_id).ok_or_else(|| {
            self.db.invalid_input(
                &ModuleGraphQuery,
                "nominal type has no definition facts".to_string(),
            )
        })?;
        let name = self.symbols.resolve(def.name).ok_or_else(|| {
            self.db.invalid_input(
                &ModuleGraphQuery,
                "nominal type name is absent from the session symbol table".to_string(),
            )
        })?;
        let package = self.resolver.package_for_definition(def_id)?;
        let mut owner_chain = Vec::new();
        let mut parent = def.parent;
        while let Some(parent_id) = parent {
            let parent_def = defs.semantic.defs.get(parent_id).ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "definition parent is missing from module facts".to_string(),
                )
            })?;
            let parent_name = self.symbols.resolve(parent_def.name).ok_or_else(|| {
                self.db.invalid_input(
                    &ModuleGraphQuery,
                    "definition parent has no resolvable symbol".to_string(),
                )
            })?;
            owner_chain.push((
                parent_name.to_string(),
                def_kind_tag(parent_def.kind),
                parent_id.0,
            ));
            parent = parent_def.parent;
        }
        let mut owner_identity = None;
        for (owner_name, owner_kind, owner_disambiguator) in owner_chain.into_iter().rev() {
            owner_identity = Some(Box::new(DefinitionId {
                module: StableModuleId {
                    package: package.clone(),
                    path: module.source_identity().normalized_path().to_owned(),
                },
                name: owner_name,
                kind: owner_kind,
                disambiguator: owner_disambiguator,
                owner: owner_identity,
            }));
        }
        Ok(DefinitionId {
            module: StableModuleId {
                package,
                path: module.source_identity().normalized_path().to_owned(),
            },
            name: name.to_string(),
            kind: def_kind_tag(def.kind),
            disambiguator: def_id.def_id.0,
            owner: owner_identity,
        })
    }

    fn encode_const_argument(
        &mut self,
        argument: &nia_ty::ConstGenericArg,
    ) -> QueryResult<StableConstArg> {
        let value = match &argument.value {
            nia_ty::ConstGenericValue::GenericParam(name) => {
                StableConstValue::GenericParam(name.raw())
            }
            nia_ty::ConstGenericValue::Int(value) => StableConstValue::Integer {
                bits: value.bits(),
                signed: value.is_signed(),
            },
            nia_ty::ConstGenericValue::Bool(value) => StableConstValue::Bool(*value),
            nia_ty::ConstGenericValue::Char(value) => StableConstValue::Char(*value),
            nia_ty::ConstGenericValue::ConstExpr(_) => {
                return Err(self.unsupported(
                    "const expression argument has no stable package representation",
                ));
            }
        };
        Ok(StableConstArg {
            ty: self.encode(argument.ty)?,
            value,
        })
    }

    fn encode_array_length(&self, length: &nia_ty::ArrayLenTy) -> QueryResult<StableArrayLength> {
        match length {
            nia_ty::ArrayLenTy::ConstValue(value) => Ok(StableArrayLength::ConstValue(*value)),
            nia_ty::ArrayLenTy::GenericParam(name) => {
                Ok(StableArrayLength::GenericParam(name.raw()))
            }
            nia_ty::ArrayLenTy::ConstExpr(id) => {
                let values = self.db.get(ConstArrayLengthsQuery(id.module_id))?;
                values
                    .values
                    .get(id)
                    .copied()
                    .map(StableArrayLength::ConstValue)
                    .ok_or_else(|| {
                        self.unsupported(
                            "array length const expression is not evaluated in the stable type graph",
                        )
                    })
            }
            nia_ty::ArrayLenTy::Builtin { .. } | nia_ty::ArrayLenTy::Infer => Err(self
                .unsupported("array length depends on contextual or target-dependent inference")),
        }
    }

    fn stable_trait_id(&self, trait_id: nia_ty::TraitId) -> QueryResult<StableTraitId> {
        Ok(match trait_id {
            nia_ty::TraitId::Source(def_id) => {
                if let Some(builtin) = self.builtin_trait_for_definition(def_id)? {
                    StableTraitId::Builtin(builtin_trait_tag(builtin)?)
                } else {
                    StableTraitId::Source(self.definition(def_id)?)
                }
            }
            nia_ty::TraitId::Builtin(builtin) => {
                StableTraitId::Builtin(builtin_trait_tag(builtin)?)
            }
        })
    }

    fn builtin_trait_for_definition(
        &self,
        def_id: GlobalDefId,
    ) -> QueryResult<Option<nia_ids::BuiltinTrait>> {
        Ok(self
            .db
            .get(ItemSignaturesQuery(def_id.module_id))?
            .semantic
            .traits
            .get(&def_id.def_id)
            .and_then(|signature| signature.builtin))
    }

    fn stable_binding(
        &mut self,
        binding: &nia_ty::AssociatedTypeBindingTy,
    ) -> QueryResult<StableAssociatedTypeBinding> {
        Ok(StableAssociatedTypeBinding {
            trait_id: binding
                .trait_id
                .map(|trait_id| self.stable_trait_id(trait_id))
                .transpose()?,
            trait_arguments: binding
                .trait_args
                .iter()
                .map(|arg| self.encode(*arg))
                .collect::<QueryResult<Vec<_>>>()?,
            trait_const_arguments: binding
                .trait_const_args
                .iter()
                .map(|arg| self.encode_const_argument(arg))
                .collect::<QueryResult<Vec<_>>>()?,
            name: binding.name.raw(),
            ty: self.encode(binding.ty)?,
        })
    }

    fn unsupported(&self, detail: &str) -> QueryError {
        self.db.invalid_input(
            &ModuleGraphQuery,
            format!("{detail}; package type graph publication is not implemented for this form"),
        )
    }
}

fn primitive_type_node(primitive: nia_ty::PrimitiveTy) -> QueryResult<StableTypeNode> {
    use nia_ty::PrimitiveTy;
    Ok(match primitive {
        PrimitiveTy::Never => StableTypeNode::Never,
        PrimitiveTy::I8 => StableTypeNode::Primitive(1),
        PrimitiveTy::I16 => StableTypeNode::Primitive(2),
        PrimitiveTy::I32 => StableTypeNode::Primitive(3),
        PrimitiveTy::I64 => StableTypeNode::Primitive(4),
        PrimitiveTy::I128 => StableTypeNode::Primitive(5),
        PrimitiveTy::Isize => StableTypeNode::Primitive(6),
        PrimitiveTy::U8 => StableTypeNode::Primitive(7),
        PrimitiveTy::U16 => StableTypeNode::Primitive(8),
        PrimitiveTy::U32 => StableTypeNode::Primitive(9),
        PrimitiveTy::U64 => StableTypeNode::Primitive(10),
        PrimitiveTy::U128 => StableTypeNode::Primitive(11),
        PrimitiveTy::Usize => StableTypeNode::Primitive(12),
        PrimitiveTy::F32 => StableTypeNode::Primitive(13),
        PrimitiveTy::F64 => StableTypeNode::Primitive(14),
        PrimitiveTy::Bool => StableTypeNode::Primitive(15),
        PrimitiveTy::Char => StableTypeNode::Primitive(16),
    })
}

pub(super) fn stable_range_kind(tag: u8) -> Option<nia_ty::RangeTyKind> {
    Some(match tag {
        0 => nia_ty::RangeTyKind::Exclusive,
        1 => nia_ty::RangeTyKind::Inclusive,
        2 => nia_ty::RangeTyKind::From,
        3 => nia_ty::RangeTyKind::To,
        4 => nia_ty::RangeTyKind::ToInclusive,
        5 => nia_ty::RangeTyKind::Full,
        _ => return None,
    })
}

fn primitive_tag(primitive: nia_ty::PrimitiveTy) -> Option<u8> {
    match primitive_type_node(primitive).ok()? {
        StableTypeNode::Primitive(tag) => Some(tag),
        StableTypeNode::Never => None,
        _ => None,
    }
}

fn range_kind_tag(kind: nia_ty::RangeTyKind) -> u8 {
    match kind {
        nia_ty::RangeTyKind::Exclusive => 0,
        nia_ty::RangeTyKind::Inclusive => 1,
        nia_ty::RangeTyKind::From => 2,
        nia_ty::RangeTyKind::To => 3,
        nia_ty::RangeTyKind::ToInclusive => 4,
        nia_ty::RangeTyKind::Full => 5,
    }
}

fn builtin_type_tag(value: nia_ids::BuiltinType) -> u8 {
    match value {
        nia_ids::BuiltinType::AsmConfig => 0,
        nia_ids::BuiltinType::AsmInputs => 1,
        nia_ids::BuiltinType::AsmOutputs => 2,
    }
}

fn builtin_trait_tag(value: nia_ids::BuiltinTrait) -> nia_ice::IceResult<u8> {
    u8::try_from(value.stable_tag())
        .map_err(|_| nia_ice::Ice::new("builtin trait stable tag exceeds metadata width"))
}

pub(super) fn stable_builtin_type(tag: u8) -> Option<nia_ids::BuiltinType> {
    Some(match tag {
        0 => nia_ids::BuiltinType::AsmConfig,
        1 => nia_ids::BuiltinType::AsmInputs,
        2 => nia_ids::BuiltinType::AsmOutputs,
        _ => return None,
    })
}

pub(super) fn stable_builtin_trait(tag: u8) -> Option<nia_ids::BuiltinTrait> {
    nia_ids::BuiltinTrait::from_stable_tag(u32::from(tag))
}

fn append_len(bytes: &mut Vec<u8>, len: usize) {
    bytes.extend_from_slice(&(len as u64).to_le_bytes());
}

fn append_bytes(bytes: &mut Vec<u8>, value: &[u8]) {
    append_len(bytes, value.len());
    bytes.extend_from_slice(value);
}

fn append_definition_key(bytes: &mut Vec<u8>, definition: &DefinitionId) {
    append_bytes(bytes, definition.module.package.namespace.as_bytes());
    append_bytes(bytes, definition.module.package.name.as_bytes());
    append_bytes(bytes, definition.module.package.version.as_bytes());
    append_bytes(bytes, definition.module.path.as_bytes());
    append_bytes(bytes, definition.name.as_bytes());
    bytes.push(definition.kind);
    match &definition.owner {
        Some(owner) => {
            bytes.push(1);
            append_definition_key(bytes, owner);
        }
        None => bytes.push(0),
    }
}

pub(super) fn def_kind_tag(kind: nia_defs::DefKind) -> u8 {
    use nia_defs::DefKind;
    match kind {
        DefKind::Module => 1,
        DefKind::Function => 2,
        DefKind::Global => 3,
        DefKind::Const => 4,
        DefKind::Struct => 5,
        DefKind::StructField => 6,
        DefKind::Union => 7,
        DefKind::UnionField => 8,
        DefKind::Trait => 9,
        DefKind::TraitAssociatedType => 10,
        DefKind::TraitMethod => 11,
        DefKind::Method => 12,
        DefKind::Enum => 13,
        DefKind::EnumVariant => 14,
        DefKind::EnumVariantField => 15,
        DefKind::TypeAlias => 16,
    }
}

impl CompilerDatabase {
    /// Rehydrates a stable package type graph into this database's canonical
    /// type store using an explicit definition identity resolver.
    pub fn rehydrate_stable_type_graph(
        &self,
        graph: &StableTypeGraph,
        resolver: &dyn StableDefinitionResolver,
    ) -> QueryResult<Vec<InternedTyId>> {
        let types = self.rehydrate_stable_type_graph_nodes(graph, resolver)?;
        self.rehydrated_types(&types, &graph.roots)
    }

    /// Rehydrates every node in a stable package graph, preserving its wire
    /// index so typed signature payloads can reference arbitrary nodes.
    fn rehydrate_stable_type_graph_nodes(
        &self,
        graph: &StableTypeGraph,
        resolver: &dyn StableDefinitionResolver,
    ) -> QueryResult<Vec<InternedTyId>> {
        graph
            .validate()
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        let entry = self.db.get(ModuleGraphQuery)?.entry();
        let append = self.db.context().type_store.append_for_module(entry);
        let mut types = Vec::with_capacity(graph.nodes.len());
        for node in &graph.nodes {
            let ty = match node {
                StableTypeNode::Primitive(tag) => stable_primitive_from_tag(*tag)
                    .map(|primitive| append.primitive(primitive))
                    .ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "unknown stable primitive tag".to_string(),
                        )
                    })?,
                StableTypeNode::Named(definition) => append.intern(nia_ty::TyKind::Nominal {
                    def_id: resolver.definition_for_identity(definition)?,
                    args: Vec::new(),
                    const_args: Vec::new(),
                }),
                StableTypeNode::NamedApplied {
                    definition,
                    arguments,
                    const_arguments,
                } => append.intern(nia_ty::TyKind::Nominal {
                    def_id: resolver.definition_for_identity(definition)?,
                    args: self.rehydrated_types(&types, arguments)?,
                    const_args: self.rehydrate_stable_const_args(const_arguments, &types)?,
                }),
                StableTypeNode::Unit => append.intern(nia_ty::TyKind::Tuple(Vec::new())),
                StableTypeNode::Never => {
                    append.intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::Never))
                }
                StableTypeNode::Tuple(elements) => append.intern(nia_ty::TyKind::Tuple(
                    self.rehydrated_types(&types, elements)?,
                )),
                StableTypeNode::Array { element, length } => append.intern(nia_ty::TyKind::Array {
                    elem: self.rehydrated_type(&types, *element)?,
                    len: match length {
                        StableArrayLength::ConstValue(value) => {
                            nia_ty::ArrayLenTy::ConstValue(*value)
                        }
                        StableArrayLength::GenericParam(hash) => {
                            nia_ty::ArrayLenTy::GenericParam(SymbolId::from_stable_hash(*hash))
                        }
                    },
                }),
                StableTypeNode::Function { parameters, result } => {
                    append.intern(nia_ty::TyKind::FunctionPointer {
                        params: self.rehydrated_types(&types, parameters)?,
                        return_type: self.rehydrated_type(&types, *result)?,
                        is_variadic: false,
                    })
                }
                StableTypeNode::Reference { target, mutable } => {
                    append.intern(nia_ty::TyKind::Pointer {
                        is_readonly: !*mutable,
                        elem: self.rehydrated_type(&types, *target)?,
                    })
                }
                StableTypeNode::Pointer { target, readonly } => {
                    append.intern(nia_ty::TyKind::Pointer {
                        is_readonly: *readonly,
                        elem: self.rehydrated_type(&types, *target)?,
                    })
                }
                StableTypeNode::GenericParam(hash) => append.intern(nia_ty::TyKind::GenericParam(
                    SymbolId::from_stable_hash(*hash),
                )),
                StableTypeNode::Error => append.intern(nia_ty::TyKind::Error),
                StableTypeNode::ConstOnly => append.intern(nia_ty::TyKind::ConstOnly),
                StableTypeNode::Opaque => append.intern(nia_ty::TyKind::Opaque),
                StableTypeNode::VolatilePointer { target, readonly } => {
                    append.intern(nia_ty::TyKind::VolatilePointer {
                        is_readonly: *readonly,
                        elem: self.rehydrated_type(&types, *target)?,
                    })
                }
                StableTypeNode::Slice { target, readonly } => {
                    append.intern(nia_ty::TyKind::Slice {
                        is_readonly: *readonly,
                        elem: self.rehydrated_type(&types, *target)?,
                    })
                }
                StableTypeNode::SlicePointee { target } => {
                    append.intern(nia_ty::TyKind::SlicePointee {
                        elem: self.rehydrated_type(&types, *target)?,
                    })
                }
                StableTypeNode::Vector { element, lanes } => {
                    append.intern(nia_ty::TyKind::Vector {
                        elem: stable_primitive_from_tag(*element).ok_or_else(|| {
                            self.db.invalid_input(
                                &ModuleGraphQuery,
                                "unknown stable vector element tag".to_string(),
                            )
                        })?,
                        lanes: *lanes,
                    })
                }
                StableTypeNode::Range { kind, bound } => append.intern(nia_ty::TyKind::Range {
                    kind: stable_range_kind(*kind).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "unknown stable range kind".to_string(),
                        )
                    })?,
                    bound: bound
                        .map(|index| self.rehydrated_type(&types, index))
                        .transpose()?,
                }),
                StableTypeNode::Optional { element } => append.intern(nia_ty::TyKind::Optional {
                    elem: self.rehydrated_type(&types, *element)?,
                }),
                StableTypeNode::ErrorUnion { error, value } => {
                    append.intern(nia_ty::TyKind::ErrorUnion {
                        error: self.rehydrated_type(&types, *error)?,
                        value: self.rehydrated_type(&types, *value)?,
                    })
                }
                StableTypeNode::Callable {
                    parameters,
                    result,
                    readonly,
                } => append.intern(nia_ty::TyKind::Callable {
                    is_readonly: *readonly,
                    params: self.rehydrated_types(&types, parameters)?,
                    return_type: self.rehydrated_type(&types, *result)?,
                }),
                StableTypeNode::CallablePointee { parameters, result } => {
                    append.intern(nia_ty::TyKind::CallablePointee {
                        params: self.rehydrated_types(&types, parameters)?,
                        return_type: self.rehydrated_type(&types, *result)?,
                    })
                }
                StableTypeNode::SelfParam => append.intern(nia_ty::TyKind::SelfParam),
                StableTypeNode::BuiltinType(tag) => append.intern(nia_ty::TyKind::BuiltinType(
                    stable_builtin_type(*tag).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "unknown stable builtin type tag".to_string(),
                        )
                    })?,
                )),
                StableTypeNode::BuiltinTrait {
                    trait_id,
                    arguments,
                } => append.intern(nia_ty::TyKind::BuiltinTrait {
                    trait_id: stable_builtin_trait(*trait_id).ok_or_else(|| {
                        self.db.invalid_input(
                            &ModuleGraphQuery,
                            "unknown stable builtin trait tag".to_string(),
                        )
                    })?,
                    args: self.rehydrated_types(&types, arguments)?,
                }),
                StableTypeNode::TraitObject {
                    readonly,
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    associated_type_bindings,
                } => append.intern(nia_ty::TyKind::TraitObject {
                    is_readonly: *readonly,
                    trait_id: self.rehydrate_stable_trait_id(trait_id, resolver)?,
                    trait_args: self.rehydrated_types(&types, trait_arguments)?,
                    trait_const_args: self
                        .rehydrate_stable_const_args(trait_const_arguments, &types)?,
                    associated_type_bindings: self.rehydrate_stable_bindings(
                        associated_type_bindings,
                        &types,
                        resolver,
                    )?,
                }),
                StableTypeNode::TraitObjectPointee {
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    associated_type_bindings,
                } => append.intern(nia_ty::TyKind::TraitObjectPointee {
                    trait_id: self.rehydrate_stable_trait_id(trait_id, resolver)?,
                    trait_args: self.rehydrated_types(&types, trait_arguments)?,
                    trait_const_args: self
                        .rehydrate_stable_const_args(trait_const_arguments, &types)?,
                    associated_type_bindings: self.rehydrate_stable_bindings(
                        associated_type_bindings,
                        &types,
                        resolver,
                    )?,
                }),
                StableTypeNode::Projection {
                    self_ty,
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    name,
                } => append.intern(nia_ty::TyKind::Projection {
                    self_ty: self.rehydrated_type(&types, *self_ty)?,
                    trait_id: self.rehydrate_stable_trait_id(trait_id, resolver)?,
                    trait_args: self.rehydrated_types(&types, trait_arguments)?,
                    trait_const_args: self
                        .rehydrate_stable_const_args(trait_const_arguments, &types)?,
                    name: SymbolId::from_stable_hash(*name),
                }),
                StableTypeNode::ClosureState {
                    owner,
                    ordinal,
                    captures,
                    parameters,
                    result,
                } => append.intern(nia_ty::TyKind::ClosureState {
                    closure_id: nia_ids::ClosureId {
                        owner: resolver.definition_for_identity(owner)?,
                        ordinal: *ordinal,
                    },
                    captures: self.rehydrated_types(&types, captures)?,
                    params: self.rehydrated_types(&types, parameters)?,
                    return_type: self.rehydrated_type(&types, *result)?,
                }),
            }?;
            types.push(ty);
        }
        Ok(types)
    }

    /// Converts session-owned type roots into a package-stable type graph.
    ///
    /// Only forms with a complete cross-package representation are accepted.
    /// Unsupported forms deliberately fail rather than serializing a
    /// session-local handle or silently weakening the interface contract.
    pub fn stable_type_graph_for_roots(
        &self,
        package: PackageId,
        roots: &[nia_ids::InternedTyId],
    ) -> QueryResult<StableTypeGraph> {
        self.stable_type_graph_for_roots_with_resolver(roots, &|def_id: GlobalDefId| {
            let graph = self.db.get(ModuleGraphQuery)?;
            let Some(entry_root) = graph.current_package_root(graph.entry()) else {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    "entry module has no package root; provide an external definition resolver"
                        .to_string(),
                ));
            };
            if graph.current_package_root(def_id.module_id) != Some(entry_root) {
                return Err(self.db.invalid_input(
                    &ModuleGraphQuery,
                    format!("nominal type belongs to an external package; provide an external definition resolver: {def_id:?}"),
                ));
            }
            Ok(package.clone())
        })
    }

    /// Converts session-owned type roots into a package-stable type graph using
    /// an explicit definition-to-package resolver.
    ///
    /// The resolver is required for every nominal definition, including
    /// definitions from the current package. This keeps external identities
    /// explicit and prevents dependency types from being silently attributed
    /// to the current package.
    pub fn stable_type_graph_for_roots_with_resolver(
        &self,
        roots: &[nia_ids::InternedTyId],
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<StableTypeGraph> {
        self.stable_type_graph_for_roots_with_resolver_and_indexes(roots, resolver)
            .map(|(graph, _)| graph)
    }

    fn stable_type_graph_for_roots_with_resolver_and_indexes(
        &self,
        roots: &[nia_ids::InternedTyId],
        resolver: &dyn StableDefinitionPackageResolver,
    ) -> QueryResult<(StableTypeGraph, HashMap<nia_ids::InternedTyId, u32>)> {
        let graph = self.db.get(ModuleGraphQuery)?;
        let symbols = self.db.context().loader_facts().symbols();
        let mut encoder = StableTypeGraphEncoder {
            db: &self.db,
            graph: &graph,
            symbols: &symbols,
            resolver,
            indexes: HashMap::new(),
            visiting: HashSet::new(),
            key_cache: HashMap::new(),
            key_visiting: HashSet::new(),
            canonical_indexes: HashMap::new(),
            nodes: Vec::new(),
        };
        // Session-local type handles are intentionally not part of the
        // stable ordering. Sort roots by their structural, relocation-
        // independent key before assigning graph indices.
        let mut keyed_roots = roots
            .iter()
            .map(|root| Ok((*root, encoder.canonical_key(*root)?)))
            .collect::<QueryResult<Vec<_>>>()?;
        keyed_roots.sort_by(|left, right| left.1.cmp(&right.1));
        let ordered_roots = keyed_roots
            .into_iter()
            .map(|(root, _)| root)
            .collect::<Vec<_>>();
        let roots = ordered_roots
            .iter()
            .map(|root| encoder.encode(*root))
            .collect::<QueryResult<Vec<_>>>()?;
        let mut roots = roots;
        roots.sort_unstable();
        roots.dedup();
        let graph = StableTypeGraph {
            nodes: encoder.nodes,
            roots,
        };
        graph
            .validate()
            .map_err(|error| self.db.invalid_input(&ModuleGraphQuery, error.to_string()))?;
        Ok((graph, encoder.indexes))
    }
}
