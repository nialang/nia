// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable symbol and type-name mangling for backend and linker boundaries.

use nia_ids::{ClosureId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_symbol::{SymbolId, stable_hash};
use nia_ty::{
    ArrayLenTy, ConstGenericArg, ConstGenericValue, PrimitiveTy, RangeTyKind, TraitId, TyKind,
    TypeStore,
};

/// Reserved package identity for compiler-generated symbols without a source
/// package owner, such as vtables and source-location metadata.
pub const COMPILER_GENERATED_PACKAGE_IDENTITY: &str = "nia:compiler-generated";

/// Canonical kind of a linker-visible Nia symbol.
///
/// This is deliberately separate from source syntax and from ABI details. The
/// encoded kind identifies the owner of a symbol; calling convention and type
/// layout are supplied by the ABI metadata for the compilation unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MangleSymbolKind {
    Function,
    Global,
    Type,
    Vtable,
    ClosureEntry,
    Adapter,
}

impl MangleSymbolKind {
    fn tag(self) -> u8 {
        match self {
            Self::Function => b'f',
            Self::Global => b'g',
            Self::Type => b't',
            Self::Vtable => b'v',
            Self::ClosureEntry => b'c',
            Self::Adapter => b'a',
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            b'f' => Self::Function,
            b'g' => Self::Global,
            b't' => Self::Type,
            b'v' => Self::Vtable,
            b'c' => Self::ClosureEntry,
            b'a' => Self::Adapter,
            _ => return None,
        })
    }
}

/// Stable, source-independent identity consumed by the canonical encoder.
///
/// `module` must be produced from the package's normalized source identity;
/// session-local `ModuleId` and `DefId` values never appear in the wire form.
/// Generic arguments are already canonical type/const encodings, allowing the
/// encoder to remain independent of the compiler's type-store handles.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StableSymbolKey {
    /// Stable package identity (registry coordinate, source root, or toolchain
    /// package key). Canonical symbols never admit an empty package identity.
    pub package: String,
    pub module: MangleModuleId,
    pub definition: String,
    pub name: String,
    pub kind: MangleSymbolKind,
    pub generic_args: Vec<String>,
}

impl StableSymbolKey {
    pub fn new(
        package: impl Into<String>,
        module: MangleModuleId,
        definition: impl Into<String>,
        name: impl Into<String>,
        kind: MangleSymbolKind,
        generic_args: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            package: package.into(),
            module,
            definition: definition.into(),
            name: name.into(),
            kind,
            generic_args: generic_args.into_iter().collect(),
        }
    }
}

/// Encodes a stable key using Nia's linker namespace.
///
/// The payload is a length-delimited binary record rendered as unpadded
/// base64url. Every field is self-terminating, so names cannot collide through
/// delimiter escaping or sanitization. `_N` is intentionally distinct from
/// both Itanium (`_Z`) and Rust v0 (`_R`) namespaces.
pub fn mangle_stable_symbol(key: &StableSymbolKey) -> String {
    assert!(
        !key.package.is_empty(),
        "Nia ICE: canonical symbol is missing package identity"
    );
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(b"NIA");
    bytes.push(1); // canonical record format, not a public compatibility track
    put_varint(&mut bytes, key.module.raw());
    put_bytes(&mut bytes, key.definition.as_bytes());
    bytes.push(key.kind.tag());
    put_bytes(&mut bytes, key.package.as_bytes());
    put_bytes(&mut bytes, key.name.as_bytes());
    put_varint(&mut bytes, key.generic_args.len() as u64);
    let mut substitutions = Vec::<&str>::new();
    for arg in &key.generic_args {
        if let Some(index) = substitutions.iter().position(|known| *known == arg) {
            bytes.push(0);
            put_varint(&mut bytes, index as u64);
        } else {
            bytes.push(1);
            put_bytes(&mut bytes, arg.as_bytes());
            substitutions.push(arg);
        }
    }
    format!("_N{}", encode_base64url(&bytes))
}

/// Decoded form of [`mangle_stable_symbol`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedStableSymbol {
    pub package: String,
    pub module: MangleModuleId,
    pub definition: String,
    pub name: String,
    pub kind: MangleSymbolKind,
    pub generic_args: Vec<String>,
}

impl DecodedStableSymbol {
    /// Returns a source-oriented diagnostic spelling without exposing the
    /// binary linkage payload. This is intentionally suitable for debug logs,
    /// while the original `_N...` string remains the only linker identity.
    pub fn debug_name(&self) -> String {
        let args = if self.generic_args.is_empty() {
            String::new()
        } else {
            format!("<{}>", self.generic_args.join(", "))
        };
        format!("{}::{}{}", self.package, self.name, args)
    }
}

/// Decodes and validates a canonical Nia symbol.
pub fn demangle_stable_symbol(symbol: &str) -> Option<DecodedStableSymbol> {
    let payload = symbol.strip_prefix("_N")?;
    let bytes = decode_base64url(payload)?;
    if bytes.get(..3)? != b"NIA" || *bytes.get(3)? != 1 {
        return None;
    }
    let mut cursor = 4;
    let module = MangleModuleId(get_varint(&bytes, &mut cursor)?);
    let definition = String::from_utf8(get_bytes(&bytes, &mut cursor)?.to_vec()).ok()?;
    let kind = MangleSymbolKind::from_tag(*bytes.get(cursor)?)?;
    cursor += 1;
    let package = String::from_utf8(get_bytes(&bytes, &mut cursor)?.to_vec()).ok()?;
    if package.is_empty() {
        return None;
    }
    let name = String::from_utf8(get_bytes(&bytes, &mut cursor)?.to_vec()).ok()?;
    let arg_count = usize::try_from(get_varint(&bytes, &mut cursor)?).ok()?;
    let mut generic_args = Vec::with_capacity(arg_count);
    let mut substitutions = Vec::<String>::new();
    for _ in 0..arg_count {
        let tag = *bytes.get(cursor)?;
        cursor += 1;
        let arg = match tag {
            0 => {
                let index = usize::try_from(get_varint(&bytes, &mut cursor)?).ok()?;
                substitutions.get(index)?.clone()
            }
            1 => String::from_utf8(get_bytes(&bytes, &mut cursor)?.to_vec()).ok()?,
            _ => return None,
        };
        substitutions.push(arg.clone());
        generic_args.push(arg);
    }
    (cursor == bytes.len()).then_some(DecodedStableSymbol {
        module,
        definition,
        package,
        name,
        kind,
        generic_args,
    })
}

fn put_varint(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn get_varint(bytes: &[u8], cursor: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*cursor)?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn put_bytes(output: &mut Vec<u8>, bytes: &[u8]) {
    put_varint(output, bytes.len() as u64);
    output.extend_from_slice(bytes);
}

fn get_bytes<'a>(bytes: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    let length = usize::try_from(get_varint(bytes, cursor)?).ok()?;
    let end = (*cursor).checked_add(length)?;
    let value = bytes.get(*cursor..end)?;
    *cursor = end;
    Some(value)
}

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn encode_base64url(bytes: &[u8]) -> String {
    let mut output = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        output.push(BASE64URL[(first >> 2) as usize] as char);
        let second = ((first & 3) << 4) | (chunk.get(1).copied().unwrap_or(0) >> 4);
        output.push(BASE64URL[second as usize] as char);
        if let Some(third) = chunk.get(1) {
            output.push(
                BASE64URL[((third & 0xf) << 2 | (chunk.get(2).copied().unwrap_or(0) >> 6)) as usize]
                    as char,
            );
        }
        if let Some(fourth) = chunk.get(2) {
            output.push(BASE64URL[(fourth & 0x3f) as usize] as char);
        }
    }
    output
}

fn decode_base64url(text: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(text.len() * 3 / 4);
    let mut values = [0u8; 4];
    let mut count = 0;
    for byte in text.bytes() {
        values[count] = BASE64URL.iter().position(|candidate| *candidate == byte)? as u8;
        count += 1;
        if count == 4 {
            output.push(values[0] << 2 | values[1] >> 4);
            output.push(values[1] << 4 | values[2] >> 2);
            output.push(values[2] << 6 | values[3]);
            count = 0;
        }
    }
    if count == 2 {
        output.push(values[0] << 2 | values[1] >> 4);
    } else if count == 3 {
        output.push(values[0] << 2 | values[1] >> 4);
        output.push(values[1] << 4 | values[2] >> 2);
    } else if count != 0 {
        return None;
    }
    (encode_base64url(&output) == text).then_some(output)
}

/// Providers used while encoding module, nominal, and array identities.
pub struct MangleResolvers<F, G, H> {
    module_id: F,
    nominal_name: G,
    array_len: H,
}

impl<F, G, H> MangleResolvers<F, G, H> {
    /// Creates a resolver bundle for one mangling operation.
    pub fn new(module_id: F, nominal_name: G, array_len: H) -> Self {
        Self {
            module_id,
            nominal_name,
            array_len,
        }
    }
}

/// Replaces non-ASCII identifier characters with `_`, preserving a non-empty name.
pub fn sanitize_symbol_part(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// Encodes a stable symbol id without depending on source text.
pub fn mangle_symbol_id(symbol: SymbolId) -> String {
    format!("sym_{:016x}", symbol.raw())
}

/// Returns the canonical textual key for a definition identity.
///
/// `DefId` is derived from `nia-defs::DefIdentity` with collision checking, so
/// its numeric payload is stable metadata identity. The surrounding
/// `GlobalDefId::module_id` remains session-local and is intentionally ignored.
pub fn stable_definition_key(def_id: GlobalDefId) -> String {
    format!("def:{:016x}", def_id.def_id.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Stable module identity derived from a normalized source path.
pub struct MangleModuleId(u64);

impl MangleModuleId {
    /// Hashes a normalized source path into a module identity.
    pub const fn from_normalized_source_path(path: &str) -> Self {
        Self(stable_hash(path))
    }

    /// Returns the raw stable module hash.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Encodes a non-generic definition with the canonical linker grammar.
pub fn mangle_base_symbol_canonical(
    package: impl Into<String>,
    module: MangleModuleId,
    definition: impl Into<String>,
    name: impl Into<String>,
    kind: MangleSymbolKind,
) -> String {
    mangle_stable_symbol(&StableSymbolKey::new(
        package,
        module,
        definition,
        name,
        kind,
        std::iter::empty(),
    ))
}

/// Canonical spelling for a definition when the caller only has a
/// session-qualified id and its stable module identity.
pub fn mangle_definition_symbol_canonical(
    package: impl Into<String>,
    def_id: GlobalDefId,
    module: MangleModuleId,
    name: impl Into<String>,
    kind: MangleSymbolKind,
) -> String {
    mangle_base_symbol_canonical(package, module, stable_definition_key(def_id), name, kind)
}

/// Encodes a generated symbol whose identity is derived from a stable owner
/// and structured arguments rather than a source definition.
pub fn mangle_derived_symbol_canonical(
    package: impl Into<String>,
    module: MangleModuleId,
    definition: impl Into<String>,
    name: impl Into<String>,
    kind: MangleSymbolKind,
    generic_args: impl IntoIterator<Item = String>,
) -> String {
    mangle_stable_symbol(&StableSymbolKey::new(
        package,
        module,
        definition,
        name,
        kind,
        generic_args,
    ))
}

/// Encodes a concrete generic instance with the canonical linker grammar.
///
/// The type and const resolver output is treated as an already canonical
/// argument representation. The binary key still length-delimits each item,
/// so nested delimiters and arbitrary source names remain unambiguous.
pub fn mangle_instance_symbol_canonical<F, G, H>(
    package: impl Into<String>,
    module: MangleModuleId,
    definition: impl Into<String>,
    name: &str,
    args: &[InternedTyId],
    const_args: &[ConstGenericArg],
    type_store: &TypeStore,
    resolvers: MangleResolvers<F, G, H>,
    kind: MangleSymbolKind,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    mangle_instance_symbol_canonical_with_context(
        package, module, definition, name, args, const_args, type_store, resolvers, None, kind,
    )
}

/// Encodes a concrete generic instance and, when needed, its instantiation
/// context. The context is an explicit canonical argument rather than an
/// opaque textual suffix, so it remains part of the reversible identity.
pub fn mangle_instance_symbol_canonical_with_context<F, G, H>(
    package: impl Into<String>,
    module: MangleModuleId,
    definition: impl Into<String>,
    name: &str,
    args: &[InternedTyId],
    const_args: &[ConstGenericArg],
    type_store: &TypeStore,
    resolvers: MangleResolvers<F, G, H>,
    context: Option<MangleModuleId>,
    kind: MangleSymbolKind,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    let MangleResolvers {
        mut module_id,
        mut nominal_name,
        mut array_len,
    } = resolvers;
    let _ = &mut module_id;
    let encoded_args = args
        .iter()
        .map(|arg| {
            mangle_type_with(
                type_store,
                *arg,
                MangleResolvers::new(&mut module_id, &mut nominal_name, &mut array_len),
            )
        })
        .collect::<Vec<_>>();
    let mut all_args = encoded_args;
    all_args.extend(const_args.iter().map(|arg| {
        format!(
            "const:{}",
            mangle_const_generic_arg(
                type_store,
                arg,
                &mut module_id,
                &mut nominal_name,
                &mut array_len,
            )
        )
    }));
    if let Some(context) = context {
        all_args.push(format!("context:{:016x}", context.raw()));
    }
    mangle_stable_symbol(&StableSymbolKey::new(
        package, module, definition, name, kind, all_args,
    ))
}

/// Derives the generated entry symbol for a closure from its concrete owner
/// symbol. Passing the already-instantiated owner symbol keeps entries from
/// distinct generic function instances disjoint without inventing synthetic
/// source definition ids.
pub fn mangle_closure_entry_symbol(owner_symbol: &str, closure_id: ClosureId) -> Option<String> {
    let owner = demangle_stable_symbol(owner_symbol)?;
    Some(mangle_derived_symbol_canonical(
        owner.package,
        owner.module,
        owner.definition,
        owner.name,
        MangleSymbolKind::ClosureEntry,
        owner
            .generic_args
            .into_iter()
            .chain(std::iter::once(format!(
                "closure:ord:{}",
                closure_id.ordinal
            ))),
    ))
}

/// Encodes one canonical type using the supplied nominal and const resolvers.
pub fn mangle_type_with<F, G, H>(
    type_store: &TypeStore,
    ty: InternedTyId,
    resolvers: MangleResolvers<F, G, H>,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    let MangleResolvers {
        mut module_id,
        mut nominal_name,
        mut array_len,
    } = resolvers;
    mangle_type_inner(
        type_store,
        ty,
        &mut module_id,
        &mut nominal_name,
        &mut array_len,
    )
}

fn mangle_type_inner<F, G, H>(
    type_store: &TypeStore,
    ty: InternedTyId,
    module_id: &mut F,
    nominal_name: &mut G,
    array_len: &mut H,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    match type_store.get(ty) {
        Some(TyKind::Opaque) => "opaque".to_string(),
        Some(TyKind::Primitive(primitive)) => mangle_primitive(*primitive),
        Some(TyKind::Tuple(elems)) => {
            let arity = elems.len();
            let encoded_elems = elems
                .iter()
                .map(|elem| {
                    mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>()
                .join("__");
            format!("tuple__len__{arity}__{encoded_elems}")
        }
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let prefix = if *is_readonly { "ptr_read" } else { "ptr" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::VolatilePointer { is_readonly, elem }) => {
            let prefix = if *is_readonly { "vptr_read" } else { "vptr" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let prefix = if *is_readonly { "slice_read" } else { "slice" };
            format!(
                "{prefix}__{}",
                mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::SlicePointee { elem }) => {
            format!(
                "slice_pointee__{}",
                mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::Array { len, elem }) => format!(
            "arr__{}__{}",
            mangle_array_len(len, type_store, module_id, nominal_name, array_len),
            mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
        ),
        Some(TyKind::Vector { elem, lanes }) => {
            format!("vec__len__{lanes}__{}", mangle_primitive(*elem))
        }
        Some(TyKind::Range { kind, bound }) => {
            let kind = match kind {
                RangeTyKind::Exclusive => "range",
                RangeTyKind::Inclusive => "range_incl",
                RangeTyKind::From => "range_from",
                RangeTyKind::To => "range_to",
                RangeTyKind::ToInclusive => "range_to_incl",
                RangeTyKind::Full => "range_full",
            };
            match bound {
                Some(bound) => format!(
                    "{kind}__{}",
                    mangle_type_inner(type_store, *bound, module_id, nominal_name, array_len)
                ),
                None => kind.to_string(),
            }
        }
        Some(TyKind::Optional { elem }) => {
            format!(
                "opt__{}",
                mangle_type_inner(type_store, *elem, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            format!(
                "erru__{}__{}",
                mangle_type_inner(type_store, *error, module_id, nominal_name, array_len),
                mangle_type_inner(type_store, *value, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let param_count = params.len();
            let params = params
                .iter()
                .map(|param| {
                    mangle_type_inner(type_store, *param, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>()
                .join("__");
            let mut result = format!(
                "fnptr__pc{}__{}__ret__{}",
                param_count,
                params,
                mangle_type_inner(type_store, *return_type, module_id, nominal_name, array_len)
            );
            if *is_variadic {
                result.push_str("__variadic");
            }
            result
        }
        Some(TyKind::Callable {
            is_readonly,
            params,
            return_type,
        }) => {
            let param_count = params.len();
            let prefix = if *is_readonly {
                "callable_read"
            } else {
                "callable"
            };
            let params = params
                .iter()
                .map(|param| {
                    mangle_type_inner(type_store, *param, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>()
                .join("__");
            format!(
                "{prefix}__pc{}__{}__ret__{}",
                param_count,
                params,
                mangle_type_inner(type_store, *return_type, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::CallablePointee {
            params,
            return_type,
        }) => {
            let param_count = params.len();
            let params = params
                .iter()
                .map(|param| {
                    mangle_type_inner(type_store, *param, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>()
                .join("__");
            format!(
                "callable_pointee__pc{}__{}__ret__{}",
                param_count,
                params,
                mangle_type_inner(type_store, *return_type, module_id, nominal_name, array_len)
            )
        }
        Some(TyKind::ClosureState { closure_id, .. }) => {
            let owner = mangle_source_def(closure_id.owner, module_id, nominal_name);
            format!("closure__{owner}__ord__{}", closure_id.ordinal)
        }
        Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) => {
            let base = mangle_source_def(*def_id, module_id, nominal_name);
            if args.is_empty() && const_args.is_empty() {
                format!("nom__{base}")
            } else {
                let arg_count = args.len();
                let args = args
                    .iter()
                    .map(|arg| {
                        mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len)
                    })
                    .collect::<Vec<_>>()
                    .join("__");
                let const_arg_parts = const_args
                    .iter()
                    .map(|arg| {
                        mangle_const_generic_arg(
                            type_store,
                            arg,
                            module_id,
                            nominal_name,
                            array_len,
                        )
                    })
                    .collect::<Vec<_>>();
                let const_args = const_arg_parts.join("__");
                format!(
                    "nom__{base}__argc{}__{}__constargc{}__{}",
                    arg_count,
                    args,
                    const_arg_parts.len(),
                    const_args
                )
            }
        }
        Some(TyKind::BuiltinType(builtin)) => {
            format!("builtin_type__{}", sanitize_symbol_part(builtin.name()))
        }
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let base = sanitize_symbol_part(trait_id.name());
            if args.is_empty() {
                format!("builtin_trait__{base}")
            } else {
                let arg_count = args.len();
                let args = args
                    .iter()
                    .map(|arg| {
                        mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len)
                    })
                    .collect::<Vec<_>>()
                    .join("__");
                format!("builtin_trait__{base}__argc{}__{}", arg_count, args)
            }
        }
        Some(TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => {
            let prefix = if *is_readonly {
                "trait_obj_read"
            } else {
                "trait_obj"
            };
            let trait_name = match trait_id {
                TraitId::Source(def_id) => mangle_source_def(*def_id, module_id, nominal_name),
                TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
            };
            let trait_arg_count = trait_args.len();
            let trait_args = trait_args
                .iter()
                .map(|arg| mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len))
                .collect::<Vec<_>>()
                .join("__");
            let trait_const_arg_parts = trait_const_args
                .iter()
                .map(|arg| {
                    mangle_const_generic_arg(type_store, arg, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>();
            let trait_const_args = trait_const_arg_parts.join("__");
            let assoc_bindings = associated_type_bindings
                .iter()
                .map(|binding| {
                    let trait_part = binding
                        .trait_id
                        .map(|trait_id| match trait_id {
                            TraitId::Source(def_id) => {
                                mangle_source_def(def_id, module_id, nominal_name)
                            }
                            TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
                        })
                        .unwrap_or_else(|| "self".to_string());
                    let trait_args = binding
                        .trait_args
                        .iter()
                        .map(|arg| {
                            mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len)
                        })
                        .collect::<Vec<_>>()
                        .join("__");
                    let trait_const_arg_parts = binding
                        .trait_const_args
                        .iter()
                        .map(|arg| {
                            mangle_const_generic_arg(
                                type_store,
                                arg,
                                module_id,
                                nominal_name,
                                array_len,
                            )
                        })
                        .collect::<Vec<_>>();
                    let trait_const_args = trait_const_arg_parts.join("__");
                    format!(
                        "{}__argc{}__{}__cargc{}__{}__{}__{}",
                        trait_part,
                        binding.trait_args.len(),
                        trait_args,
                        trait_const_arg_parts.len(),
                        trait_const_args,
                        mangle_symbol_id(binding.name),
                        mangle_type_inner(
                            type_store,
                            binding.ty,
                            module_id,
                            nominal_name,
                            array_len,
                        )
                    )
                })
                .collect::<Vec<_>>()
                .join("__");
            format!(
                "{prefix}__{}__argc{}__{}__cargc{}__{}__assoc{}__{}",
                trait_name,
                trait_arg_count,
                trait_args,
                trait_const_arg_parts.len(),
                trait_const_args,
                associated_type_bindings.len(),
                assoc_bindings
            )
        }
        Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => {
            let trait_name = match trait_id {
                TraitId::Source(def_id) => mangle_source_def(*def_id, module_id, nominal_name),
                TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
            };
            let trait_arg_count = trait_args.len();
            let trait_args = trait_args
                .iter()
                .map(|arg| mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len))
                .collect::<Vec<_>>()
                .join("__");
            let trait_const_arg_parts = trait_const_args
                .iter()
                .map(|arg| {
                    mangle_const_generic_arg(type_store, arg, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>();
            let trait_const_args = trait_const_arg_parts.join("__");
            let assoc_bindings = associated_type_bindings
                .iter()
                .map(|binding| {
                    let trait_part = binding
                        .trait_id
                        .map(|trait_id| match trait_id {
                            TraitId::Source(def_id) => {
                                mangle_source_def(def_id, module_id, nominal_name)
                            }
                            TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
                        })
                        .unwrap_or_else(|| "self".to_string());
                    let trait_args = binding
                        .trait_args
                        .iter()
                        .map(|arg| {
                            mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len)
                        })
                        .collect::<Vec<_>>()
                        .join("__");
                    let trait_const_arg_parts = binding
                        .trait_const_args
                        .iter()
                        .map(|arg| {
                            mangle_const_generic_arg(
                                type_store,
                                arg,
                                module_id,
                                nominal_name,
                                array_len,
                            )
                        })
                        .collect::<Vec<_>>();
                    let trait_const_args = trait_const_arg_parts.join("__");
                    format!(
                        "{}__argc{}__{}__cargc{}__{}__{}__{}",
                        trait_part,
                        binding.trait_args.len(),
                        trait_args,
                        trait_const_arg_parts.len(),
                        trait_const_args,
                        mangle_symbol_id(binding.name),
                        mangle_type_inner(
                            type_store,
                            binding.ty,
                            module_id,
                            nominal_name,
                            array_len,
                        )
                    )
                })
                .collect::<Vec<_>>()
                .join("__");
            format!(
                "trait_obj_pointee__{}__argc{}__{}__cargc{}__{}__assoc{}__{}",
                trait_name,
                trait_arg_count,
                trait_args,
                trait_const_arg_parts.len(),
                trait_const_args,
                associated_type_bindings.len(),
                assoc_bindings
            )
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        }) => {
            let self_ty =
                mangle_type_inner(type_store, *self_ty, module_id, nominal_name, array_len);
            let trait_name = match trait_id {
                TraitId::Source(def_id) => mangle_source_def(*def_id, module_id, nominal_name),
                TraitId::Builtin(trait_id) => format!("builtin__{}", trait_id.name()),
            };
            let trait_arg_count = trait_args.len();
            let trait_args = trait_args
                .iter()
                .map(|arg| mangle_type_inner(type_store, *arg, module_id, nominal_name, array_len))
                .collect::<Vec<_>>()
                .join("__");
            let trait_const_arg_parts = trait_const_args
                .iter()
                .map(|arg| {
                    mangle_const_generic_arg(type_store, arg, module_id, nominal_name, array_len)
                })
                .collect::<Vec<_>>();
            let trait_const_args = trait_const_arg_parts.join("__");
            format!(
                "proj__{}__as__{}__argc{}__{}__cargc{}__{}__{}",
                self_ty,
                trait_name,
                trait_arg_count,
                trait_args,
                trait_const_arg_parts.len(),
                trait_const_args,
                mangle_symbol_id(*name)
            )
        }
        Some(TyKind::GenericParam(name)) => format!("gen__{}", mangle_symbol_id(*name)),
        Some(TyKind::SelfParam) => "self_param".to_string(),
        Some(TyKind::ConstOnly) => "const_only".to_string(),
        Some(TyKind::Error) => "ty_error".to_string(),
        None => format!(
            "ty_missing__store_{:?}__slot_{}",
            ty.store_id,
            ty.index.index()
        ),
    }
}

fn mangle_source_def<F, G>(def_id: GlobalDefId, module_id: &mut F, nominal_name: &mut G) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
{
    format!(
        "s{:016x}__d{}__{}",
        module_id(def_id.module_id).0,
        def_id.def_id.0,
        nominal_name(def_id)
    )
}

fn mangle_array_len<F, G, H>(
    len: &ArrayLenTy,
    type_store: &TypeStore,
    module_id: &mut F,
    nominal_name: &mut G,
    array_len: &mut H,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    match len {
        ArrayLenTy::Infer => "infer".to_string(),
        ArrayLenTy::GenericParam(name) => format!("gen_len__{}", mangle_symbol_id(*name)),
        ArrayLenTy::ConstValue(value) => format!("len__{value}"),
        ArrayLenTy::ConstExpr(id) => array_len(*id)
            .map(|value| format!("len__{value}"))
            // Keep mangling total so later phases can keep reporting errors.
            // The unresolved marker is stable and cannot collide with a valid
            // evaluated length.
            .unwrap_or_else(|| {
                format!(
                    "len_unresolved__s{:016x}__c{}",
                    module_id(id.module_id).0,
                    id.const_expr_id.0
                )
            }),
        ArrayLenTy::Builtin { builtin, ty } => format!(
            "builtin__{}__{}",
            sanitize_symbol_part(builtin.name()),
            mangle_type_inner(type_store, *ty, module_id, nominal_name, array_len)
        ),
    }
}

fn mangle_const_generic_arg<F, G, H>(
    type_store: &TypeStore,
    arg: &ConstGenericArg,
    module_id: &mut F,
    nominal_name: &mut G,
    array_len: &mut H,
) -> String
where
    F: FnMut(ModuleId) -> MangleModuleId,
    G: FnMut(GlobalDefId) -> String,
    H: FnMut(GlobalConstExprId) -> Option<u64>,
{
    let ty = mangle_type_inner(type_store, arg.ty, module_id, nominal_name, array_len);
    let value = match &arg.value {
        ConstGenericValue::GenericParam(name) => format!("g{}", mangle_symbol_id(*name)),
        ConstGenericValue::ConstExpr(id) => array_len(*id)
            .map(|value| format!("expr_len__{value}"))
            .unwrap_or_else(|| {
                format!(
                    "expr_unresolved__s{:016x}__c{}",
                    module_id(id.module_id).0,
                    id.const_expr_id.0
                )
            }),
        ConstGenericValue::Int(value) => {
            let sign = if value.is_signed() { "i" } else { "u" };
            format!("{sign}{}", value.bits())
        }
        ConstGenericValue::Bool(value) => format!("b{}", u8::from(*value)),
        ConstGenericValue::Char(value) => format!("c{}", *value as u32),
    };
    format!("const__{ty}__{value}")
}

fn mangle_primitive(primitive: PrimitiveTy) -> String {
    match primitive {
        PrimitiveTy::Never => "never".to_string(),
        _ => primitive.name().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{BuiltinTrait, DefId, ModuleIdAllocator, TypeStoreIndex};

    const TEST_PACKAGE: &str = "test/package@0";

    #[test]
    fn canonical_symbol_round_trips_and_is_linker_safe() {
        let key = StableSymbolKey::new(
            "acme/demo@0.2.0",
            MangleModuleId::from_normalized_source_path("pkg/unicode-模块.nia"),
            "def:42",
            "name_with::delimiters/\u{03bb}",
            MangleSymbolKind::Function,
            ["tuple(i32,bool)".to_string(), "const:17".to_string()],
        );
        let symbol = mangle_stable_symbol(&key);
        assert!(symbol.starts_with("_N"));
        assert!(
            symbol
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        );
        let decoded = demangle_stable_symbol(&symbol).expect("canonical symbol must decode");
        assert_eq!(decoded.module, key.module);
        assert_eq!(decoded.definition, key.definition);
        assert_eq!(decoded.package, key.package);
        assert_eq!(decoded.name, key.name);
        assert_eq!(decoded.kind, key.kind);
        assert_eq!(decoded.generic_args, key.generic_args);
        assert!(decoded.debug_name().contains("name_with::delimiters"));
    }

    #[test]
    #[should_panic(expected = "canonical symbol is missing package identity")]
    fn canonical_symbol_rejects_an_empty_package_identity() {
        let key = StableSymbolKey {
            package: String::new(),
            module: MangleModuleId::from_normalized_source_path("main.nia"),
            definition: "def:1".into(),
            name: "main".into(),
            kind: MangleSymbolKind::Function,
            generic_args: Vec::new(),
        };
        let _ = mangle_stable_symbol(&key);
    }

    #[test]
    fn canonical_symbol_framing_prevents_name_and_argument_collisions() {
        let module = MangleModuleId::from_normalized_source_path("main.nia");
        let first = mangle_stable_symbol(&StableSymbolKey::new(
            TEST_PACKAGE,
            module,
            "def:1",
            "ab",
            MangleSymbolKind::Function,
            ["c".to_string()],
        ));
        let second = mangle_stable_symbol(&StableSymbolKey::new(
            TEST_PACKAGE,
            module,
            "def:1",
            "a",
            MangleSymbolKind::Function,
            ["bc".to_string()],
        ));
        assert_ne!(first, second);
        assert!(demangle_stable_symbol(&first).is_some());
        assert!(demangle_stable_symbol(&second).is_some());

        let package_a = mangle_stable_symbol(&StableSymbolKey::new(
            "a",
            module,
            "def:1",
            "same",
            MangleSymbolKind::Function,
            [],
        ));
        let package_b = mangle_stable_symbol(&StableSymbolKey::new(
            "b",
            module,
            "def:1",
            "same",
            MangleSymbolKind::Function,
            [],
        ));
        assert_ne!(package_a, package_b);
        assert_eq!(demangle_stable_symbol(&package_a).unwrap().package, "a");
        assert_eq!(demangle_stable_symbol(&package_b).unwrap().package, "b");

        let repeated = mangle_stable_symbol(&StableSymbolKey::new(
            TEST_PACKAGE,
            module,
            "def:2",
            "repeat",
            MangleSymbolKind::Function,
            (0..4).map(|_| "very-long-type-name".to_string()),
        ));
        let distinct = mangle_stable_symbol(&StableSymbolKey::new(
            TEST_PACKAGE,
            module,
            "def:2",
            "repeat",
            MangleSymbolKind::Function,
            [
                "very-long-type-name".to_string(),
                "very-long-type-name-2".to_string(),
                "very-long-type-name-3".to_string(),
                "very-long-type-name-4".to_string(),
            ],
        ));
        assert!(repeated.len() < distinct.len());
    }

    #[test]
    fn canonical_decoder_rejects_truncated_and_foreign_namespaces() {
        assert!(demangle_stable_symbol("_NAA").is_none());
        assert!(demangle_stable_symbol("_Zabcdef").is_none());
        assert!(demangle_stable_symbol("_N!!!!").is_none());
        assert!(demangle_stable_symbol("_NAB").is_none());
    }

    #[test]
    fn canonical_instance_api_preserves_argument_order() {
        let store = TypeStore::new();
        let module_id = ModuleIdAllocator::new().allocate();
        let append = store.append_for_module(module_id);
        let first = append.primitive(PrimitiveTy::I32);
        let second = append.primitive(PrimitiveTy::Bool);
        let symbol = mangle_instance_symbol_canonical(
            TEST_PACKAGE,
            MangleModuleId::from_normalized_source_path("main.nia"),
            "def:9",
            "run",
            &[first, second],
            &[],
            &store,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("main.nia"),
                |_| "item".into(),
                |_| None,
            ),
            MangleSymbolKind::Function,
        );
        let decoded = demangle_stable_symbol(&symbol).expect("canonical instance must decode");
        assert_eq!(decoded.generic_args.len(), 2);
        assert_ne!(decoded.generic_args[0], decoded.generic_args[1]);
    }

    #[test]
    fn canonical_identity_survives_session_reallocation_and_path_relocation() {
        let first = StableSymbolKey::new(
            "pkg/demo@0.2.0",
            MangleModuleId::from_normalized_source_path("toolchain:/pkg/src/main.nia"),
            "def:stable-function",
            "sym_abc",
            MangleSymbolKind::Function,
            ["i32".to_string()],
        );
        let relocated = StableSymbolKey::new(
            "pkg/demo@0.2.0",
            MangleModuleId::from_normalized_source_path("toolchain:/pkg/src/main.nia"),
            "def:stable-function",
            "sym_abc",
            MangleSymbolKind::Function,
            ["i32".to_string()],
        );
        assert_eq!(
            mangle_stable_symbol(&first),
            mangle_stable_symbol(&relocated)
        );
    }

    #[test]
    fn stable_definition_key_ignores_session_local_module_owner() {
        let mut first = ModuleIdAllocator::new();
        let first_module = first.allocate();
        let mut second = ModuleIdAllocator::new();
        let second_module = second.allocate();
        assert_eq!(
            stable_definition_key(GlobalDefId {
                module_id: first_module,
                def_id: DefId(7),
            }),
            stable_definition_key(GlobalDefId {
                module_id: second_module,
                def_id: DefId(7),
            })
        );
    }

    #[test]
    fn base_mangling_is_stable_across_module_allocator_universes() {
        let mut first_ids = ModuleIdAllocator::new();
        let first_module = first_ids.allocate();
        let mut second_ids = ModuleIdAllocator::new();
        let _unrelated_module = second_ids.allocate();
        let second_module = second_ids.allocate();
        let stable_module = MangleModuleId::from_normalized_source_path("std/error.nia");

        assert_eq!(
            mangle_definition_symbol_canonical(
                TEST_PACKAGE,
                GlobalDefId {
                    module_id: first_module,
                    def_id: DefId(7),
                },
                stable_module,
                "Error",
                MangleSymbolKind::Type,
            ),
            mangle_definition_symbol_canonical(
                TEST_PACKAGE,
                GlobalDefId {
                    module_id: second_module,
                    def_id: DefId(7),
                },
                stable_module,
                "Error",
                MangleSymbolKind::Type,
            )
        );
    }

    #[test]
    fn nominal_mangling_is_stable_across_module_allocator_universes() {
        let type_store = TypeStore::new();
        let mut module_ids = ModuleIdAllocator::new();
        let first_module = module_ids.allocate();
        let _unrelated_module = module_ids.allocate();
        let second_module = module_ids.allocate();
        let first = type_store
            .append_for_module(first_module)
            .intern(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id: first_module,
                    def_id: DefId(7),
                },
                args: Vec::new(),
                const_args: Vec::new(),
            });
        let second = type_store
            .append_for_module(second_module)
            .intern(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id: second_module,
                    def_id: DefId(7),
                },
                args: Vec::new(),
                const_args: Vec::new(),
            });

        let first = mangle_type_with(
            &type_store,
            first,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("std/error.nia"),
                |_| "Error".into(),
                |_| None,
            ),
        );
        let second = mangle_type_with(
            &type_store,
            second,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("std/error.nia"),
                |_| "Error".into(),
                |_| None,
            ),
        );
        assert_eq!(first, second);
    }

    #[test]
    fn nominal_mangling_distinguishes_source_identities() {
        let type_store = TypeStore::new();
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let ty = type_store
            .append_for_module(module_id)
            .intern(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(7),
                },
                args: Vec::new(),
                const_args: Vec::new(),
            });

        let first = mangle_type_with(
            &type_store,
            ty,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("first/error.nia"),
                |_| "Error".into(),
                |_| None,
            ),
        );
        let second = mangle_type_with(
            &type_store,
            ty,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("second/error.nia"),
                |_| "Error".into(),
                |_| None,
            ),
        );
        assert_ne!(first, second);
    }

    #[test]
    fn closure_entry_mangling_uses_concrete_owner_symbol_and_ordinal() {
        use nia_ids::{DefId, ModuleIdAllocator};

        let module_id = ModuleIdAllocator::new().allocate();
        let closure_id = ClosureId {
            owner: GlobalDefId {
                module_id,
                def_id: DefId(7),
            },
            ordinal: 2,
        };
        let module = MangleModuleId::from_normalized_source_path("main.nia");
        let source_owner = mangle_derived_symbol_canonical(
            TEST_PACKAGE,
            module,
            "def:7",
            "owner",
            MangleSymbolKind::Function,
            std::iter::empty(),
        );
        let instance_owner = mangle_derived_symbol_canonical(
            TEST_PACKAGE,
            module,
            "def:7",
            "owner",
            MangleSymbolKind::Function,
            ["i32".to_string()],
        );
        let source = mangle_closure_entry_symbol(&source_owner, closure_id).unwrap();
        let instance = mangle_closure_entry_symbol(&instance_owner, closure_id).unwrap();

        assert_eq!(
            demangle_stable_symbol(&source).unwrap().kind,
            MangleSymbolKind::ClosureEntry
        );
        assert_eq!(
            demangle_stable_symbol(&instance)
                .unwrap()
                .generic_args
                .len(),
            2
        );
        assert_ne!(source, instance);
        assert_ne!(
            source,
            mangle_closure_entry_symbol(
                &source_owner,
                ClosureId {
                    ordinal: 3,
                    ..closure_id
                }
            )
            .unwrap()
        );
        assert!(mangle_closure_entry_symbol("legacy-owner", closure_id).is_none());
    }

    #[test]
    fn mangles_real_error_type_for_diagnostic_recovery() {
        let type_store = TypeStore::new();
        let mut module_ids = ModuleIdAllocator::new();
        let error = type_store.append_for_module(module_ids.allocate()).error();

        assert_eq!(
            mangle_type_with(
                &type_store,
                error,
                MangleResolvers::new(
                    |_| MangleModuleId::from_normalized_source_path("main.nia"),
                    |_| "item".into(),
                    |_| None,
                ),
            ),
            "ty_error"
        );
    }

    #[test]
    fn tuple_mangling_preserves_unit_arity_and_element_order() {
        let type_store = TypeStore::new();
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let append = type_store.append_for_module(module_id);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let bool_ty = append.primitive(PrimitiveTy::Bool);
        let unit = append.intern(TyKind::Tuple(Vec::new()));
        let pair = append.intern(TyKind::Tuple(vec![i32_ty, bool_ty]));
        let reversed = append.intern(TyKind::Tuple(vec![bool_ty, i32_ty]));
        let resolvers = || {
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("main.nia"),
                |_| "item".into(),
                |_| None,
            )
        };

        assert_eq!(
            mangle_type_with(&type_store, unit, resolvers()),
            "tuple__len__0__"
        );
        assert_eq!(
            mangle_type_with(&type_store, pair, resolvers()),
            "tuple__len__2__i32__bool"
        );
        assert_eq!(
            mangle_type_with(&type_store, reversed, resolvers()),
            "tuple__len__2__bool__i32"
        );
    }

    #[test]
    fn generic_mangling_records_argument_counts_not_encoded_text_lengths() {
        let type_store = TypeStore::new();
        let module_id = ModuleIdAllocator::new().allocate();
        let append = type_store.append_for_module(module_id);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let nominal = append.intern(TyKind::Nominal {
            def_id: GlobalDefId {
                module_id,
                def_id: DefId(4),
            },
            args: vec![i32_ty],
            const_args: Vec::new(),
        });
        let builtin_trait = append.intern(TyKind::BuiltinTrait {
            trait_id: BuiltinTrait::Deref,
            args: vec![i32_ty],
        });
        let resolvers = || {
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("main.nia"),
                |_| "Item".into(),
                |_| None,
            )
        };

        let nominal_name = mangle_type_with(&type_store, nominal, resolvers());
        let trait_name = mangle_type_with(&type_store, builtin_trait, resolvers());
        assert!(nominal_name.contains("__argc1__i32__"), "{nominal_name}");
        assert!(trait_name.ends_with("__argc1__i32"), "{trait_name}");
        assert!(!nominal_name.contains("__argc3__"), "{nominal_name}");
        assert!(!trait_name.contains("__argc3__"), "{trait_name}");
    }

    #[test]
    fn function_and_callable_mangling_preserve_arity_mutability_and_signature_order() {
        let type_store = TypeStore::new();
        let module_id = ModuleIdAllocator::new().allocate();
        let append = type_store.append_for_module(module_id);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let bool_ty = append.primitive(PrimitiveTy::Bool);
        let nullary_function = append.intern(TyKind::FunctionPointer {
            params: Vec::new(),
            return_type: i32_ty,
            is_variadic: false,
        });
        let binary_function = append.intern(TyKind::FunctionPointer {
            params: vec![i32_ty, bool_ty],
            return_type: i32_ty,
            is_variadic: false,
        });
        let readonly = append.intern(TyKind::Callable {
            is_readonly: true,
            params: vec![i32_ty, bool_ty],
            return_type: i32_ty,
        });
        let mutable = append.intern(TyKind::Callable {
            is_readonly: false,
            params: vec![i32_ty, bool_ty],
            return_type: i32_ty,
        });
        let pointee = append.intern(TyKind::CallablePointee {
            params: vec![bool_ty, i32_ty],
            return_type: i32_ty,
        });
        let resolvers = || {
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("main.nia"),
                |_| "item".into(),
                |_| None,
            )
        };

        assert_eq!(
            mangle_type_with(&type_store, nullary_function, resolvers()),
            "fnptr__pc0____ret__i32"
        );
        assert_eq!(
            mangle_type_with(&type_store, binary_function, resolvers()),
            "fnptr__pc2__i32__bool__ret__i32"
        );
        assert_eq!(
            mangle_type_with(&type_store, readonly, resolvers()),
            "callable_read__pc2__i32__bool__ret__i32"
        );
        assert_eq!(
            mangle_type_with(&type_store, mutable, resolvers()),
            "callable__pc2__i32__bool__ret__i32"
        );
        assert_eq!(
            mangle_type_with(&type_store, pointee, resolvers()),
            "callable_pointee__pc2__bool__i32__ret__i32"
        );
    }

    #[test]
    fn missing_type_id_uses_a_stable_recovery_symbol() {
        let type_store = TypeStore::new();
        let missing = InternedTyId::new(type_store.id(), TypeStoreIndex::from_store_index(999));

        let first = mangle_type_with(
            &type_store,
            missing,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("main.nia"),
                |_| "item".into(),
                |_| None,
            ),
        );
        let second = mangle_type_with(
            &type_store,
            missing,
            MangleResolvers::new(
                |_| MangleModuleId::from_normalized_source_path("main.nia"),
                |_| "item".into(),
                |_| None,
            ),
        );
        assert_eq!(first, second);
        assert!(first.starts_with("ty_missing__store_"));
        assert!(first.ends_with("__slot_999"));
    }
}
