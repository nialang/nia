// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable type-lowering graph encoding and reconstruction.

use super::*;

pub(crate) fn encode_type_lowering(
    lowering: &TypeLowering,
    source_version: SourceVersion,
    module_paths: &HashMap<ModuleId, String>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
) -> io::Result<Vec<u8>> {
    if !lowering.diagnostics.is_empty()
        || !lowering.const_exprs.is_empty()
        || !lowering.const_expr_summaries.is_empty()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot cache signature lowering with diagnostics or const expressions",
        ));
    }
    let mut uses = lowering
        .type_uses
        .iter()
        .map(|(site, ty)| {
            let mut encoded_site = Vec::new();
            write_node_site(&mut encoded_site, site, source_version.id)?;
            Ok((encoded_site, *ty))
        })
        .collect::<io::Result<Vec<_>>>()?;
    uses.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    let mut graph = TypeGraphEncoder {
        type_store,
        module_paths,
        symbols,
        indexes: HashMap::new(),
        visiting: HashSet::new(),
        nodes: Vec::new(),
    };
    let uses = uses
        .into_iter()
        .map(|(site, ty)| Ok((site, graph.intern(ty)?)))
        .collect::<io::Result<Vec<_>>>()?;

    let mut encoded = Vec::new();
    write_type_graph(&mut encoded, graph.nodes);
    write_u64(&mut encoded, uses.len() as u64);
    for (site, index) in uses {
        write_u64(&mut encoded, site.len().saturating_add(8) as u64);
        encoded.extend_from_slice(&site);
        write_u64(&mut encoded, index as u64);
    }
    Ok(encoded)
}

pub(crate) fn decode_type_lowering(
    encoded: &[u8],
    source_version: SourceVersion,
    source_len: usize,
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
    module_id: ModuleId,
) -> Option<TypeLowering> {
    let mut cursor = Cursor::new(encoded);
    let types = read_type_graph(
        &mut cursor,
        encoded.len(),
        modules,
        symbols,
        type_store,
        module_id,
    )?;

    let mut type_uses = HashMap::new();
    for entry in read_entries(&mut cursor, encoded.len())? {
        let mut entry = Cursor::new(entry);
        let site = read_node_site(&mut entry, source_version.id, source_len)?;
        let ty = *types.get(usize::try_from(read_u64(&mut entry)?).ok()?)?;
        if usize::try_from(entry.position()).ok()? != entry.get_ref().len()
            || type_uses.insert(site, ty).is_some()
        {
            return None;
        }
    }
    if usize::try_from(cursor.position()).ok()? != encoded.len() {
        return None;
    }
    Some(TypeLowering {
        type_uses,
        const_exprs: HashMap::new(),
        const_expr_summaries: HashMap::new(),
        diagnostics: Vec::new(),
    })
}

pub(crate) fn write_type_graph(encoded: &mut Vec<u8>, nodes: Vec<Vec<u8>>) {
    write_u64(encoded, nodes.len() as u64);
    for node in nodes {
        write_u64(encoded, node.len() as u64);
        encoded.extend_from_slice(&node);
    }
}

pub(crate) fn read_type_graph(
    cursor: &mut Cursor<&[u8]>,
    encoded_len: usize,
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
    module_id: ModuleId,
) -> Option<Vec<InternedTyId>> {
    let node_entries = read_entries(cursor, encoded_len)?;
    let append = type_store.append_for_module(module_id);
    let mut types = Vec::with_capacity(node_entries.len());
    for entry in node_entries {
        let mut entry = Cursor::new(entry);
        let kind = read_ty_kind(&mut entry, &types, modules, symbols)?;
        if usize::try_from(entry.position()).ok()? != entry.get_ref().len() {
            return None;
        }
        types.push(append.intern(kind));
    }
    Some(types)
}

pub(crate) struct TypeGraphEncoder<'a> {
    pub(crate) type_store: &'a TypeStore,
    pub(crate) module_paths: &'a HashMap<ModuleId, String>,
    pub(crate) symbols: &'a SymbolTable,
    pub(crate) indexes: HashMap<InternedTyId, u32>,
    pub(crate) visiting: HashSet<InternedTyId>,
    pub(crate) nodes: Vec<Vec<u8>>,
}

impl TypeGraphEncoder<'_> {
    pub(crate) fn intern(&mut self, ty: InternedTyId) -> io::Result<u32> {
        if let Some(index) = self.indexes.get(&ty) {
            return Ok(*index);
        }
        if !self.visiting.insert(ty) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cyclic type graph cannot be cached",
            ));
        }
        let kind = self.type_store.get(ty).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "signature lowering type belongs to another type store",
            )
        })?;
        // Recursive interning writes every referenced child before assigning the current index.
        // The decoder can therefore rebuild nodes in one pass and reject forward/cyclic edges.
        let mut encoded = Vec::new();
        write_ty_kind(&mut encoded, &kind, self)?;
        self.visiting.remove(&ty);
        let index = u32::try_from(self.nodes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "type graph too large"))?;
        self.nodes.push(encoded);
        self.indexes.insert(ty, index);
        Ok(index)
    }

    pub(crate) fn write_symbol(
        &self,
        encoded: &mut Vec<u8>,
        symbol: nia_symbol::SymbolId,
    ) -> io::Result<()> {
        let text = self.symbols.resolve(symbol).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "unresolved cached type symbol")
        })?;
        write_string(encoded, &text);
        Ok(())
    }
}

pub(crate) fn write_ty_kind(
    encoded: &mut Vec<u8>,
    kind: &TyKind,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    match kind {
        TyKind::Error => encoded.push(0),
        TyKind::ConstOnly => encoded.push(1),
        TyKind::Primitive(primitive) => {
            encoded.push(2);
            encoded.push(primitive_tag(*primitive));
        }
        TyKind::Pointer { is_readonly, elem } => {
            encoded.push(3);
            write_bool(encoded, *is_readonly);
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::VolatilePointer { is_readonly, elem } => {
            encoded.push(4);
            write_bool(encoded, *is_readonly);
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::Slice { is_readonly, elem } => {
            encoded.push(5);
            write_bool(encoded, *is_readonly);
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::SlicePointee { elem } => {
            encoded.push(6);
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::Array { len, elem } => {
            encoded.push(7);
            write_array_len(encoded, len, graph)?;
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::Vector { elem, lanes } => {
            encoded.push(8);
            encoded.push(primitive_tag(*elem));
            write_u32(encoded, *lanes);
        }
        TyKind::Range { kind, bound } => {
            encoded.push(9);
            encoded.push(range_kind_tag(*kind));
            write_optional_type(encoded, *bound, graph)?;
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        } => {
            encoded.push(10);
            write_types(encoded, params, graph)?;
            write_type_index(encoded, graph.intern(*return_type)?);
            write_bool(encoded, *is_variadic);
        }
        TyKind::Optional { elem } => {
            encoded.push(11);
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::ErrorUnion { error, value } => {
            encoded.push(12);
            write_type_index(encoded, graph.intern(*error)?);
            write_type_index(encoded, graph.intern(*value)?);
        }
        TyKind::Nominal {
            def_id,
            args,
            const_args,
        } => {
            encoded.push(13);
            write_global_def(encoded, *def_id, graph.module_paths)?;
            write_types(encoded, args, graph)?;
            write_const_args(encoded, const_args, graph)?;
        }
        TyKind::BuiltinType(builtin) => {
            encoded.push(14);
            encoded.push(builtin_type_tag(*builtin));
        }
        TyKind::BuiltinTrait { trait_id, args } => {
            encoded.push(15);
            encoded.push(builtin_trait_tag(*trait_id));
            write_types(encoded, args, graph)?;
        }
        TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        } => {
            encoded.push(16);
            write_bool(encoded, *is_readonly);
            write_trait_id(encoded, *trait_id, graph.module_paths)?;
            write_types(encoded, trait_args, graph)?;
            write_const_args(encoded, trait_const_args, graph)?;
            write_associated_bindings(encoded, associated_type_bindings, graph)?;
        }
        TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        } => {
            encoded.push(17);
            write_trait_id(encoded, *trait_id, graph.module_paths)?;
            write_types(encoded, trait_args, graph)?;
            write_const_args(encoded, trait_const_args, graph)?;
            write_associated_bindings(encoded, associated_type_bindings, graph)?;
        }
        TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        } => {
            encoded.push(18);
            write_type_index(encoded, graph.intern(*self_ty)?);
            write_trait_id(encoded, *trait_id, graph.module_paths)?;
            write_types(encoded, trait_args, graph)?;
            write_const_args(encoded, trait_const_args, graph)?;
            graph.write_symbol(encoded, *name)?;
        }
        TyKind::GenericParam(name) => {
            encoded.push(19);
            graph.write_symbol(encoded, *name)?;
        }
        TyKind::SelfParam => encoded.push(20),
        TyKind::Opaque => encoded.push(21),
        TyKind::Tuple(elems) => {
            encoded.push(22);
            write_types(encoded, elems, graph)?;
        }
        TyKind::ClosureState {
            closure_id,
            captures,
            params,
            return_type,
        } => {
            encoded.push(23);
            write_global_def(encoded, closure_id.owner, graph.module_paths)?;
            write_u32(encoded, closure_id.ordinal);
            write_types(encoded, captures, graph)?;
            write_types(encoded, params, graph)?;
            write_type_index(encoded, graph.intern(*return_type)?);
        }
        TyKind::Callable {
            is_readonly,
            params,
            return_type,
        } => {
            encoded.push(24);
            write_bool(encoded, *is_readonly);
            write_types(encoded, params, graph)?;
            write_type_index(encoded, graph.intern(*return_type)?);
        }
        TyKind::CallablePointee {
            params,
            return_type,
        } => {
            encoded.push(25);
            write_types(encoded, params, graph)?;
            write_type_index(encoded, graph.intern(*return_type)?);
        }
    }
    Ok(())
}

pub(crate) fn read_ty_kind(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
) -> Option<TyKind> {
    Some(match read_u8(cursor)? {
        0 => TyKind::Error,
        1 => TyKind::ConstOnly,
        2 => TyKind::Primitive(read_primitive(cursor)?),
        3 => TyKind::Pointer {
            is_readonly: read_bool(cursor)?,
            elem: read_type_index(cursor, types)?,
        },
        4 => TyKind::VolatilePointer {
            is_readonly: read_bool(cursor)?,
            elem: read_type_index(cursor, types)?,
        },
        5 => TyKind::Slice {
            is_readonly: read_bool(cursor)?,
            elem: read_type_index(cursor, types)?,
        },
        6 => TyKind::SlicePointee {
            elem: read_type_index(cursor, types)?,
        },
        7 => TyKind::Array {
            len: read_array_len(cursor, types, symbols)?,
            elem: read_type_index(cursor, types)?,
        },
        8 => TyKind::Vector {
            elem: read_primitive(cursor)?,
            lanes: read_u32(cursor)?,
        },
        9 => TyKind::Range {
            kind: read_range_kind(cursor)?,
            bound: read_optional_type(cursor, types)?,
        },
        10 => TyKind::FunctionPointer {
            params: read_types(cursor, types)?,
            return_type: read_type_index(cursor, types)?,
            is_variadic: read_bool(cursor)?,
        },
        11 => TyKind::Optional {
            elem: read_type_index(cursor, types)?,
        },
        12 => TyKind::ErrorUnion {
            error: read_type_index(cursor, types)?,
            value: read_type_index(cursor, types)?,
        },
        13 => TyKind::Nominal {
            def_id: read_global_def(cursor, modules)?,
            args: read_types(cursor, types)?,
            const_args: read_const_args(cursor, types, symbols)?,
        },
        14 => TyKind::BuiltinType(read_builtin_type(cursor)?),
        15 => TyKind::BuiltinTrait {
            trait_id: read_builtin_trait(cursor)?,
            args: read_types(cursor, types)?,
        },
        16 => TyKind::TraitObject {
            is_readonly: read_bool(cursor)?,
            trait_id: read_trait_id(cursor, modules)?,
            trait_args: read_types(cursor, types)?,
            trait_const_args: read_const_args(cursor, types, symbols)?,
            associated_type_bindings: read_associated_bindings(cursor, types, modules, symbols)?,
        },
        17 => TyKind::TraitObjectPointee {
            trait_id: read_trait_id(cursor, modules)?,
            trait_args: read_types(cursor, types)?,
            trait_const_args: read_const_args(cursor, types, symbols)?,
            associated_type_bindings: read_associated_bindings(cursor, types, modules, symbols)?,
        },
        18 => TyKind::Projection {
            self_ty: read_type_index(cursor, types)?,
            trait_id: read_trait_id(cursor, modules)?,
            trait_args: read_types(cursor, types)?,
            trait_const_args: read_const_args(cursor, types, symbols)?,
            name: read_symbol(cursor, symbols)?,
        },
        19 => TyKind::GenericParam(read_symbol(cursor, symbols)?),
        20 => TyKind::SelfParam,
        21 => TyKind::Opaque,
        22 => TyKind::Tuple(read_types(cursor, types)?),
        23 => TyKind::ClosureState {
            closure_id: nia_ids::ClosureId {
                owner: read_global_def(cursor, modules)?,
                ordinal: read_u32(cursor)?,
            },
            captures: read_types(cursor, types)?,
            params: read_types(cursor, types)?,
            return_type: read_type_index(cursor, types)?,
        },
        24 => TyKind::Callable {
            is_readonly: read_bool(cursor)?,
            params: read_types(cursor, types)?,
            return_type: read_type_index(cursor, types)?,
        },
        25 => TyKind::CallablePointee {
            params: read_types(cursor, types)?,
            return_type: read_type_index(cursor, types)?,
        },
        _ => return None,
    })
}

pub(crate) fn write_types(
    encoded: &mut Vec<u8>,
    types: &[InternedTyId],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, types.len() as u64);
    for ty in types {
        write_type_index(encoded, graph.intern(*ty)?);
    }
    Ok(())
}

pub(crate) fn read_types(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
) -> Option<Vec<InternedTyId>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    (0..len).map(|_| read_type_index(cursor, types)).collect()
}

pub(crate) fn write_optional_type(
    encoded: &mut Vec<u8>,
    ty: Option<InternedTyId>,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    match ty {
        Some(ty) => {
            encoded.push(1);
            write_type_index(encoded, graph.intern(ty)?);
        }
        None => encoded.push(0),
    }
    Ok(())
}

pub(crate) fn read_optional_type(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
) -> Option<Option<InternedTyId>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(read_type_index(cursor, types)?)),
        _ => None,
    }
}

pub(crate) fn write_array_len(
    encoded: &mut Vec<u8>,
    len: &ArrayLenTy,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    match len {
        ArrayLenTy::Infer => encoded.push(0),
        ArrayLenTy::GenericParam(name) => {
            encoded.push(1);
            graph.write_symbol(encoded, *name)?;
        }
        ArrayLenTy::ConstValue(value) => {
            encoded.push(2);
            write_u64(encoded, *value);
        }
        ArrayLenTy::ConstExpr(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "const-expression array length is not cacheable",
            ));
        }
        ArrayLenTy::Builtin { builtin, ty } => {
            encoded.push(3);
            encoded.push(layout_builtin_tag(*builtin));
            write_type_index(encoded, graph.intern(*ty)?);
        }
    }
    Ok(())
}

pub(crate) fn read_array_len(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
) -> Option<ArrayLenTy> {
    Some(match read_u8(cursor)? {
        0 => ArrayLenTy::Infer,
        1 => ArrayLenTy::GenericParam(read_symbol(cursor, symbols)?),
        2 => ArrayLenTy::ConstValue(read_u64(cursor)?),
        3 => ArrayLenTy::Builtin {
            builtin: read_layout_builtin(cursor)?,
            ty: read_type_index(cursor, types)?,
        },
        _ => return None,
    })
}

pub(crate) fn write_const_args(
    encoded: &mut Vec<u8>,
    args: &[ConstGenericArg],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, args.len() as u64);
    for arg in args {
        write_type_index(encoded, graph.intern(arg.ty)?);
        match arg.value {
            ConstGenericValue::GenericParam(name) => {
                encoded.push(0);
                graph.write_symbol(encoded, name)?;
            }
            ConstGenericValue::ConstExpr(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "const-expression generic argument is not cacheable",
                ));
            }
            ConstGenericValue::Int(value) => {
                encoded.push(1);
                write_u128(encoded, value.bits());
                write_bool(encoded, value.is_signed());
            }
            ConstGenericValue::Bool(value) => {
                encoded.push(2);
                write_bool(encoded, value);
            }
            ConstGenericValue::Char(value) => {
                encoded.push(3);
                write_u32(encoded, value as u32);
            }
        }
    }
    Ok(())
}

pub(crate) fn read_const_args(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
) -> Option<Vec<ConstGenericArg>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut args = Vec::with_capacity(len);
    for _ in 0..len {
        let ty = read_type_index(cursor, types)?;
        let value = match read_u8(cursor)? {
            0 => ConstGenericValue::GenericParam(read_symbol(cursor, symbols)?),
            1 => {
                let bits = read_u128(cursor)?;
                ConstGenericValue::Int(if read_bool(cursor)? {
                    IntConst::signed_bits(bits)
                } else {
                    IntConst::unsigned(bits)
                })
            }
            2 => ConstGenericValue::Bool(read_bool(cursor)?),
            3 => ConstGenericValue::Char(char::from_u32(read_u32(cursor)?)?),
            _ => return None,
        };
        args.push(ConstGenericArg { ty, value });
    }
    Some(args)
}

pub(crate) fn write_associated_bindings(
    encoded: &mut Vec<u8>,
    bindings: &[AssociatedTypeBindingTy],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, bindings.len() as u64);
    for binding in bindings {
        match binding.trait_id {
            Some(trait_id) => {
                encoded.push(1);
                write_trait_id(encoded, trait_id, graph.module_paths)?;
            }
            None => encoded.push(0),
        }
        write_types(encoded, &binding.trait_args, graph)?;
        write_const_args(encoded, &binding.trait_const_args, graph)?;
        graph.write_symbol(encoded, binding.name)?;
        write_type_index(encoded, graph.intern(binding.ty)?);
    }
    Ok(())
}

pub(crate) fn read_associated_bindings(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
) -> Option<Vec<AssociatedTypeBindingTy>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut bindings = Vec::with_capacity(len);
    for _ in 0..len {
        let trait_id = match read_u8(cursor)? {
            0 => None,
            1 => Some(read_trait_id(cursor, modules)?),
            _ => return None,
        };
        bindings.push(AssociatedTypeBindingTy {
            trait_id,
            trait_args: read_types(cursor, types)?,
            trait_const_args: read_const_args(cursor, types, symbols)?,
            name: read_symbol(cursor, symbols)?,
            ty: read_type_index(cursor, types)?,
        });
    }
    Some(bindings)
}

pub(crate) fn write_trait_id(
    encoded: &mut Vec<u8>,
    trait_id: TraitId,
    module_paths: &HashMap<ModuleId, String>,
) -> io::Result<()> {
    match trait_id {
        TraitId::Source(def_id) => {
            encoded.push(0);
            write_global_def(encoded, def_id, module_paths)?;
        }
        TraitId::Builtin(builtin) => {
            encoded.push(1);
            encoded.push(builtin_trait_tag(builtin));
        }
    }
    Ok(())
}

pub(crate) fn read_trait_id(
    cursor: &mut Cursor<&[u8]>,
    modules: &HashMap<String, ModuleId>,
) -> Option<TraitId> {
    match read_u8(cursor)? {
        0 => Some(TraitId::Source(read_global_def(cursor, modules)?)),
        1 => Some(TraitId::Builtin(read_builtin_trait(cursor)?)),
        _ => None,
    }
}

pub(crate) fn read_symbol(
    cursor: &mut Cursor<&[u8]>,
    symbols: &SymbolTable,
) -> Option<nia_symbol::SymbolId> {
    let text = read_string(cursor, cursor.get_ref().len())?;
    symbols.intern(&text).ok()
}

pub(crate) fn write_type_index(encoded: &mut Vec<u8>, index: u32) {
    write_u64(encoded, u64::from(index));
}

pub(crate) fn read_type_index(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
) -> Option<InternedTyId> {
    types.get(usize::try_from(read_u64(cursor)?).ok()?).copied()
}

pub(crate) fn write_bool(encoded: &mut Vec<u8>, value: bool) {
    encoded.push(u8::from(value));
}

pub(crate) fn read_bool(cursor: &mut Cursor<&[u8]>) -> Option<bool> {
    match read_u8(cursor)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

pub(crate) fn range_kind_tag(value: RangeTyKind) -> u8 {
    match value {
        RangeTyKind::Exclusive => 0,
        RangeTyKind::Inclusive => 1,
        RangeTyKind::From => 2,
        RangeTyKind::To => 3,
        RangeTyKind::ToInclusive => 4,
        RangeTyKind::Full => 5,
    }
}

pub(crate) fn read_range_kind(cursor: &mut Cursor<&[u8]>) -> Option<RangeTyKind> {
    Some(match read_u8(cursor)? {
        0 => RangeTyKind::Exclusive,
        1 => RangeTyKind::Inclusive,
        2 => RangeTyKind::From,
        3 => RangeTyKind::To,
        4 => RangeTyKind::ToInclusive,
        5 => RangeTyKind::Full,
        _ => return None,
    })
}

pub(crate) fn layout_builtin_tag(value: LayoutBuiltin) -> u8 {
    match value {
        LayoutBuiltin::Size => 0,
        LayoutBuiltin::Align => 1,
    }
}

pub(crate) fn read_layout_builtin(cursor: &mut Cursor<&[u8]>) -> Option<LayoutBuiltin> {
    Some(match read_u8(cursor)? {
        0 => LayoutBuiltin::Size,
        1 => LayoutBuiltin::Align,
        _ => return None,
    })
}

pub(crate) fn builtin_type_tag(value: BuiltinType) -> u8 {
    // These tags are part of the persistent signature-cache format. Existing
    // values may never be renumbered; new builtin types must append new tags.
    match value {
        BuiltinType::AsmConfig => 0,
        BuiltinType::AsmInputs => 1,
        BuiltinType::AsmOutputs => 2,
    }
}

pub(crate) fn read_builtin_type(cursor: &mut Cursor<&[u8]>) -> Option<BuiltinType> {
    match read_u8(cursor)? {
        0 => Some(BuiltinType::AsmConfig),
        1 => Some(BuiltinType::AsmInputs),
        2 => Some(BuiltinType::AsmOutputs),
        _ => None,
    }
}
