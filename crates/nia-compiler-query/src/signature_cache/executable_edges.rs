// SPDX-License-Identifier: GPL-3.0-or-later
//! Executable function/global reference-edge cache encoding.

use super::*;

pub(crate) fn encode_executable_value_ref_edges(
    edges: &CachedExecutableValueRefEdges,
    module_paths: &HashMap<ModuleId, String>,
) -> io::Result<Vec<u8>> {
    let mut encoded = Vec::new();
    write_global_def_set(&mut encoded, &edges.functions, module_paths)?;
    write_global_def_set(&mut encoded, &edges.globals, module_paths)?;
    Ok(encoded)
}

pub(crate) fn decode_executable_value_ref_edges(
    encoded: &[u8],
    modules: &HashMap<String, ModuleId>,
) -> Option<CachedExecutableValueRefEdges> {
    let mut cursor = Cursor::new(encoded);
    let functions = read_global_def_set(&mut cursor, modules)?;
    let globals = read_global_def_set(&mut cursor, modules)?;
    (usize::try_from(cursor.position()).ok()? == encoded.len())
        .then_some(CachedExecutableValueRefEdges { functions, globals })
}

pub(crate) fn write_global_def_set(
    encoded: &mut Vec<u8>,
    values: &HashSet<GlobalDefId>,
    module_paths: &HashMap<ModuleId, String>,
) -> io::Result<()> {
    let mut stable_values = values
        .iter()
        .map(|value| {
            let path = module_paths.get(&value.module_id).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "value-ref edge module is not loaded",
                )
            })?;
            Ok((path, value.def_id))
        })
        .collect::<io::Result<Vec<_>>>()?;
    stable_values.sort_unstable();
    write_u64(encoded, stable_values.len() as u64);
    for (path, def_id) in stable_values {
        write_string(encoded, path);
        write_u64(encoded, def_id.0);
    }
    Ok(())
}

pub(crate) fn read_global_def_set(
    cursor: &mut Cursor<&[u8]>,
    modules: &HashMap<String, ModuleId>,
) -> Option<HashSet<GlobalDefId>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut values = HashSet::with_capacity(len);
    let mut previous: Option<(String, DefId)> = None;
    for _ in 0..len {
        let path = read_string(cursor, cursor.get_ref().len())?;
        let def_id = DefId(read_u64(cursor)?);
        if previous
            .as_ref()
            .is_some_and(|previous| previous >= &(path.clone(), def_id))
        {
            return None;
        }
        let value = GlobalDefId {
            module_id: *modules.get(&path)?,
            def_id,
        };
        if !values.insert(value) {
            return None;
        }
        previous = Some((path, def_id));
    }
    Some(values)
}
