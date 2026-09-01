// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic syntax/type-resolution node encoding.

use super::*;

pub(crate) fn write_sorted_entries(
    encoded: &mut Vec<u8>,
    entries: impl IntoIterator<Item = io::Result<Vec<u8>>>,
) -> io::Result<()> {
    let mut entries = entries.into_iter().collect::<io::Result<Vec<_>>>()?;
    // Hash-backed compiler maps have no stable iteration order. Sorting complete encoded records
    // makes cache bytes and their checksums reproducible across processes.
    entries.sort_unstable();
    write_u64(encoded, entries.len() as u64);
    for entry in entries {
        write_u64(encoded, entry.len() as u64);
        encoded.extend_from_slice(&entry);
    }
    Ok(())
}

pub(crate) fn read_entries<'a>(
    cursor: &mut Cursor<&'a [u8]>,
    encoded_len: usize,
) -> Option<Vec<&'a [u8]>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut entries = Vec::with_capacity(len);
    for _ in 0..len {
        let entry_len = read_len(cursor, encoded_len)?;
        let start = usize::try_from(cursor.position()).ok()?;
        let end = start.checked_add(entry_len)?;
        let entry = cursor.get_ref().get(start..end)?;
        cursor.set_position(u64::try_from(end).ok()?);
        entries.push(entry);
    }
    Some(entries)
}

pub(crate) fn write_node_site(
    encoded: &mut Vec<u8>,
    site: &NodeSite,
    source_id: nia_source::SourceId,
) -> io::Result<()> {
    if site.source_id != source_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "type resolution node belongs to another source",
        ));
    }
    encoded.push(syntax_kind_tag(site.kind));
    match &site.position {
        NodePosition::Span(span) => {
            encoded.push(0);
            write_u64(encoded, span.start as u64);
            write_u64(encoded, span.end as u64);
        }
        NodePosition::ChildPath(path) => {
            encoded.push(1);
            write_child_path(encoded, path);
        }
        NodePosition::ChildPathRange { start, end } => {
            encoded.push(2);
            write_child_path(encoded, start);
            write_child_path(encoded, end);
        }
    }
    Ok(())
}

pub(crate) fn read_node_site(
    cursor: &mut Cursor<&[u8]>,
    source_id: nia_source::SourceId,
    source_len: usize,
) -> Option<NodeSite> {
    let kind = read_syntax_kind(cursor)?;
    let position = match read_u8(cursor)? {
        0 => {
            let start = usize::try_from(read_u64(cursor)?).ok()?;
            let end = usize::try_from(read_u64(cursor)?).ok()?;
            if start > end || end > source_len {
                return None;
            }
            NodePosition::Span(nia_span::Span::new(start, end))
        }
        1 => NodePosition::ChildPath(read_child_path(cursor)?),
        2 => NodePosition::ChildPathRange {
            start: read_child_path(cursor)?,
            end: read_child_path(cursor)?,
        },
        _ => return None,
    };
    Some(NodeSite {
        source_id,
        kind,
        position,
    })
}

pub(crate) fn write_child_path(encoded: &mut Vec<u8>, path: &NodeChildPath) {
    write_u64(encoded, path.steps().len() as u64);
    for step in path.steps() {
        encoded.extend_from_slice(&step.to_le_bytes());
    }
}

pub(crate) fn read_child_path(cursor: &mut Cursor<&[u8]>) -> Option<NodeChildPath> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut steps = Vec::with_capacity(len);
    for _ in 0..len {
        let mut bytes = [0_u8; 4];
        cursor.read_exact(&mut bytes).ok()?;
        steps.push(u32::from_le_bytes(bytes));
    }
    Some(NodeChildPath::from_steps(steps))
}

pub(crate) fn write_type_name_resolution(
    encoded: &mut Vec<u8>,
    value: TypeNameResolution,
    module_paths: &HashMap<ModuleId, String>,
) -> io::Result<()> {
    match value {
        TypeNameResolution::Primitive(spelling) => {
            encoded.push(0);
            write_primitive_spelling(encoded, spelling);
        }
        TypeNameResolution::BuiltinTrait(value) => {
            encoded.push(1);
            encoded.push(builtin_trait_tag(value));
        }
        TypeNameResolution::Def(value) => {
            encoded.push(2);
            write_u64(encoded, value.0);
        }
        TypeNameResolution::External(value) => {
            encoded.push(3);
            write_global_def(encoded, value, module_paths)?;
        }
        TypeNameResolution::GenericParam => encoded.push(4),
        TypeNameResolution::AssociatedType => encoded.push(5),
        TypeNameResolution::Error => encoded.push(6),
    }
    Ok(())
}

pub(crate) fn read_type_name_resolution(
    cursor: &mut Cursor<&[u8]>,
    modules: &HashMap<String, ModuleId>,
) -> Option<TypeNameResolution> {
    Some(match read_u8(cursor)? {
        0 => TypeNameResolution::Primitive(read_primitive_spelling(cursor)?),
        1 => TypeNameResolution::BuiltinTrait(read_builtin_trait(cursor)?),
        2 => TypeNameResolution::Def(DefId(read_u64(cursor)?)),
        3 => TypeNameResolution::External(read_global_def(cursor, modules)?),
        4 => TypeNameResolution::GenericParam,
        5 => TypeNameResolution::AssociatedType,
        6 => TypeNameResolution::Error,
        _ => return None,
    })
}

pub(crate) fn write_global_def(
    encoded: &mut Vec<u8>,
    value: GlobalDefId,
    module_paths: &HashMap<ModuleId, String>,
) -> io::Result<()> {
    let path = module_paths.get(&value.module_id).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "global def module is not loaded",
        )
    })?;
    write_string(encoded, path);
    write_u64(encoded, value.def_id.0);
    Ok(())
}

pub(crate) fn read_global_def(
    cursor: &mut Cursor<&[u8]>,
    modules: &HashMap<String, ModuleId>,
) -> Option<GlobalDefId> {
    let path = read_string(cursor, cursor.get_ref().len())?;
    Some(GlobalDefId {
        module_id: *modules.get(&path)?,
        def_id: DefId(read_u64(cursor)?),
    })
}

pub(crate) fn write_primitive_spelling(encoded: &mut Vec<u8>, spelling: PrimitiveTypeSpelling) {
    match spelling {
        PrimitiveTypeSpelling::Scalar(primitive) => {
            encoded.push(0);
            encoded.push(primitive_tag(primitive));
        }
        PrimitiveTypeSpelling::Vector { elem, lanes } => {
            encoded.push(1);
            encoded.push(primitive_tag(elem));
            encoded.extend_from_slice(&lanes.to_le_bytes());
        }
    }
}

pub(crate) fn read_primitive_spelling(cursor: &mut Cursor<&[u8]>) -> Option<PrimitiveTypeSpelling> {
    match read_u8(cursor)? {
        0 => Some(PrimitiveTypeSpelling::Scalar(read_primitive(cursor)?)),
        1 => {
            let elem = read_primitive(cursor)?;
            let mut bytes = [0_u8; 4];
            cursor.read_exact(&mut bytes).ok()?;
            Some(PrimitiveTypeSpelling::Vector {
                elem,
                lanes: u32::from_le_bytes(bytes),
            })
        }
        _ => None,
    }
}

pub(crate) fn primitive_tag(value: PrimitiveTy) -> u8 {
    match value {
        PrimitiveTy::I8 => 0,
        PrimitiveTy::I16 => 1,
        PrimitiveTy::I32 => 2,
        PrimitiveTy::I64 => 3,
        PrimitiveTy::I128 => 4,
        PrimitiveTy::Isize => 5,
        PrimitiveTy::U8 => 6,
        PrimitiveTy::U16 => 7,
        PrimitiveTy::U32 => 8,
        PrimitiveTy::U64 => 9,
        PrimitiveTy::U128 => 10,
        PrimitiveTy::Usize => 11,
        PrimitiveTy::F32 => 12,
        PrimitiveTy::F64 => 13,
        PrimitiveTy::Bool => 14,
        PrimitiveTy::Char => 15,
        PrimitiveTy::Never => 17,
    }
}

pub(crate) fn read_primitive(cursor: &mut Cursor<&[u8]>) -> Option<PrimitiveTy> {
    Some(match read_u8(cursor)? {
        0 => PrimitiveTy::I8,
        1 => PrimitiveTy::I16,
        2 => PrimitiveTy::I32,
        3 => PrimitiveTy::I64,
        4 => PrimitiveTy::I128,
        5 => PrimitiveTy::Isize,
        6 => PrimitiveTy::U8,
        7 => PrimitiveTy::U16,
        8 => PrimitiveTy::U32,
        9 => PrimitiveTy::U64,
        10 => PrimitiveTy::U128,
        11 => PrimitiveTy::Usize,
        12 => PrimitiveTy::F32,
        13 => PrimitiveTy::F64,
        14 => PrimitiveTy::Bool,
        15 => PrimitiveTy::Char,
        17 => PrimitiveTy::Never,
        _ => return None,
    })
}

pub(crate) fn builtin_trait_tag(value: BuiltinTrait) -> u8 {
    match value {
        BuiltinTrait::Add => 0,
        BuiltinTrait::Sub => 1,
        BuiltinTrait::Mul => 2,
        BuiltinTrait::Div => 3,
        BuiltinTrait::Rem => 4,
        BuiltinTrait::Neg => 5,
        BuiltinTrait::Not => 6,
        BuiltinTrait::BitNot => 7,
        BuiltinTrait::BitAnd => 8,
        BuiltinTrait::BitOr => 9,
        BuiltinTrait::BitXor => 10,
        BuiltinTrait::Shl => 11,
        BuiltinTrait::Shr => 12,
        BuiltinTrait::Eq => 13,
        BuiltinTrait::Ord => 14,
        BuiltinTrait::Sized => 15,
        BuiltinTrait::Unsized => 16,
        BuiltinTrait::Deref => 17,
        BuiltinTrait::DerefMut => 18,
        BuiltinTrait::Index => 19,
        BuiltinTrait::IndexMut => 20,
        BuiltinTrait::Slice => 21,
        BuiltinTrait::SliceMut => 22,
        BuiltinTrait::Iterable => 28,
        BuiltinTrait::Iterator => 29,
        BuiltinTrait::Simd => 30,
        BuiltinTrait::SimdMask => 31,
    }
}

pub(crate) fn read_builtin_trait(cursor: &mut Cursor<&[u8]>) -> Option<BuiltinTrait> {
    Some(match read_u8(cursor)? {
        0 => BuiltinTrait::Add,
        1 => BuiltinTrait::Sub,
        2 => BuiltinTrait::Mul,
        3 => BuiltinTrait::Div,
        4 => BuiltinTrait::Rem,
        5 => BuiltinTrait::Neg,
        6 => BuiltinTrait::Not,
        7 => BuiltinTrait::BitNot,
        8 => BuiltinTrait::BitAnd,
        9 => BuiltinTrait::BitOr,
        10 => BuiltinTrait::BitXor,
        11 => BuiltinTrait::Shl,
        12 => BuiltinTrait::Shr,
        13 => BuiltinTrait::Eq,
        14 => BuiltinTrait::Ord,
        15 => BuiltinTrait::Sized,
        16 => BuiltinTrait::Unsized,
        17 => BuiltinTrait::Deref,
        18 => BuiltinTrait::DerefMut,
        19 => BuiltinTrait::Index,
        20 => BuiltinTrait::IndexMut,
        21 => BuiltinTrait::Slice,
        22 => BuiltinTrait::SliceMut,
        28 => BuiltinTrait::Iterable,
        29 => BuiltinTrait::Iterator,
        30 => BuiltinTrait::Simd,
        31 => BuiltinTrait::SimdMask,
        _ => return None,
    })
}

pub(crate) fn syntax_kind_tag(value: SyntaxKind) -> u8 {
    match value {
        SyntaxKind::Module => 0,
        SyntaxKind::Item => 1,
        SyntaxKind::Stmt => 2,
        SyntaxKind::Expr => 3,
        SyntaxKind::Type => 4,
        SyntaxKind::Pattern => 5,
        SyntaxKind::Param => 6,
        SyntaxKind::Syntax => 7,
        SyntaxKind::Token => 8,
    }
}

pub(crate) fn read_syntax_kind(cursor: &mut Cursor<&[u8]>) -> Option<SyntaxKind> {
    Some(match read_u8(cursor)? {
        0 => SyntaxKind::Module,
        1 => SyntaxKind::Item,
        2 => SyntaxKind::Stmt,
        3 => SyntaxKind::Expr,
        4 => SyntaxKind::Type,
        5 => SyntaxKind::Pattern,
        6 => SyntaxKind::Param,
        7 => SyntaxKind::Syntax,
        8 => SyntaxKind::Token,
        _ => return None,
    })
}
