// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable codec for module paths shared by frontend cache products.

use std::{collections::BTreeSet, io::Cursor};

use nia_symbol::SymbolId;

use crate::used_paths::{UsedModulePath, UsedModulePathProcessing};

use super::{
    read_bool, read_optional_symbol, read_symbol, read_symbols, read_u8, write_optional_symbol,
    write_symbols,
};

pub(super) fn write_used_module_path(encoded: &mut Vec<u8>, path: &UsedModulePath) {
    let (tag, package, segments, include_declared_children, processing) = match path {
        UsedModulePath::Package {
            package,
            segments,
            include_declared_children,
            processing,
        } => (
            0,
            Some(*package),
            segments,
            *include_declared_children,
            processing,
        ),
        UsedModulePath::PackageRelative {
            segments,
            include_declared_children,
            processing,
        } => (1, None, segments, *include_declared_children, processing),
        UsedModulePath::ParentRelative {
            segments,
            include_declared_children,
            processing,
        } => (2, None, segments, *include_declared_children, processing),
        UsedModulePath::Local {
            segments,
            include_declared_children,
            processing,
        } => (3, None, segments, *include_declared_children, processing),
    };
    encoded.push(tag);
    if let Some(package) = package {
        encoded.extend_from_slice(&package.raw().to_le_bytes());
    }
    write_symbols(encoded, segments);
    encoded.push(u8::from(include_declared_children));
    write_used_module_path_processing(encoded, processing);
}

pub(super) fn read_used_module_path(cursor: &mut Cursor<&[u8]>) -> Option<UsedModulePath> {
    let tag = read_u8(cursor)?;
    (tag <= 3).then_some(())?;
    let package = if tag == 0 {
        Some(read_symbol(cursor)?)
    } else {
        None
    };
    let segments = read_symbols(cursor)?;
    let include_declared_children = read_bool(cursor)?;
    let processing = read_used_module_path_processing(cursor)?;
    match tag {
        0 => Some(UsedModulePath::Package {
            package: package?,
            segments,
            include_declared_children,
            processing,
        }),
        1 => Some(UsedModulePath::PackageRelative {
            segments,
            include_declared_children,
            processing,
        }),
        2 => Some(UsedModulePath::ParentRelative {
            segments,
            include_declared_children,
            processing,
        }),
        3 => Some(UsedModulePath::Local {
            segments,
            include_declared_children,
            processing,
        }),
        _ => None,
    }
}

fn write_used_module_path_processing(encoded: &mut Vec<u8>, processing: &UsedModulePathProcessing) {
    // These discriminants are persisted protocol values. Reordering the enum
    // must not change them without a frontend cache format version bump.
    match processing {
        UsedModulePathProcessing::Never => encoded.push(0),
        UsedModulePathProcessing::Always => encoded.push(1),
        UsedModulePathProcessing::IfSelectedItem => encoded.push(2),
        UsedModulePathProcessing::IfProvidesExtensions => encoded.push(3),
        UsedModulePathProcessing::IfProvidesTraitImpl {
            target_type_name,
            trait_name,
        } => {
            encoded.push(4);
            write_optional_symbol(encoded, *target_type_name);
            encoded.extend_from_slice(&trait_name.raw().to_le_bytes());
        }
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name } => {
            encoded.push(5);
            encoded.extend_from_slice(&trait_name.raw().to_le_bytes());
        }
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name,
            associated_name,
        } => {
            encoded.push(6);
            write_optional_symbol(encoded, *target_type_name);
            encoded.extend_from_slice(&associated_name.raw().to_le_bytes());
        }
    }
}

fn read_used_module_path_processing(
    cursor: &mut Cursor<&[u8]>,
) -> Option<UsedModulePathProcessing> {
    match read_u8(cursor)? {
        0 => Some(UsedModulePathProcessing::Never),
        1 => Some(UsedModulePathProcessing::Always),
        2 => Some(UsedModulePathProcessing::IfSelectedItem),
        3 => Some(UsedModulePathProcessing::IfProvidesExtensions),
        4 => Some(UsedModulePathProcessing::IfProvidesTraitImpl {
            target_type_name: read_optional_symbol(cursor)?,
            trait_name: read_symbol(cursor)?,
        }),
        5 => Some(UsedModulePathProcessing::IfProvidesImplicitTraitImpl {
            trait_name: read_symbol(cursor)?,
        }),
        6 => Some(UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name: read_optional_symbol(cursor)?,
            associated_name: read_symbol(cursor)?,
        }),
        _ => None,
    }
}

pub(super) fn collect_used_module_path_symbols(
    path: &UsedModulePath,
    symbols: &mut BTreeSet<SymbolId>,
) {
    let (package, segments, processing) = match path {
        UsedModulePath::Package {
            package,
            segments,
            processing,
            ..
        } => (Some(*package), segments, processing),
        UsedModulePath::PackageRelative {
            segments,
            processing,
            ..
        }
        | UsedModulePath::ParentRelative {
            segments,
            processing,
            ..
        }
        | UsedModulePath::Local {
            segments,
            processing,
            ..
        } => (None, segments, processing),
    };
    symbols.extend(package);
    symbols.extend(segments.iter().copied());
    match processing {
        UsedModulePathProcessing::Never
        | UsedModulePathProcessing::Always
        | UsedModulePathProcessing::IfSelectedItem
        | UsedModulePathProcessing::IfProvidesExtensions => {}
        UsedModulePathProcessing::IfProvidesTraitImpl {
            target_type_name,
            trait_name,
        } => {
            symbols.extend(*target_type_name);
            symbols.insert(*trait_name);
        }
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name } => {
            symbols.insert(*trait_name);
        }
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name,
            associated_name,
        } => {
            symbols.extend(*target_type_name);
            symbols.insert(*associated_name);
        }
    }
}
