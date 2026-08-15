// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable encoding for public item-signature tables.

use super::*;

pub(crate) fn encode_item_signatures(
    signatures: &ItemSignatures,
    module_paths: &HashMap<ModuleId, String>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
) -> io::Result<Vec<u8>> {
    if !signatures.diagnostics.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot cache item signature diagnostics",
        ));
    }
    let mut graph = TypeGraphEncoder {
        type_store,
        module_paths,
        symbols,
        indexes: HashMap::new(),
        visiting: HashSet::new(),
        nodes: Vec::new(),
    };
    let mut body = Vec::new();
    write_def_map(&mut body, &signatures.functions, |encoded, signature| {
        write_function_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.structs, |encoded, signature| {
        write_struct_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.unions, |encoded, signature| {
        write_union_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.traits, |encoded, signature| {
        write_trait_signature(encoded, signature, &mut graph)
    })?;
    write_u64(&mut body, signatures.trait_impls.len() as u64);
    for signature in &signatures.trait_impls {
        write_trait_impl_signature(&mut body, signature, &mut graph)?;
    }
    write_def_map(&mut body, &signatures.enums, |encoded, signature| {
        write_enum_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.type_aliases, |encoded, signature| {
        write_type_alias_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.globals, |encoded, signature| {
        write_global_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.consts, |encoded, signature| {
        write_const_signature(encoded, signature, &mut graph)
    })?;

    let mut encoded = Vec::new();
    write_type_graph(&mut encoded, graph.nodes);
    encoded.extend_from_slice(&body);
    Ok(encoded)
}

pub(crate) fn decode_item_signatures(
    encoded: &[u8],
    source_len: usize,
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
    module_id: ModuleId,
) -> Option<ItemSignatures> {
    let mut cursor = Cursor::new(encoded);
    let types = read_type_graph(
        &mut cursor,
        encoded.len(),
        modules,
        symbols,
        type_store,
        module_id,
    )?;
    let functions = read_def_map(&mut cursor, |cursor| {
        read_function_signature(cursor, &types, symbols, source_len)
    })?;
    let structs = read_def_map(&mut cursor, |cursor| {
        read_struct_signature(cursor, &types, symbols, source_len)
    })?;
    let unions = read_def_map(&mut cursor, |cursor| {
        read_union_signature(cursor, &types, symbols, source_len)
    })?;
    let traits = read_def_map(&mut cursor, |cursor| {
        read_trait_signature(cursor, &types, symbols, source_len)
    })?;
    let trait_impl_len = read_len(&mut cursor, MAX_SEQUENCE_LEN)?;
    let mut trait_impls = Vec::with_capacity(trait_impl_len);
    let mut trait_impl_ids = HashSet::new();
    for _ in 0..trait_impl_len {
        let signature = read_trait_impl_signature(&mut cursor, &types, symbols, source_len)?;
        if !trait_impl_ids.insert(signature.impl_id) {
            return None;
        }
        trait_impls.push(signature);
    }
    let enums = read_def_map(&mut cursor, |cursor| {
        read_enum_signature(cursor, &types, symbols, source_len)
    })?;
    let type_aliases = read_def_map(&mut cursor, |cursor| {
        read_type_alias_signature(cursor, &types, symbols, source_len)
    })?;
    let globals = read_def_map(&mut cursor, |cursor| {
        read_global_signature(cursor, &types, source_len)
    })?;
    let consts = read_def_map(&mut cursor, |cursor| {
        read_const_signature(cursor, &types, source_len)
    })?;
    if cursor.position() as usize != encoded.len() {
        return None;
    }
    Some(ItemSignatures {
        functions,
        structs,
        unions,
        traits,
        trait_impls,
        enums,
        type_aliases,
        globals,
        consts,
        diagnostics: Vec::new(),
    })
}

pub(crate) fn write_def_map<T>(
    encoded: &mut Vec<u8>,
    values: &HashMap<DefId, T>,
    mut write_value: impl FnMut(&mut Vec<u8>, &T) -> io::Result<()>,
) -> io::Result<()> {
    let mut values = values.iter().collect::<Vec<_>>();
    values.sort_unstable_by_key(|(def_id, _)| def_id.0);
    write_u64(encoded, values.len() as u64);
    for (def_id, value) in values {
        write_u64(encoded, def_id.0);
        write_value(encoded, value)?;
    }
    Ok(())
}

pub(crate) fn read_def_map<T>(
    cursor: &mut Cursor<&[u8]>,
    mut read_value: impl FnMut(&mut Cursor<&[u8]>) -> Option<T>,
) -> Option<HashMap<DefId, T>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut values = HashMap::with_capacity(len);
    let mut previous = None;
    for _ in 0..len {
        let def_id = DefId(read_u64(cursor)?);
        if previous.is_some_and(|previous| previous >= def_id) {
            return None;
        }
        previous = Some(def_id);
        if values.insert(def_id, read_value(cursor)?).is_some() {
            return None;
        }
    }
    Some(values)
}

pub(crate) fn write_function_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::FunctionSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    graph.write_symbol(encoded, signature.name)?;
    write_symbols(encoded, &signature.generics, graph)?;
    write_generic_params(encoded, &signature.generic_params, graph)?;
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_u64(encoded, signature.params.len() as u64);
    for param in &signature.params {
        write_optional_symbol(encoded, param.name, graph)?;
        write_optional_receiver(encoded, param.receiver);
        write_type_index(encoded, graph.intern(param.ty)?);
        write_span(encoded, param.span);
    }
    write_type_index(encoded, graph.intern(signature.return_type)?);
    write_bool(encoded, signature.is_extern);
    write_bool(encoded, signature.is_const);
    write_bool(encoded, signature.is_variadic);
    write_u64(encoded, signature.attributes.len() as u64);
    for attribute in &signature.attributes {
        match attribute {
            item_signatures::FunctionAttribute::Naked => encoded.push(0),
            item_signatures::FunctionAttribute::Builtin(builtin) => {
                encoded.push(1);
                encoded.push(builtin_function_tag(*builtin));
            }
        }
    }
    write_bool(encoded, signature.has_body);
    write_span(encoded, signature.span);
    Ok(())
}

pub(crate) fn read_function_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::FunctionSignature> {
    let function_name = read_symbol(cursor, symbols)?;
    let generics = read_symbols(cursor, symbols)?;
    let generic_params = read_generic_params(cursor, types, symbols)?;
    let where_predicates = read_where_predicates(cursor, types, symbols, source_len)?;
    let param_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut params = Vec::with_capacity(param_len);
    for _ in 0..param_len {
        params.push(item_signatures::ParamSignature {
            name: read_optional_symbol(cursor, symbols)?,
            receiver: read_optional_receiver(cursor)?,
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        });
    }
    let return_type = read_type_index(cursor, types)?;
    let is_extern = read_bool(cursor)?;
    let is_const = read_bool(cursor)?;
    let is_variadic = read_bool(cursor)?;
    let attribute_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut attributes = Vec::with_capacity(attribute_len);
    for _ in 0..attribute_len {
        attributes.push(match read_u8(cursor)? {
            0 => item_signatures::FunctionAttribute::Naked,
            1 => item_signatures::FunctionAttribute::Builtin(read_builtin_function(cursor)?),
            _ => return None,
        });
    }
    Some(item_signatures::FunctionSignature {
        name: function_name,
        generics,
        generic_params,
        where_predicates,
        params,
        return_type,
        is_extern,
        is_const,
        is_variadic,
        attributes,
        has_body: read_bool(cursor)?,
        span: read_span(cursor, source_len)?,
    })
}

pub(crate) fn write_generic_params(
    encoded: &mut Vec<u8>,
    params: &[item_signatures::GenericParamSignature],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, params.len() as u64);
    for param in params {
        graph.write_symbol(encoded, param.name)?;
        match &param.kind {
            item_signatures::GenericParamSignatureKind::Type => encoded.push(0),
            item_signatures::GenericParamSignatureKind::Const { ty } => {
                encoded.push(1);
                write_type_index(encoded, graph.intern(*ty)?);
            }
        }
    }
    Ok(())
}

pub(crate) fn read_generic_params(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
) -> Option<Vec<item_signatures::GenericParamSignature>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut params = Vec::with_capacity(len);
    for _ in 0..len {
        let name = read_symbol(cursor, symbols)?;
        let kind = match read_u8(cursor)? {
            0 => item_signatures::GenericParamSignatureKind::Type,
            1 => item_signatures::GenericParamSignatureKind::Const {
                ty: read_type_index(cursor, types)?,
            },
            _ => return None,
        };
        params.push(item_signatures::GenericParamSignature { name, kind });
    }
    Some(params)
}

pub(crate) fn write_struct_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::StructSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_symbols(encoded, &signature.generics, graph)?;
    write_generic_params(encoded, &signature.generic_params, graph)?;
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_fields(encoded, &signature.fields, graph)?;
    // Keep the tuple-shape bit adjacent to fields; changing this order makes
    // old cache payloads fail decoding rather than silently changing layout.
    write_bool(encoded, signature.is_tuple);
    write_bool(encoded, signature.is_extern);
    write_span(encoded, signature.span);
    Ok(())
}

pub(crate) fn read_struct_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::StructSignature> {
    Some(item_signatures::StructSignature {
        generics: read_symbols(cursor, symbols)?,
        generic_params: read_generic_params(cursor, types, symbols)?,
        where_predicates: read_where_predicates(cursor, types, symbols, source_len)?,
        fields: read_fields(cursor, types, symbols, source_len)?,
        is_tuple: read_bool(cursor)?,
        is_extern: read_bool(cursor)?,
        span: read_span(cursor, source_len)?,
    })
}

pub(crate) fn write_union_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::UnionSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_symbols(encoded, &signature.generics, graph)?;
    write_generic_params(encoded, &signature.generic_params, graph)?;
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_fields(encoded, &signature.fields, graph)?;
    write_bool(encoded, signature.is_extern);
    write_span(encoded, signature.span);
    Ok(())
}

pub(crate) fn read_union_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::UnionSignature> {
    Some(item_signatures::UnionSignature {
        generics: read_symbols(cursor, symbols)?,
        generic_params: read_generic_params(cursor, types, symbols)?,
        where_predicates: read_where_predicates(cursor, types, symbols, source_len)?,
        fields: read_fields(cursor, types, symbols, source_len)?,
        is_extern: read_bool(cursor)?,
        span: read_span(cursor, source_len)?,
    })
}

pub(crate) fn write_trait_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::TraitSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_symbols(encoded, &signature.generics, graph)?;
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_u64(encoded, signature.supertraits.len() as u64);
    for supertrait in &signature.supertraits {
        write_type_index(encoded, graph.intern(supertrait.ty)?);
        write_span(encoded, supertrait.span);
    }
    write_u64(encoded, signature.associated_types.len() as u64);
    for associated in &signature.associated_types {
        write_u64(encoded, associated.def_id.0);
        graph.write_symbol(encoded, associated.name)?;
        write_span(encoded, associated.span);
    }
    write_u64(encoded, signature.associated_values.len() as u64);
    for associated in &signature.associated_values {
        write_u64(encoded, associated.def_id.0);
        graph.write_symbol(encoded, associated.name)?;
        write_type_index(encoded, graph.intern(associated.ty)?);
        write_span(encoded, associated.span);
    }
    write_u64(encoded, signature.methods.len() as u64);
    for method in &signature.methods {
        write_u64(encoded, method.def_id.0);
        graph.write_symbol(encoded, method.name)?;
        write_function_signature(encoded, &method.signature, graph)?;
        write_bool(encoded, method.has_default);
        write_span(encoded, method.span);
    }
    match signature.builtin {
        Some(builtin) => {
            encoded.push(1);
            encoded.push(builtin_trait_tag(builtin));
        }
        None => encoded.push(0),
    }
    write_span(encoded, signature.span);
    Ok(())
}

pub(crate) fn read_trait_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::TraitSignature> {
    let generics = read_symbols(cursor, symbols)?;
    let where_predicates = read_where_predicates(cursor, types, symbols, source_len)?;
    let supertrait_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut supertraits = Vec::with_capacity(supertrait_len);
    for _ in 0..supertrait_len {
        supertraits.push(item_signatures::TraitSupertraitSignature {
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        });
    }
    let associated_type_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut associated_types = Vec::with_capacity(associated_type_len);
    let mut associated_type_ids = HashSet::new();
    for _ in 0..associated_type_len {
        let value = item_signatures::TraitAssociatedTypeSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            span: read_span(cursor, source_len)?,
        };
        if !associated_type_ids.insert(value.def_id) {
            return None;
        }
        associated_types.push(value);
    }
    let associated_value_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut associated_values = Vec::with_capacity(associated_value_len);
    let mut associated_value_ids = HashSet::new();
    for _ in 0..associated_value_len {
        let value = item_signatures::TraitAssociatedValueSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        };
        if !associated_value_ids.insert(value.def_id) {
            return None;
        }
        associated_values.push(value);
    }
    let method_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut methods = Vec::with_capacity(method_len);
    let mut method_ids = HashSet::new();
    for _ in 0..method_len {
        let method = item_signatures::TraitMethodSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            signature: read_function_signature(cursor, types, symbols, source_len)?,
            has_default: read_bool(cursor)?,
            span: read_span(cursor, source_len)?,
        };
        if !method_ids.insert(method.def_id) {
            return None;
        }
        methods.push(method);
    }
    let builtin = match read_u8(cursor)? {
        0 => None,
        1 => Some(read_builtin_trait(cursor)?),
        _ => return None,
    };
    Some(item_signatures::TraitSignature {
        generics,
        where_predicates,
        supertraits,
        associated_types,
        associated_values,
        methods,
        builtin,
        span: read_span(cursor, source_len)?,
    })
}

pub(crate) fn write_trait_impl_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::TraitImplSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, signature.impl_id.0);
    write_optional_string(encoded, signature.builtin.as_deref());
    write_symbols(encoded, &signature.generics, graph)?;
    write_generic_params(encoded, &signature.generic_params, graph)?;
    write_type_index(encoded, graph.intern(signature.target_ty)?);
    write_optional_type(encoded, signature.trait_ty, graph)?;
    write_optional_span(encoded, signature.trait_span);
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_u64(encoded, signature.associated_types.len() as u64);
    for associated in &signature.associated_types {
        graph.write_symbol(encoded, associated.name)?;
        write_type_index(encoded, graph.intern(associated.ty)?);
        write_span(encoded, associated.span);
    }
    write_u64(encoded, signature.associated_values.len() as u64);
    for associated in &signature.associated_values {
        write_u64(encoded, associated.def_id.0);
        graph.write_symbol(encoded, associated.name)?;
        encoded.push(visibility_tag(associated.visibility));
        write_span(encoded, associated.span);
    }
    write_u64(encoded, signature.methods.len() as u64);
    for method in &signature.methods {
        write_u64(encoded, method.def_id.0);
        graph.write_symbol(encoded, method.name)?;
        encoded.push(visibility_tag(method.visibility));
        write_span(encoded, method.span);
    }
    write_span(encoded, signature.span);
    Ok(())
}

pub(crate) fn read_trait_impl_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::TraitImplSignature> {
    let impl_id = TraitImplId(read_u64(cursor)?);
    let builtin = read_optional_string(cursor)?;
    let generics = read_symbols(cursor, symbols)?;
    let generic_params = read_generic_params(cursor, types, symbols)?;
    let target_ty = read_type_index(cursor, types)?;
    let trait_ty = read_optional_type(cursor, types)?;
    let trait_span = read_optional_span(cursor, source_len)?;
    if trait_ty.is_some() != trait_span.is_some() {
        return None;
    }
    let where_predicates = read_where_predicates(cursor, types, symbols, source_len)?;
    let associated_type_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut associated_types = Vec::with_capacity(associated_type_len);
    for _ in 0..associated_type_len {
        associated_types.push(item_signatures::TraitImplAssociatedTypeSignature {
            name: read_symbol(cursor, symbols)?,
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        });
    }
    let associated_value_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut associated_values = Vec::with_capacity(associated_value_len);
    let mut associated_value_ids = HashSet::new();
    for _ in 0..associated_value_len {
        let value = item_signatures::TraitImplAssociatedValueSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            visibility: read_visibility(cursor)?,
            span: read_span(cursor, source_len)?,
        };
        if !associated_value_ids.insert(value.def_id) {
            return None;
        }
        associated_values.push(value);
    }
    let method_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut methods = Vec::with_capacity(method_len);
    let mut method_ids = HashSet::new();
    for _ in 0..method_len {
        let method = item_signatures::TraitImplMethodSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            visibility: read_visibility(cursor)?,
            span: read_span(cursor, source_len)?,
        };
        if !method_ids.insert(method.def_id) {
            return None;
        }
        methods.push(method);
    }
    Some(item_signatures::TraitImplSignature {
        impl_id,
        builtin,
        generics,
        generic_params,
        target_ty,
        trait_ty,
        trait_span,
        where_predicates,
        associated_types,
        associated_values,
        methods,
        span: read_span(cursor, source_len)?,
    })
}

pub(crate) fn write_enum_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::EnumSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_type_index(encoded, graph.intern(signature.backing_type)?);
    write_bool(encoded, signature.is_open);
    write_u64(encoded, signature.variants.len() as u64);
    for variant in &signature.variants {
        write_u64(encoded, variant.def_id.0);
        graph.write_symbol(encoded, variant.name)?;
        match &variant.payload {
            item_signatures::EnumVariantPayloadSignature::Unit => encoded.push(0),
            item_signatures::EnumVariantPayloadSignature::Tuple(fields) => {
                encoded.push(1);
                write_u64(encoded, fields.len() as u64);
                for field in fields {
                    write_type_index(encoded, graph.intern(*field)?);
                }
            }
            item_signatures::EnumVariantPayloadSignature::Named(fields) => {
                encoded.push(2);
                write_fields(encoded, fields, graph)?;
            }
        }
        write_span(encoded, variant.span);
    }
    write_span(encoded, signature.span);
    Ok(())
}

pub(crate) fn read_enum_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::EnumSignature> {
    let backing_type = read_type_index(cursor, types)?;
    let is_open = read_bool(cursor)?;
    let variant_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut variants = Vec::with_capacity(variant_len);
    let mut variant_ids = HashSet::new();
    for _ in 0..variant_len {
        let def_id = DefId(read_u64(cursor)?);
        let name = read_symbol(cursor, symbols)?;
        let payload = match read_u8(cursor)? {
            0 => item_signatures::EnumVariantPayloadSignature::Unit,
            1 => {
                let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
                let mut fields = Vec::with_capacity(len);
                for _ in 0..len {
                    fields.push(read_type_index(cursor, types)?);
                }
                item_signatures::EnumVariantPayloadSignature::Tuple(fields)
            }
            2 => item_signatures::EnumVariantPayloadSignature::Named(read_fields(
                cursor, types, symbols, source_len,
            )?),
            _ => return None,
        };
        let variant = item_signatures::EnumVariantSignature {
            def_id,
            name,
            payload,
            span: read_span(cursor, source_len)?,
        };
        if !variant_ids.insert(variant.def_id) {
            return None;
        }
        variants.push(variant);
    }
    Some(item_signatures::EnumSignature {
        backing_type,
        is_open,
        variants,
        span: read_span(cursor, source_len)?,
    })
}

pub(crate) fn write_type_alias_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::TypeAliasSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_symbols(encoded, &signature.generics, graph)?;
    write_type_index(encoded, graph.intern(signature.target)?);
    write_span(encoded, signature.span);
    Ok(())
}

pub(crate) fn read_type_alias_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::TypeAliasSignature> {
    Some(item_signatures::TypeAliasSignature {
        generics: read_symbols(cursor, symbols)?,
        target: read_type_index(cursor, types)?,
        span: read_span(cursor, source_len)?,
    })
}

pub(crate) fn write_global_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::GlobalSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_optional_type(encoded, signature.explicit_type, graph)?;
    write_bool(encoded, signature.is_mutable);
    write_bool(encoded, signature.is_extern);
    write_span(encoded, signature.span);
    Ok(())
}

pub(crate) fn read_global_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    source_len: usize,
) -> Option<item_signatures::GlobalSignature> {
    Some(item_signatures::GlobalSignature {
        explicit_type: read_optional_type(cursor, types)?,
        is_mutable: read_bool(cursor)?,
        is_extern: read_bool(cursor)?,
        span: read_span(cursor, source_len)?,
    })
}

pub(crate) fn write_const_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::ConstSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_optional_type(encoded, signature.explicit_type, graph)?;
    match signature.builtin {
        Some(builtin) => {
            encoded.push(1);
            encoded.push(builtin_const_tag(builtin));
        }
        None => encoded.push(0),
    }
    write_span(encoded, signature.span);
    Ok(())
}

pub(crate) fn read_const_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    source_len: usize,
) -> Option<item_signatures::ConstSignature> {
    let explicit_type = read_optional_type(cursor, types)?;
    let builtin = match read_u8(cursor)? {
        0 => None,
        1 => Some(read_builtin_const(cursor)?),
        _ => return None,
    };
    Some(item_signatures::ConstSignature {
        explicit_type,
        builtin,
        span: read_span(cursor, source_len)?,
    })
}

pub(crate) fn write_fields(
    encoded: &mut Vec<u8>,
    fields: &[item_signatures::FieldSignature],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, fields.len() as u64);
    for field in fields {
        write_u64(encoded, field.def_id.0);
        graph.write_symbol(encoded, field.name)?;
        write_type_index(encoded, graph.intern(field.ty)?);
        write_span(encoded, field.span);
    }
    Ok(())
}

pub(crate) fn read_fields(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<Vec<item_signatures::FieldSignature>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut fields = Vec::with_capacity(len);
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for _ in 0..len {
        let field = item_signatures::FieldSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        };
        if !ids.insert(field.def_id) || !names.insert(field.name) {
            return None;
        }
        fields.push(field);
    }
    Some(fields)
}

pub(crate) fn write_where_predicates(
    encoded: &mut Vec<u8>,
    predicates: &[item_signatures::WherePredicateSignature],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, predicates.len() as u64);
    for predicate in predicates {
        write_type_index(encoded, graph.intern(predicate.ty)?);
        write_u64(encoded, predicate.bounds.len() as u64);
        for bound in &predicate.bounds {
            write_type_index(encoded, graph.intern(bound.trait_ty)?);
            write_u64(encoded, bound.associated_type_bindings.len() as u64);
            for binding in &bound.associated_type_bindings {
                graph.write_symbol(encoded, binding.name)?;
                write_type_index(encoded, graph.intern(binding.ty)?);
                write_span(encoded, binding.span);
            }
            write_span(encoded, bound.span);
        }
        write_span(encoded, predicate.span);
    }
    Ok(())
}

pub(crate) fn read_where_predicates(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<Vec<item_signatures::WherePredicateSignature>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut predicates = Vec::with_capacity(len);
    for _ in 0..len {
        let ty = read_type_index(cursor, types)?;
        let bound_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
        let mut bounds = Vec::with_capacity(bound_len);
        for _ in 0..bound_len {
            let trait_ty = read_type_index(cursor, types)?;
            let binding_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
            let mut associated_type_bindings = Vec::with_capacity(binding_len);
            for _ in 0..binding_len {
                associated_type_bindings.push(item_signatures::AssociatedTypeBindingSignature {
                    name: read_symbol(cursor, symbols)?,
                    ty: read_type_index(cursor, types)?,
                    span: read_span(cursor, source_len)?,
                });
            }
            bounds.push(item_signatures::WhereBoundSignature {
                trait_ty,
                associated_type_bindings,
                span: read_span(cursor, source_len)?,
            });
        }
        predicates.push(item_signatures::WherePredicateSignature {
            ty,
            bounds,
            span: read_span(cursor, source_len)?,
        });
    }
    Some(predicates)
}

pub(crate) fn write_symbols(
    encoded: &mut Vec<u8>,
    symbols: &[nia_symbol::SymbolId],
    graph: &TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, symbols.len() as u64);
    for symbol in symbols {
        graph.write_symbol(encoded, *symbol)?;
    }
    Ok(())
}

pub(crate) fn read_symbols(
    cursor: &mut Cursor<&[u8]>,
    symbols: &SymbolTable,
) -> Option<Vec<nia_symbol::SymbolId>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    (0..len).map(|_| read_symbol(cursor, symbols)).collect()
}

pub(crate) fn write_optional_symbol(
    encoded: &mut Vec<u8>,
    symbol: Option<nia_symbol::SymbolId>,
    graph: &TypeGraphEncoder<'_>,
) -> io::Result<()> {
    match symbol {
        Some(symbol) => {
            encoded.push(1);
            graph.write_symbol(encoded, symbol)?;
        }
        None => encoded.push(0),
    }
    Ok(())
}

pub(crate) fn read_optional_symbol(
    cursor: &mut Cursor<&[u8]>,
    symbols: &SymbolTable,
) -> Option<Option<nia_symbol::SymbolId>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(read_symbol(cursor, symbols)?)),
        _ => None,
    }
}

pub(crate) fn write_span(encoded: &mut Vec<u8>, span: nia_span::Span) {
    write_u64(encoded, span.start as u64);
    write_u64(encoded, span.end as u64);
}

pub(crate) fn read_span(cursor: &mut Cursor<&[u8]>, source_len: usize) -> Option<nia_span::Span> {
    let start = usize::try_from(read_u64(cursor)?).ok()?;
    let end = usize::try_from(read_u64(cursor)?).ok()?;
    (start <= end && end <= source_len).then(|| nia_span::Span::new(start, end))
}

pub(crate) fn write_optional_span(encoded: &mut Vec<u8>, span: Option<nia_span::Span>) {
    match span {
        Some(span) => {
            encoded.push(1);
            write_span(encoded, span);
        }
        None => encoded.push(0),
    }
}

pub(crate) fn read_optional_span(
    cursor: &mut Cursor<&[u8]>,
    source_len: usize,
) -> Option<Option<nia_span::Span>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(read_span(cursor, source_len)?)),
        _ => None,
    }
}

pub(crate) fn write_optional_string(encoded: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            encoded.push(1);
            write_string(encoded, value);
        }
        None => encoded.push(0),
    }
}

pub(crate) fn read_optional_string(cursor: &mut Cursor<&[u8]>) -> Option<Option<String>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(read_string(cursor, cursor.get_ref().len())?)),
        _ => None,
    }
}

pub(crate) fn write_optional_receiver(encoded: &mut Vec<u8>, receiver: Option<ReceiverKind>) {
    encoded.push(match receiver {
        None => 0,
        Some(ReceiverKind::RefReadOnly) => 1,
        Some(ReceiverKind::Ref) => 2,
        Some(ReceiverKind::Value) => 3,
    });
}

pub(crate) fn read_optional_receiver(cursor: &mut Cursor<&[u8]>) -> Option<Option<ReceiverKind>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(ReceiverKind::RefReadOnly)),
        2 => Some(Some(ReceiverKind::Ref)),
        3 => Some(Some(ReceiverKind::Value)),
        _ => None,
    }
}

pub(crate) fn visibility_tag(visibility: Visibility) -> u8 {
    match visibility {
        Visibility::Private => 0,
        Visibility::PublicSuper => 1,
        Visibility::PublicPkg => 2,
        Visibility::Public => 3,
    }
}

pub(crate) fn read_visibility(cursor: &mut Cursor<&[u8]>) -> Option<Visibility> {
    Some(match read_u8(cursor)? {
        0 => Visibility::Private,
        1 => Visibility::PublicSuper,
        2 => Visibility::PublicPkg,
        3 => Visibility::Public,
        _ => return None,
    })
}

pub(crate) fn builtin_function_tag(value: BuiltinFunction) -> u8 {
    BuiltinFunction::ALL
        .iter()
        .position(|candidate| *candidate == value)
        .expect("all builtin functions have stable tags") as u8
}

pub(crate) fn read_builtin_function(cursor: &mut Cursor<&[u8]>) -> Option<BuiltinFunction> {
    BuiltinFunction::ALL.get(read_u8(cursor)? as usize).copied()
}

const BUILTIN_CONSTS: [BuiltinConstValue; 7] = [
    BuiltinConstValue::TargetArch,
    BuiltinConstValue::TargetVendor,
    BuiltinConstValue::TargetOs,
    BuiltinConstValue::TargetEnv,
    BuiltinConstValue::TargetAbi,
    BuiltinConstValue::TargetEndian,
    BuiltinConstValue::TargetPointerWidth,
];

pub(crate) fn builtin_const_tag(value: BuiltinConstValue) -> u8 {
    BUILTIN_CONSTS
        .iter()
        .position(|candidate| *candidate == value)
        .expect("all builtin consts have stable tags") as u8
}

pub(crate) fn read_builtin_const(cursor: &mut Cursor<&[u8]>) -> Option<BuiltinConstValue> {
    BUILTIN_CONSTS.get(read_u8(cursor)? as usize).copied()
}
