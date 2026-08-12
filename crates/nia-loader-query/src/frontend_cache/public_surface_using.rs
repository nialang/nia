// SPDX-License-Identifier: GPL-3.0-or-later
//! Recursive `using` codec and validation for persisted public-surface facts.

use std::{collections::BTreeSet, io, io::Cursor};

use nia_ast::PathSegmentKind;
use nia_defs::{
    ModuleUsing, PublicSurfaceModuleFacts, UsingGroupItem, UsingName, UsingPathSegment,
    UsingSelector,
};
use nia_symbol::SymbolId;

use super::{
    MAX_CACHE_SEQUENCE_LEN, read_len, read_span, read_symbol, read_u8, read_visibility,
    valid_source_span, write_span, write_visibility,
};

// Bound both trusted encoding and untrusted decoding before recursive descent
// so malformed or generated selector trees cannot exhaust the process stack.
const MAX_USING_SELECTOR_DEPTH: usize = 256;

pub(super) fn validate_module_using(
    using: &ModuleUsing,
    source_len: usize,
    depth: usize,
) -> Option<()> {
    (depth <= MAX_USING_SELECTOR_DEPTH).then_some(())?;
    valid_source_span(using.span, source_len).then_some(())?;
    for segment in &using.host {
        valid_source_span(segment.span, source_len).then_some(())?;
    }
    validate_using_selector(&using.selector, source_len, depth)
}

fn validate_using_selector(
    selector: &UsingSelector,
    source_len: usize,
    depth: usize,
) -> Option<()> {
    (depth <= MAX_USING_SELECTOR_DEPTH).then_some(())?;
    match selector {
        UsingSelector::Single(name) => validate_using_name(name, source_len),
        UsingSelector::Group(items) => {
            for item in items {
                match item {
                    UsingGroupItem::Name(name) => validate_using_name(name, source_len)?,
                    UsingGroupItem::Nested { host, selector } => {
                        for segment in host {
                            valid_source_span(segment.span, source_len).then_some(())?;
                        }
                        validate_using_selector(selector, source_len, depth + 1)?;
                    }
                }
            }
            Some(())
        }
        UsingSelector::Wildcard { span } => valid_source_span(*span, source_len).then_some(()),
        UsingSelector::SelfName => Some(()),
    }
}

fn validate_using_name(name: &UsingName, source_len: usize) -> Option<()> {
    valid_source_span(name.name_span, source_len).then_some(())?;
    match (name.alias, name.alias_span) {
        (Some(_), Some(alias_span)) => valid_source_span(alias_span, source_len).then_some(()),
        (None, None) => Some(()),
        _ => None,
    }
}

pub(super) fn write_module_using(
    encoded: &mut Vec<u8>,
    using: &ModuleUsing,
    depth: usize,
) -> io::Result<()> {
    if depth > MAX_USING_SELECTOR_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "public surface using selector is too deeply nested",
        ));
    }
    write_visibility(encoded, using.visibility);
    write_span(encoded, using.span);
    write_using_path_segments(encoded, &using.host);
    write_using_selector(encoded, &using.selector, depth)
}

pub(super) fn read_module_using(
    cursor: &mut Cursor<&[u8]>,
    source_len: usize,
    depth: usize,
) -> Option<ModuleUsing> {
    (depth <= MAX_USING_SELECTOR_DEPTH).then_some(())?;
    Some(ModuleUsing {
        visibility: read_visibility(cursor)?,
        span: read_span(cursor, source_len)?,
        host: read_using_path_segments(cursor, source_len)?,
        selector: read_using_selector(cursor, source_len, depth)?,
    })
}

fn write_using_path_segments(encoded: &mut Vec<u8>, segments: &[UsingPathSegment]) {
    encoded.extend_from_slice(&(segments.len() as u64).to_le_bytes());
    for segment in segments {
        match segment.kind {
            PathSegmentKind::Name(name) => {
                encoded.push(0);
                encoded.extend_from_slice(&name.raw().to_le_bytes());
            }
            PathSegmentKind::Package => encoded.push(1),
            PathSegmentKind::Super => encoded.push(2),
            PathSegmentKind::SelfValue => encoded.push(3),
        }
        write_span(encoded, segment.span);
    }
}

fn read_using_path_segments(
    cursor: &mut Cursor<&[u8]>,
    source_len: usize,
) -> Option<Vec<UsingPathSegment>> {
    let len = read_len(cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut segments = Vec::with_capacity(len);
    for _ in 0..len {
        let kind = match read_u8(cursor)? {
            0 => PathSegmentKind::Name(read_symbol(cursor)?),
            1 => PathSegmentKind::Package,
            2 => PathSegmentKind::Super,
            3 => PathSegmentKind::SelfValue,
            _ => return None,
        };
        segments.push(UsingPathSegment {
            kind,
            span: read_span(cursor, source_len)?,
        });
    }
    Some(segments)
}

fn write_using_selector(
    encoded: &mut Vec<u8>,
    selector: &UsingSelector,
    depth: usize,
) -> io::Result<()> {
    if depth > MAX_USING_SELECTOR_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "public surface using selector is too deeply nested",
        ));
    }
    match selector {
        UsingSelector::Single(name) => {
            encoded.push(0);
            write_using_name(encoded, name);
        }
        UsingSelector::Group(items) => {
            encoded.push(1);
            encoded.extend_from_slice(&(items.len() as u64).to_le_bytes());
            for item in items {
                write_using_group_item(encoded, item, depth + 1)?;
            }
        }
        UsingSelector::Wildcard { span } => {
            encoded.push(2);
            write_span(encoded, *span);
        }
        UsingSelector::SelfName => encoded.push(3),
    }
    Ok(())
}

fn read_using_selector(
    cursor: &mut Cursor<&[u8]>,
    source_len: usize,
    depth: usize,
) -> Option<UsingSelector> {
    (depth <= MAX_USING_SELECTOR_DEPTH).then_some(())?;
    match read_u8(cursor)? {
        0 => Some(UsingSelector::Single(read_using_name(cursor, source_len)?)),
        1 => {
            let len = read_len(cursor, MAX_CACHE_SEQUENCE_LEN)?;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(read_using_group_item(cursor, source_len, depth + 1)?);
            }
            Some(UsingSelector::Group(items))
        }
        2 => Some(UsingSelector::Wildcard {
            span: read_span(cursor, source_len)?,
        }),
        3 => Some(UsingSelector::SelfName),
        _ => None,
    }
}

fn write_using_group_item(
    encoded: &mut Vec<u8>,
    item: &UsingGroupItem,
    depth: usize,
) -> io::Result<()> {
    match item {
        UsingGroupItem::Name(name) => {
            encoded.push(0);
            write_using_name(encoded, name);
        }
        UsingGroupItem::Nested { host, selector } => {
            encoded.push(1);
            write_using_path_segments(encoded, host);
            write_using_selector(encoded, selector, depth)?;
        }
    }
    Ok(())
}

fn read_using_group_item(
    cursor: &mut Cursor<&[u8]>,
    source_len: usize,
    depth: usize,
) -> Option<UsingGroupItem> {
    (depth <= MAX_USING_SELECTOR_DEPTH).then_some(())?;
    match read_u8(cursor)? {
        0 => Some(UsingGroupItem::Name(read_using_name(cursor, source_len)?)),
        1 => Some(UsingGroupItem::Nested {
            host: read_using_path_segments(cursor, source_len)?,
            selector: Box::new(read_using_selector(cursor, source_len, depth)?),
        }),
        _ => None,
    }
}

fn write_using_name(encoded: &mut Vec<u8>, name: &UsingName) {
    encoded.extend_from_slice(&name.name.raw().to_le_bytes());
    write_span(encoded, name.name_span);
    match (name.alias, name.alias_span) {
        (Some(alias), Some(alias_span)) => {
            encoded.push(1);
            encoded.extend_from_slice(&alias.raw().to_le_bytes());
            write_span(encoded, alias_span);
        }
        _ => encoded.push(0),
    }
}

fn read_using_name(cursor: &mut Cursor<&[u8]>, source_len: usize) -> Option<UsingName> {
    let name = read_symbol(cursor)?;
    let name_span = read_span(cursor, source_len)?;
    let (alias, alias_span) = match read_u8(cursor)? {
        0 => (None, None),
        1 => (
            Some(read_symbol(cursor)?),
            Some(read_span(cursor, source_len)?),
        ),
        _ => return None,
    };
    Some(UsingName {
        name,
        name_span,
        alias,
        alias_span,
    })
}

pub(super) fn public_surface_fact_symbols(
    facts: &PublicSurfaceModuleFacts,
) -> io::Result<BTreeSet<SymbolId>> {
    // The persisted dictionary must be the exact symbol closure of the facts;
    // decoding later rejects either missing text or unrelated dictionary data.
    let mut symbols = facts
        .defs
        .iter()
        .map(|def| def.name)
        .collect::<BTreeSet<_>>();
    for entries in [
        &facts.module_scope.modules,
        &facts.module_scope.types,
        &facts.module_scope.values,
    ] {
        symbols.extend(entries.iter().map(|(name, _)| *name));
    }
    for scope in &facts.enum_scopes {
        symbols.extend(scope.variants.iter().map(|(name, _)| *name));
    }
    for using in &facts.module_usings {
        collect_using_path_segment_symbols(&using.host, &mut symbols);
        collect_using_selector_symbols(&using.selector, &mut symbols, 0)?;
    }
    Ok(symbols)
}

fn collect_using_path_segment_symbols(
    segments: &[UsingPathSegment],
    symbols: &mut BTreeSet<SymbolId>,
) {
    symbols.extend(segments.iter().filter_map(|segment| match segment.kind {
        PathSegmentKind::Name(name) => Some(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
    }));
}

fn collect_using_selector_symbols(
    selector: &UsingSelector,
    symbols: &mut BTreeSet<SymbolId>,
    depth: usize,
) -> io::Result<()> {
    if depth > MAX_USING_SELECTOR_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "public surface using selector is too deeply nested",
        ));
    }
    match selector {
        UsingSelector::Single(name) => collect_using_name_symbols(name, symbols)?,
        UsingSelector::Group(items) => {
            for item in items {
                match item {
                    UsingGroupItem::Name(name) => collect_using_name_symbols(name, symbols)?,
                    UsingGroupItem::Nested { host, selector } => {
                        collect_using_path_segment_symbols(host, symbols);
                        collect_using_selector_symbols(selector, symbols, depth + 1)?;
                    }
                }
            }
        }
        UsingSelector::Wildcard { .. } | UsingSelector::SelfName => {}
    }
    Ok(())
}

fn collect_using_name_symbols(
    name: &UsingName,
    symbols: &mut BTreeSet<SymbolId>,
) -> io::Result<()> {
    symbols.insert(name.name);
    match (name.alias, name.alias_span) {
        (Some(alias), Some(_)) => {
            symbols.insert(alias);
        }
        (None, None) => {}
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "public surface using alias and span disagree",
            ));
        }
    }
    Ok(())
}
