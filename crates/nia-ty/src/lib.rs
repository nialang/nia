// SPDX-License-Identifier: GPL-3.0-or-later
use nia_hash::{FastHashMap, FastHashSet};
pub use nia_ids::{BuiltinTrait, BuiltinType, LayoutBuiltin, TraitId};
use nia_ids::{
    GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TyInternerId, TyInternerIndex,
    TypeOrigin, TypeStoreId,
};
use nia_span::Span;
use nia_symbol::SymbolId;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::{Arc, Mutex, RwLock};

thread_local! {
    static HELD_MODULE_INTERNERS: RefCell<FastHashSet<(TypeStoreId, ModuleId)>> =
        RefCell::new(FastHashSet::default());
}

struct ModuleInternerGuard {
    key: (TypeStoreId, ModuleId),
}

impl ModuleInternerGuard {
    fn acquire(key: (TypeStoreId, ModuleId)) -> Self {
        HELD_MODULE_INTERNERS.with(|held| {
            assert!(
                held.borrow_mut().insert(key),
                "Nia ICE: reentrant access to the same type store module shard"
            );
        });
        Self { key }
    }
}

impl Drop for ModuleInternerGuard {
    fn drop(&mut self) {
        HELD_MODULE_INTERNERS.with(|held| {
            held.borrow_mut().remove(&self.key);
        });
    }
}

#[derive(Debug)]
pub struct TypeStore {
    id: TypeStoreId,
    core: Arc<TypeStoreCore>,
    modules: RwLock<FastHashMap<ModuleId, Arc<Mutex<Option<TyInterner>>>>>,
}

#[derive(Debug)]
struct TypeStoreCore {
    id: TypeStoreId,
    slots: Mutex<TypeStoreSlots>,
}

#[derive(Debug, Default)]
struct TypeStoreSlots {
    canonical: FastHashMap<TyKind, InternedTyId>,
    origins: Vec<TypeOrigin>,
}

impl TypeStoreCore {
    fn intern(&self, origin: TypeOrigin, kind: &TyKind) -> InternedTyId {
        let mut slots = self.slots.lock().expect("type store slots lock poisoned");
        if let Some(ty) = slots.canonical.get(kind) {
            return *ty;
        }
        let index = u32::try_from(slots.origins.len()).expect("type store slot space exhausted");
        let ty = InternedTyId::new(self.id, TyInternerIndex::from_interner_index(index));
        slots.origins.push(origin);
        slots.canonical.insert(kind.clone(), ty);
        ty
    }

    fn type_origin(&self, ty: InternedTyId) -> Option<TypeOrigin> {
        if ty.store_id != self.id {
            return None;
        }
        self.slots
            .lock()
            .expect("type store slots lock poisoned")
            .origins
            .get(ty.index.index() as usize)
            .copied()
    }
}

impl PartialEq for TypeStore {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TypeStore {}

pub struct TypeStoreModuleCheckout {
    interner: Option<TyInterner>,
    slot: Arc<Mutex<Option<TyInterner>>>,
    _guard: ModuleInternerGuard,
    _not_send: PhantomData<Rc<()>>,
}

impl Deref for TypeStoreModuleCheckout {
    type Target = TyInterner;

    fn deref(&self) -> &Self::Target {
        self.interner
            .as_ref()
            .expect("type store checkout returned")
    }
}

impl DerefMut for TypeStoreModuleCheckout {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.interner
            .as_mut()
            .expect("type store checkout returned")
    }
}

impl Drop for TypeStoreModuleCheckout {
    fn drop(&mut self) {
        let interner = self.interner.take().expect("type store checkout returned");
        let mut slot = self.slot.lock().expect("type store module lock poisoned");
        assert!(
            slot.replace(interner).is_none(),
            "Nia ICE: occupied type store slot returned from checkout"
        );
    }
}

impl Default for TypeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeStore {
    pub fn new() -> Self {
        let id = TypeStoreId::fresh();
        Self {
            id,
            core: Arc::new(TypeStoreCore {
                id,
                slots: Mutex::new(TypeStoreSlots::default()),
            }),
            modules: RwLock::new(FastHashMap::default()),
        }
    }

    pub fn id(&self) -> TypeStoreId {
        self.id
    }

    pub fn type_origin(&self, ty: InternedTyId) -> Option<TypeOrigin> {
        self.core.type_origin(ty)
    }

    #[doc(hidden)]
    pub fn with_module_interner_for_semantic_migration<T>(
        &self,
        module_id: ModuleId,
        f: impl FnOnce(&mut TyInterner) -> T,
    ) -> T {
        let _guard = ModuleInternerGuard::acquire((self.id, module_id));
        let interner = self.module_interner(module_id);
        let mut interner = interner.lock().expect("type store module lock poisoned");
        if interner.is_none() {
            drop(interner);
            panic!("Nia ICE: type store module shard is checked out");
        }
        f(interner.as_mut().expect("checked type store module slot"))
    }

    #[doc(hidden)]
    pub fn module_snapshot(&self, module_id: ModuleId) -> TyInterner {
        self.with_module_interner_for_semantic_migration(module_id, |interner| interner.clone())
    }

    #[doc(hidden)]
    pub fn checkout_module_for_semantic_migration(
        &self,
        module_id: ModuleId,
    ) -> TypeStoreModuleCheckout {
        let guard = ModuleInternerGuard::acquire((self.id, module_id));
        let slot = self.module_interner(module_id);
        let interner = slot
            .lock()
            .expect("type store module lock poisoned")
            .take()
            .unwrap_or_else(|| panic!("Nia ICE: type store module shard is already checked out"));
        TypeStoreModuleCheckout {
            interner: Some(interner),
            slot,
            _guard: guard,
            _not_send: PhantomData,
        }
    }

    fn module_interner(&self, module_id: ModuleId) -> Arc<Mutex<Option<TyInterner>>> {
        if let Some(interner) = self
            .modules
            .read()
            .expect("type store lock poisoned")
            .get(&module_id)
        {
            return Arc::clone(interner);
        }
        let mut modules = self.modules.write().expect("type store lock poisoned");
        Arc::clone(modules.entry(module_id).or_insert_with(|| {
            Arc::new(Mutex::new(Some(TyInterner::with_core(
                Arc::clone(&self.core),
                module_id,
            ))))
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TyKind {
    Error,
    ConstOnly,
    Primitive(PrimitiveTy),
    Pointer {
        is_readonly: bool,
        elem: InternedTyId,
    },
    VolatilePointer {
        is_readonly: bool,
        elem: InternedTyId,
    },
    Slice {
        is_readonly: bool,
        elem: InternedTyId,
    },
    SlicePointee {
        elem: InternedTyId,
    },
    Array {
        len: ArrayLenTy,
        elem: InternedTyId,
    },
    Vector {
        elem: PrimitiveTy,
        lanes: u32,
    },
    Range {
        kind: RangeTyKind,
        bound: Option<InternedTyId>,
    },
    FunctionPointer {
        params: Vec<InternedTyId>,
        return_type: InternedTyId,
        is_variadic: bool,
    },
    Optional {
        elem: InternedTyId,
    },
    ErrorUnion {
        error: InternedTyId,
        value: InternedTyId,
    },
    Nominal {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    },
    BuiltinType(BuiltinType),
    BuiltinTrait {
        trait_id: BuiltinTrait,
        args: Vec<InternedTyId>,
    },
    TraitObject {
        is_readonly: bool,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
        associated_type_bindings: Vec<AssociatedTypeBindingTy>,
    },
    TraitObjectPointee {
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
        associated_type_bindings: Vec<AssociatedTypeBindingTy>,
    },
    Projection {
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
        name: SymbolId,
    },
    GenericParam(SymbolId),
    SelfParam,
}

impl TyKind {
    fn visit_referenced_types(&self, mut visit: impl FnMut(InternedTyId)) {
        fn visit_const_args(args: &[ConstGenericArg], visit: &mut impl FnMut(InternedTyId)) {
            for arg in args {
                visit(arg.ty);
            }
        }
        match self {
            Self::Pointer { elem, .. }
            | Self::VolatilePointer { elem, .. }
            | Self::Slice { elem, .. }
            | Self::SlicePointee { elem }
            | Self::Optional { elem } => visit(*elem),
            Self::Array { len, elem } => {
                if let ArrayLenTy::Builtin { ty, .. } = len {
                    visit(*ty);
                }
                visit(*elem);
            }
            Self::Range { bound, .. } => {
                if let Some(bound) = bound {
                    visit(*bound);
                }
            }
            Self::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                for param in params {
                    visit(*param);
                }
                visit(*return_type);
            }
            Self::ErrorUnion { error, value } => {
                visit(*error);
                visit(*value);
            }
            Self::Nominal {
                args, const_args, ..
            } => {
                for arg in args {
                    visit(*arg);
                }
                visit_const_args(const_args, &mut visit);
            }
            Self::BuiltinTrait { args, .. } => {
                for arg in args {
                    visit(*arg);
                }
            }
            Self::TraitObject {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }
            | Self::TraitObjectPointee {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            } => {
                for arg in trait_args {
                    visit(*arg);
                }
                visit_const_args(trait_const_args, &mut visit);
                for binding in associated_type_bindings {
                    for arg in &binding.trait_args {
                        visit(*arg);
                    }
                    visit_const_args(&binding.trait_const_args, &mut visit);
                    visit(binding.ty);
                }
            }
            Self::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            } => {
                visit(*self_ty);
                for arg in trait_args {
                    visit(*arg);
                }
                visit_const_args(trait_const_args, &mut visit);
            }
            Self::Error
            | Self::ConstOnly
            | Self::Primitive(_)
            | Self::Vector { .. }
            | Self::BuiltinType(_)
            | Self::GenericParam(_)
            | Self::SelfParam => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssociatedTypeBindingTy {
    pub trait_id: Option<TraitId>,
    pub trait_args: Vec<InternedTyId>,
    pub trait_const_args: Vec<ConstGenericArg>,
    pub name: SymbolId,
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RangeTyKind {
    Exclusive,
    Inclusive,
    From,
    To,
    ToInclusive,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimitiveTy {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    F32,
    F64,
    Bool,
    Char,
    Void,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IntConst {
    bits: u128,
    signed: bool,
}

impl IntConst {
    pub fn from_i128(value: i128) -> Self {
        Self::signed_bits(value as u128)
    }

    pub fn signed_bits(bits: u128) -> Self {
        Self { bits, signed: true }
    }

    pub fn signed(value: i128) -> Self {
        Self::from_i128(value)
    }

    pub fn unsigned(bits: u128) -> Self {
        Self {
            bits,
            signed: false,
        }
    }

    pub fn bits(self) -> u128 {
        self.bits
    }

    pub fn is_signed(self) -> bool {
        self.signed
    }

    pub fn as_i128(self) -> Option<i128> {
        if self.signed {
            Some(self.bits as i128)
        } else {
            i128::try_from(self.bits).ok()
        }
    }

    pub fn cast_to_primitive_int(self, primitive: PrimitiveTy, pointer_width: u32) -> Option<Self> {
        let bits = primitive.integer_bits(pointer_width)?;
        let mask = integer_mask(bits);
        let bits = self.bits & mask;
        if primitive.is_signed_integer() {
            Some(Self::signed_bits(bits))
        } else {
            Some(Self::unsigned(bits))
        }
    }
}

impl From<i128> for IntConst {
    fn from(value: i128) -> Self {
        Self::from_i128(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstGenericArg {
    pub ty: InternedTyId,
    pub value: ConstGenericValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstGenericValue {
    GenericParam(SymbolId),
    ConstExpr(GlobalConstExprId),
    Int(IntConst),
    Bool(bool),
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveTypeSpelling {
    Scalar(PrimitiveTy),
    Vector { elem: PrimitiveTy, lanes: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArrayLenTy {
    Infer,
    GenericParam(SymbolId),
    ConstValue(u64),
    ConstExpr(GlobalConstExprId),
    Builtin {
        builtin: LayoutBuiltin,
        ty: InternedTyId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstExprSummary {
    pub span: Span,
    pub literal_array_len: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TyInterner {
    core: Arc<TypeStoreCore>,
    interner_id: TyInternerId,
    tys: Vec<(InternedTyId, TyKind)>,
    map: nia_hash::FastHashMap<TyKind, InternedTyId>,
    positions: nia_hash::FastHashMap<InternedTyId, usize>,
    error_ty: InternedTyId,
    primitive_tys: nia_hash::FastHashMap<PrimitiveTy, InternedTyId>,
    builtin_tys: nia_hash::FastHashMap<BuiltinType, InternedTyId>,
}

impl PartialEq for TyInterner {
    fn eq(&self, other: &Self) -> bool {
        self.interner_id == other.interner_id && self.tys == other.tys
    }
}

impl Default for TyInterner {
    fn default() -> Self {
        Self::new(ModuleId(0))
    }
}

impl TyInterner {
    pub fn new(module_id: ModuleId) -> Self {
        let id = TypeStoreId::fresh();
        Self::with_core(
            Arc::new(TypeStoreCore {
                id,
                slots: Mutex::new(TypeStoreSlots::default()),
            }),
            module_id,
        )
    }

    fn with_core(core: Arc<TypeStoreCore>, module_id: ModuleId) -> Self {
        let interner_id = TyInternerId::new(core.id, module_id);
        let placeholder = InternedTyId::new(core.id, TyInternerIndex::from_interner_index(0));
        let mut interner = Self {
            core,
            interner_id,
            tys: Vec::new(),
            map: nia_hash::FastHashMap::default(),
            positions: nia_hash::FastHashMap::default(),
            error_ty: placeholder,
            primitive_tys: nia_hash::FastHashMap::default(),
            builtin_tys: nia_hash::FastHashMap::default(),
        };
        let error_ty = interner.intern_local(TyKind::Error);
        interner.error_ty = error_ty;
        for primitive in PrimitiveTy::ALL {
            let ty = interner.intern_local(TyKind::Primitive(primitive));
            interner.primitive_tys.insert(primitive, ty);
        }
        for builtin in BuiltinType::ALL {
            let ty = interner.intern_local(TyKind::BuiltinType(builtin));
            interner.builtin_tys.insert(builtin, ty);
        }
        interner
    }

    pub fn interner_id(&self) -> TyInternerId {
        self.interner_id
    }

    pub fn type_origin(&self, ty: InternedTyId) -> Option<TypeOrigin> {
        self.core.type_origin(ty)
    }

    pub fn intern(&mut self, kind: TyKind) -> InternedTyId {
        self.intern_local(kind)
    }

    fn intern_local(&mut self, kind: TyKind) -> InternedTyId {
        if let Some(ty) = self.map.get(&kind) {
            return *ty;
        }
        kind.visit_referenced_types(|referenced| {
            assert!(
                self.get(referenced).is_some(),
                "Nia ICE: interned type references a handle outside its active type view"
            );
        });
        let ty = self
            .core
            .intern(TypeOrigin::Module(self.interner_id.module_id()), &kind);
        self.positions.insert(ty, self.tys.len());
        self.tys.push((ty, kind.clone()));
        self.map.insert(kind, ty);
        ty
    }

    pub fn get(&self, id: InternedTyId) -> Option<&TyKind> {
        if id.store_id != self.interner_id.store_id() {
            return None;
        }
        let position = self.positions.get(&id)?;
        self.tys.get(*position).map(|(_, kind)| kind)
    }

    pub fn iter(&self) -> impl Iterator<Item = (InternedTyId, &TyKind)> {
        self.tys.iter().map(|(ty, kind)| (*ty, kind))
    }

    pub fn error(&self) -> InternedTyId {
        self.error_ty
    }

    pub fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        *self
            .primitive_tys
            .get(&primitive)
            .expect("primitive type must be preinterned")
    }

    pub fn builtin_type(&self, builtin: BuiltinType) -> InternedTyId {
        *self
            .builtin_tys
            .get(&builtin)
            .expect("builtin type must be preinterned")
    }

    pub fn len(&self) -> usize {
        self.tys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tys.is_empty()
    }

    pub fn is_prefix_of(&self, other: &Self) -> bool {
        self.interner_id == other.interner_id
            && self.tys.len() <= other.tys.len()
            && self
                .tys
                .iter()
                .zip(other.tys.iter())
                .all(|(left, right)| left == right)
    }

    pub fn contains_error(&self, id: InternedTyId) -> bool {
        let mut seen = FastHashSet::default();
        self.contains_error_inner(id, &mut seen)
    }

    fn contains_error_inner(&self, id: InternedTyId, seen: &mut FastHashSet<InternedTyId>) -> bool {
        if !seen.insert(id) {
            return false;
        }
        match self.get(id) {
            None | Some(TyKind::Error) => true,
            Some(
                TyKind::ConstOnly
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::GenericParam(_)
                | TyKind::SelfParam,
            ) => false,
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
            | Some(TyKind::Optional { elem }) => self.contains_error_inner(*elem, seen),
            Some(TyKind::Array { len, elem }) => {
                self.array_len_contains_error(len, seen) || self.contains_error_inner(*elem, seen)
            }
            Some(TyKind::Vector { .. }) => false,
            Some(TyKind::Range { bound, .. }) => bound
                .map(|bound| self.contains_error_inner(bound, seen))
                .unwrap_or(false),
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                params
                    .iter()
                    .any(|param| self.contains_error_inner(*param, seen))
                    || self.contains_error_inner(*return_type, seen)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.contains_error_inner(*error, seen) || self.contains_error_inner(*value, seen)
            }
            Some(TyKind::Nominal { args, .. }) | Some(TyKind::BuiltinTrait { args, .. }) => {
                args.iter().any(|arg| self.contains_error_inner(*arg, seen))
            }
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .iter()
                    .any(|arg| self.contains_error_inner(*arg, seen))
                    || associated_type_bindings
                        .iter()
                        .any(|binding| self.associated_type_binding_contains_error(binding, seen))
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.contains_error_inner(*self_ty, seen)
                    || trait_args
                        .iter()
                        .any(|arg| self.contains_error_inner(*arg, seen))
            }
        }
    }

    fn array_len_contains_error(
        &self,
        len: &ArrayLenTy,
        seen: &mut FastHashSet<InternedTyId>,
    ) -> bool {
        match len {
            ArrayLenTy::Builtin { ty, .. } => self.contains_error_inner(*ty, seen),
            ArrayLenTy::Infer
            | ArrayLenTy::GenericParam(_)
            | ArrayLenTy::ConstValue(_)
            | ArrayLenTy::ConstExpr(_) => false,
        }
    }

    fn associated_type_binding_contains_error(
        &self,
        binding: &AssociatedTypeBindingTy,
        seen: &mut FastHashSet<InternedTyId>,
    ) -> bool {
        binding
            .trait_args
            .iter()
            .any(|arg| self.contains_error_inner(*arg, seen))
            || self.contains_error_inner(binding.ty, seen)
    }
}

pub fn import_type_into(
    target: &mut TyInterner,
    source: &TyInterner,
    ty: InternedTyId,
) -> InternedTyId {
    try_import_type_into(target, source, ty).unwrap_or_else(|error| panic!("{}", error))
}

pub fn try_import_type_into(
    target: &mut TyInterner,
    source: &TyInterner,
    ty: InternedTyId,
) -> Result<InternedTyId, TypeImportError> {
    match source.get(ty).cloned() {
        Some(TyKind::Error) => Ok(target.error()),
        None => Err(TypeImportError {
            source_interner: source.interner_id(),
            target_interner: target.interner_id(),
            ty,
        }),
        Some(TyKind::ConstOnly) => Ok(target.intern(TyKind::ConstOnly)),
        Some(TyKind::Primitive(primitive)) => Ok(target.primitive(primitive)),
        Some(TyKind::GenericParam(name)) => Ok(target.intern(TyKind::GenericParam(name))),
        Some(TyKind::SelfParam) => Ok(target.intern(TyKind::SelfParam)),
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let elem = try_import_type_into(target, source, elem)?;
            Ok(target.intern(TyKind::Pointer { is_readonly, elem }))
        }
        Some(TyKind::VolatilePointer { is_readonly, elem }) => {
            let elem = try_import_type_into(target, source, elem)?;
            Ok(target.intern(TyKind::VolatilePointer { is_readonly, elem }))
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let elem = try_import_type_into(target, source, elem)?;
            Ok(target.intern(TyKind::Slice { is_readonly, elem }))
        }
        Some(TyKind::SlicePointee { elem }) => {
            let elem = try_import_type_into(target, source, elem)?;
            Ok(target.intern(TyKind::SlicePointee { elem }))
        }
        Some(TyKind::Array { len, elem }) => {
            let len = try_import_array_len_into(target, source, &len)?;
            let elem = try_import_type_into(target, source, elem)?;
            Ok(target.intern(TyKind::Array { len, elem }))
        }
        Some(TyKind::Vector { elem, lanes }) => Ok(target.intern(TyKind::Vector { elem, lanes })),
        Some(TyKind::Range { kind, bound }) => {
            let bound = bound
                .map(|bound| try_import_type_into(target, source, bound))
                .transpose()?;
            Ok(target.intern(TyKind::Range { kind, bound }))
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let params = params
                .into_iter()
                .map(|param| try_import_type_into(target, source, param))
                .collect::<Result<_, _>>()?;
            let return_type = try_import_type_into(target, source, return_type)?;
            Ok(target.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }))
        }
        Some(TyKind::Optional { elem }) => {
            let elem = try_import_type_into(target, source, elem)?;
            Ok(target.intern(TyKind::Optional { elem }))
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            let error = try_import_type_into(target, source, error)?;
            let value = try_import_type_into(target, source, value)?;
            Ok(target.intern(TyKind::ErrorUnion { error, value }))
        }
        Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) => {
            let args = args
                .into_iter()
                .map(|arg| try_import_type_into(target, source, arg))
                .collect::<Result<_, _>>()?;
            let const_args = const_args
                .into_iter()
                .map(|arg| try_import_const_generic_arg_into(target, source, &arg))
                .collect::<Result<_, _>>()?;
            Ok(target.intern(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }))
        }
        Some(TyKind::BuiltinType(builtin)) => Ok(target.intern(TyKind::BuiltinType(builtin))),
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let args = args
                .into_iter()
                .map(|arg| try_import_type_into(target, source, arg))
                .collect::<Result<_, _>>()?;
            Ok(target.intern(TyKind::BuiltinTrait { trait_id, args }))
        }
        Some(TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| try_import_type_into(target, source, arg))
                .collect::<Result<_, _>>()?;
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|arg| try_import_const_generic_arg_into(target, source, &arg))
                .collect::<Result<_, _>>()?;
            let associated_type_bindings = associated_type_bindings
                .into_iter()
                .map(|binding| {
                    Ok(AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| try_import_type_into(target, source, arg))
                            .collect::<Result<_, _>>()?,
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|arg| try_import_const_generic_arg_into(target, source, &arg))
                            .collect::<Result<_, _>>()?,
                        name: binding.name,
                        ty: try_import_type_into(target, source, binding.ty)?,
                    })
                })
                .collect::<Result<_, TypeImportError>>()?;
            Ok(target.intern(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }))
        }
        Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| try_import_type_into(target, source, arg))
                .collect::<Result<_, _>>()?;
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|arg| try_import_const_generic_arg_into(target, source, &arg))
                .collect::<Result<_, _>>()?;
            let associated_type_bindings = associated_type_bindings
                .into_iter()
                .map(|binding| {
                    Ok(AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| try_import_type_into(target, source, arg))
                            .collect::<Result<_, _>>()?,
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|arg| try_import_const_generic_arg_into(target, source, &arg))
                            .collect::<Result<_, _>>()?,
                        name: binding.name,
                        ty: try_import_type_into(target, source, binding.ty)?,
                    })
                })
                .collect::<Result<_, TypeImportError>>()?;
            Ok(target.intern(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }))
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        }) => {
            let self_ty = try_import_type_into(target, source, self_ty)?;
            let trait_args = trait_args
                .into_iter()
                .map(|arg| try_import_type_into(target, source, arg))
                .collect::<Result<_, _>>()?;
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|arg| try_import_const_generic_arg_into(target, source, &arg))
                .collect::<Result<_, _>>()?;
            Ok(target.intern(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeImportError {
    pub source_interner: TyInternerId,
    pub target_interner: TyInternerId,
    pub ty: InternedTyId,
}

impl std::fmt::Display for TypeImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "type import failed: {:?} is not in source interner {:?} while importing into {:?}",
            self.ty, self.source_interner, self.target_interner
        )
    }
}

impl std::error::Error for TypeImportError {}

fn try_import_array_len_into(
    target: &mut TyInterner,
    source: &TyInterner,
    len: &ArrayLenTy,
) -> Result<ArrayLenTy, TypeImportError> {
    match len {
        ArrayLenTy::Builtin { builtin, ty } => Ok(ArrayLenTy::Builtin {
            builtin: *builtin,
            // Layout-builtin lengths carry a type operand; after cross-module copying it must
            // point at the target interner just like ordinary array element types do.
            ty: try_import_type_into(target, source, *ty)?,
        }),
        ArrayLenTy::Infer
        | ArrayLenTy::GenericParam(_)
        | ArrayLenTy::ConstValue(_)
        | ArrayLenTy::ConstExpr(_) => Ok(len.clone()),
    }
}

fn try_import_const_generic_arg_into(
    target: &mut TyInterner,
    source: &TyInterner,
    arg: &ConstGenericArg,
) -> Result<ConstGenericArg, TypeImportError> {
    Ok(ConstGenericArg {
        ty: try_import_type_into(target, source, arg.ty)?,
        value: arg.value.clone(),
    })
}

pub trait TypeEquivalence {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind>;
    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool;
    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool;

    fn same_type_args_for_equiv(&self, left: &[InternedTyId], right: &[InternedTyId]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| self.same_type_for_equiv(*left, *right))
    }

    fn same_const_generic_args_for_equiv(
        &self,
        left: &[ConstGenericArg],
        right: &[ConstGenericArg],
    ) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left, right)| {
                self.same_type_for_equiv(left.ty, right.ty) && left.value == right.value
            })
    }

    fn compute_same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        match (self.ty_kind_for_equiv(left), self.ty_kind_for_equiv(right)) {
            (Some(TyKind::Error), Some(TyKind::Error)) => true,
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            (Some(TyKind::SelfParam), Some(TyKind::SelfParam)) => true,
            (Some(TyKind::BuiltinType(left)), Some(TyKind::BuiltinType(right))) => left == right,
            (
                Some(TyKind::Pointer {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::VolatilePointer {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::VolatilePointer {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            ) => left_const == right_const && self.same_type_for_equiv(*left_elem, *right_elem),
            (
                Some(TyKind::SlicePointee { elem: left_elem }),
                Some(TyKind::SlicePointee { elem: right_elem }),
            ) => self.same_type_for_equiv(*left_elem, *right_elem),
            (
                Some(TyKind::Array {
                    len: left_len,
                    elem: left_elem,
                }),
                Some(TyKind::Array {
                    len: right_len,
                    elem: right_elem,
                }),
            ) => {
                self.same_array_len_for_equiv(left_len, right_len)
                    && self.same_type_for_equiv(*left_elem, *right_elem)
            }
            (
                Some(TyKind::Range {
                    kind: left_kind,
                    bound: left_bound,
                }),
                Some(TyKind::Range {
                    kind: right_kind,
                    bound: right_bound,
                }),
            ) => {
                left_kind == right_kind
                    && match (left_bound, right_bound) {
                        (Some(left), Some(right)) => self.same_type_for_equiv(*left, *right),
                        (None, None) => true,
                        _ => false,
                    }
            }
            (
                Some(TyKind::FunctionPointer {
                    params: left_params,
                    return_type: left_return,
                    is_variadic: left_variadic,
                }),
                Some(TyKind::FunctionPointer {
                    params: right_params,
                    return_type: right_return,
                    is_variadic: right_variadic,
                }),
            ) => {
                left_variadic == right_variadic
                    && self.same_type_args_for_equiv(left_params, right_params)
                    && self.same_type_for_equiv(*left_return, *right_return)
            }
            (
                Some(TyKind::Nominal {
                    def_id: left_def,
                    args: left_args,
                    const_args: left_const_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                    const_args: right_const_args,
                }),
            ) => {
                left_def == right_def
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_const_generic_args_for_equiv(left_const_args, right_const_args)
            }
            (
                Some(TyKind::BuiltinTrait {
                    trait_id: left_trait,
                    args: left_args,
                }),
                Some(TyKind::BuiltinTrait {
                    trait_id: right_trait,
                    args: right_args,
                }),
            ) => left_trait == right_trait && self.same_type_args_for_equiv(left_args, right_args),
            (Some(TyKind::Optional { elem: left }), Some(TyKind::Optional { elem: right })) => {
                self.same_type_for_equiv(*left, *right)
            }
            (
                Some(TyKind::ErrorUnion {
                    error: left_error,
                    value: left_value,
                }),
                Some(TyKind::ErrorUnion {
                    error: right_error,
                    value: right_value,
                }),
            ) => {
                self.same_type_for_equiv(*left_error, *right_error)
                    && self.same_type_for_equiv(*left_value, *right_value)
            }
            (
                Some(TyKind::TraitObject {
                    is_readonly: left_const,
                    trait_id: left_trait,
                    trait_args: left_args,
                    trait_const_args: left_const_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObject {
                    is_readonly: right_const,
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_const == right_const
                    && left_trait == right_trait
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_const_generic_args_for_equiv(left_const_args, right_const_args)
                    && self.same_associated_type_bindings_for_equiv(left_bindings, right_bindings)
            }
            (
                Some(TyKind::TraitObjectPointee {
                    trait_id: left_trait,
                    trait_args: left_args,
                    trait_const_args: left_const_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObjectPointee {
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_trait == right_trait
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_const_generic_args_for_equiv(left_const_args, right_const_args)
                    && self.same_associated_type_bindings_for_equiv(left_bindings, right_bindings)
            }
            (
                Some(TyKind::Projection {
                    self_ty: left_self,
                    trait_id: left_trait,
                    trait_args: left_args,
                    trait_const_args: left_const_args,
                    name: left_name,
                }),
                Some(TyKind::Projection {
                    self_ty: right_self,
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    name: right_name,
                }),
            ) => {
                left_trait == right_trait
                    && left_name == right_name
                    && self.same_type_for_equiv(*left_self, *right_self)
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_const_generic_args_for_equiv(left_const_args, right_const_args)
            }
            _ => false,
        }
    }

    fn same_associated_type_bindings_for_equiv(
        &self,
        left: &[AssociatedTypeBindingTy],
        right: &[AssociatedTypeBindingTy],
    ) -> bool {
        left.len() == right.len()
            && left.iter().all(|left_binding| {
                right
                    .iter()
                    .find(|right_binding| {
                        self.same_associated_type_binding_key_for_equiv(left_binding, right_binding)
                    })
                    .is_some_and(|right_binding| {
                        self.same_type_for_equiv(left_binding.ty, right_binding.ty)
                    })
            })
    }

    fn same_associated_type_binding_key_for_equiv(
        &self,
        left: &AssociatedTypeBindingTy,
        right: &AssociatedTypeBindingTy,
    ) -> bool {
        left.name == right.name
            && left.trait_id == right.trait_id
            && self.same_type_args_for_equiv(&left.trait_args, &right.trait_args)
            && self
                .same_const_generic_args_for_equiv(&left.trait_const_args, &right.trait_const_args)
    }
}

impl PrimitiveTy {
    pub const ALL: [Self; 18] = [
        Self::I8,
        Self::I16,
        Self::I32,
        Self::I64,
        Self::I128,
        Self::Isize,
        Self::U8,
        Self::U16,
        Self::U32,
        Self::U64,
        Self::U128,
        Self::Usize,
        Self::F32,
        Self::F64,
        Self::Bool,
        Self::Char,
        Self::Void,
        Self::Never,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "i8" => Self::I8,
            "i16" => Self::I16,
            "i32" => Self::I32,
            "i64" => Self::I64,
            "i128" => Self::I128,
            "isize" => Self::Isize,
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "u128" => Self::U128,
            "usize" => Self::Usize,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "bool" => Self::Bool,
            "char" => Self::Char,
            "void" => Self::Void,
            "!" => Self::Never,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::I128 => "i128",
            Self::Isize => "isize",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::U128 => "u128",
            Self::Usize => "usize",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Bool => "bool",
            Self::Char => "char",
            Self::Void => "void",
            Self::Never => "!",
        }
    }

    pub fn vector_element_from_name(name: &str) -> Option<Self> {
        match Self::from_name(name)? {
            primitive if primitive.is_vector_element() => Some(primitive),
            _ => None,
        }
    }

    pub fn is_vector_element(self) -> bool {
        !matches!(self, Self::Char | Self::Void | Self::Never)
    }

    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Self::I8
                | Self::I16
                | Self::I32
                | Self::I64
                | Self::I128
                | Self::Isize
                | Self::U8
                | Self::U16
                | Self::U32
                | Self::U64
                | Self::U128
                | Self::Usize
        )
    }

    pub fn is_signed_integer(self) -> bool {
        matches!(
            self,
            Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::I128 | Self::Isize
        )
    }

    pub fn integer_bits(self, pointer_width: u32) -> Option<u32> {
        match self {
            Self::I8 | Self::U8 => Some(8),
            Self::I16 | Self::U16 => Some(16),
            Self::I32 | Self::U32 => Some(32),
            Self::I64 | Self::U64 => Some(64),
            Self::I128 | Self::U128 => Some(128),
            Self::Isize | Self::Usize => Some(pointer_width),
            _ => None,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }
}

fn integer_mask(bits: u32) -> u128 {
    if bits >= u128::BITS {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

impl PrimitiveTypeSpelling {
    pub fn from_name(name: &str) -> Option<Self> {
        if let Some(primitive) = PrimitiveTy::from_name(name) {
            return Some(Self::Scalar(primitive));
        }
        vector_type_spelling(name)
    }
}

fn vector_type_spelling(name: &str) -> Option<PrimitiveTypeSpelling> {
    let (elem, lane_text) = if let Some(rest) = name.strip_prefix("boolx") {
        (PrimitiveTy::Bool, rest)
    } else {
        let split = name.rfind('x')?;
        let elem_text = &name[..split];
        let lane_text = &name[(split + 1)..];
        let elem = PrimitiveTy::vector_element_from_name(elem_text)?;
        if elem == PrimitiveTy::Bool {
            return None;
        }
        (elem, lane_text)
    };
    if lane_text.is_empty() || !lane_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let lanes = lane_text.parse::<u32>().ok()?;
    if lanes == 0 {
        return None;
    }
    Some(PrimitiveTypeSpelling::Vector { elem, lanes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interns_identical_types_once() {
        let mut interner = TyInterner::new(ModuleId(0));
        let initial_len = interner.len();
        let a = interner.intern(TyKind::Primitive(PrimitiveTy::I32));
        let b = interner.intern(TyKind::Primitive(PrimitiveTy::I32));
        assert_eq!(a, b);
        assert_eq!(interner.len(), initial_len);
    }

    #[test]
    fn primitive_ids_match_preinterned_layout() {
        let interner = TyInterner::new(ModuleId(0));
        for primitive in PrimitiveTy::ALL {
            let id = interner.primitive(primitive);
            assert_eq!(interner.get(id), Some(&TyKind::Primitive(primitive)));
        }
    }

    #[test]
    fn type_store_identity_rejects_foreign_session_handles() {
        let first = TypeStore::new();
        let second = TypeStore::new();
        let first_interner = first.module_snapshot(ModuleId(7));
        let second_interner = second.module_snapshot(ModuleId(7));
        let first_i32 = first_interner.primitive(PrimitiveTy::I32);
        let second_i32 = second_interner.primitive(PrimitiveTy::I32);

        assert_ne!(first.id(), second.id());
        assert_ne!(first_interner.interner_id(), second_interner.interner_id());
        assert_ne!(first_i32, second_i32);
        assert_eq!(second_interner.get(first_i32), None);
        assert_eq!(first_interner.get(second_i32), None);
        assert_eq!(
            first.type_origin(first_i32),
            Some(TypeOrigin::Module(ModuleId(7)))
        );
        assert_eq!(first.type_origin(second_i32), None);
        assert_eq!(
            first_interner.type_origin(first_i32),
            Some(TypeOrigin::Module(ModuleId(7)))
        );
        assert_eq!(first_interner.type_origin(second_i32), None);
    }

    #[test]
    fn type_views_share_canonical_ids_and_resolve_physical_origins() {
        let store = TypeStore::new();
        let local = store.module_snapshot(ModuleId(2));
        let foreign = store.module_snapshot(ModuleId(9));
        let foreign_ty = foreign.primitive(PrimitiveTy::U32);

        assert_eq!(foreign_ty, local.primitive(PrimitiveTy::U32));
        assert_eq!(
            local.type_origin(foreign_ty),
            Some(TypeOrigin::Module(ModuleId(2)))
        );
    }

    #[test]
    fn module_views_intern_structural_types_to_one_session_id() {
        let store = TypeStore::new();
        let first_pointer =
            store.with_module_interner_for_semantic_migration(ModuleId(2), |interner| {
                let elem = interner.primitive(PrimitiveTy::U32);
                interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem,
                })
            });
        let second_pointer =
            store.with_module_interner_for_semantic_migration(ModuleId(9), |interner| {
                let elem = interner.primitive(PrimitiveTy::U32);
                interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem,
                })
            });

        assert_eq!(first_pointer, second_pointer);
        assert_eq!(
            store.type_origin(first_pointer),
            Some(TypeOrigin::Module(ModuleId(2)))
        );
    }

    #[test]
    #[should_panic(expected = "outside its active type view")]
    fn interning_rejects_foreign_session_type_dependencies() {
        let mut local = TyInterner::new(ModuleId(2));
        let foreign = TyInterner::new(ModuleId(9));

        local.intern(TyKind::Pointer {
            is_readonly: true,
            elem: foreign.primitive(PrimitiveTy::U32),
        });
    }

    #[test]
    fn same_session_import_preserves_id_and_adopts_the_type_view() {
        let store = TypeStore::new();
        let pointer = store.with_module_interner_for_semantic_migration(ModuleId(3), |interner| {
            let elem = interner.primitive(PrimitiveTy::U16);
            interner.intern(TyKind::Pointer {
                is_readonly: false,
                elem,
            })
        });
        let source = store.module_snapshot(ModuleId(3));
        let imported = store.with_module_interner_for_semantic_migration(ModuleId(8), |target| {
            assert!(target.get(pointer).is_none());
            let imported = import_type_into(target, &source, pointer);
            assert!(target.get(pointer).is_some());
            imported
        });

        assert_eq!(imported, pointer);
    }

    #[test]
    fn session_type_handle_is_word_sized() {
        assert_eq!(
            std::mem::size_of::<InternedTyId>(),
            std::mem::size_of::<u64>()
        );
    }

    #[test]
    fn type_store_module_slots_are_append_only_across_transactions() {
        let store = TypeStore::new();
        let module_id = ModuleId(3);
        let pointer = store.with_module_interner_for_semantic_migration(module_id, |interner| {
            let elem = interner.primitive(PrimitiveTy::U8);
            interner.intern(TyKind::Pointer {
                is_readonly: true,
                elem,
            })
        });
        let before = store.module_snapshot(module_id);
        let slice = store.with_module_interner_for_semantic_migration(module_id, |interner| {
            let elem = interner.primitive(PrimitiveTy::U8);
            interner.intern(TyKind::Slice {
                is_readonly: true,
                elem,
            })
        });
        let after = store.module_snapshot(module_id);

        assert!(before.is_prefix_of(&after));
        assert_eq!(before.get(pointer), after.get(pointer));
        assert_eq!(before.get(slice), None);
        assert!(matches!(after.get(slice), Some(TyKind::Slice { .. })));
    }

    #[test]
    fn type_store_checkout_returns_appended_slots_to_the_session() {
        let store = TypeStore::new();
        let module_id = ModuleId(4);
        let before = store.module_snapshot(module_id);
        let pointer = {
            let mut checkout = store.checkout_module_for_semantic_migration(module_id);
            assert_eq!(checkout.interner_id(), before.interner_id());
            let elem = checkout.primitive(PrimitiveTy::U16);
            checkout.intern(TyKind::Pointer {
                is_readonly: false,
                elem,
            })
        };
        let after = store.module_snapshot(module_id);

        assert!(before.is_prefix_of(&after));
        assert!(matches!(after.get(pointer), Some(TyKind::Pointer { .. })));
    }

    #[test]
    fn type_store_checkout_rejects_reentry_and_restores_the_shard() {
        let store = TypeStore::new();
        let module_id = ModuleId(5);
        let checkout = store.checkout_module_for_semantic_migration(module_id);
        let reentry = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.module_snapshot(module_id)
        }));

        assert!(reentry.is_err());
        drop(checkout);
        assert_eq!(
            store.module_snapshot(module_id).interner_id(),
            TyInternerId::new(store.id(), module_id)
        );
    }

    #[test]
    fn primitive_type_spelling_resolves_scalar_and_vector_names() {
        assert_eq!(
            PrimitiveTypeSpelling::from_name("i32"),
            Some(PrimitiveTypeSpelling::Scalar(PrimitiveTy::I32))
        );
        assert_eq!(
            PrimitiveTypeSpelling::from_name("u8x16"),
            Some(PrimitiveTypeSpelling::Vector {
                elem: PrimitiveTy::U8,
                lanes: 16,
            })
        );
        assert_eq!(
            PrimitiveTypeSpelling::from_name("boolx4"),
            Some(PrimitiveTypeSpelling::Vector {
                elem: PrimitiveTy::Bool,
                lanes: 4,
            })
        );

        assert_eq!(PrimitiveTypeSpelling::from_name("boolx"), None);
        assert_eq!(PrimitiveTypeSpelling::from_name("boolx0"), None);
        assert_eq!(PrimitiveTypeSpelling::from_name("charx4"), None);
        assert_eq!(PrimitiveTypeSpelling::from_name("voidx4"), None);
        assert_eq!(PrimitiveTypeSpelling::from_name("!x4"), None);
    }

    #[test]
    fn contains_error_detects_nested_error_types() {
        let mut interner = TyInterner::new(ModuleId(0));
        let err = interner.error();
        let ptr = interner.intern(TyKind::Pointer {
            is_readonly: false,
            elem: err,
        });
        let value = interner.primitive(PrimitiveTy::I32);
        let union = interner.intern(TyKind::ErrorUnion { error: ptr, value });

        assert!(interner.contains_error(union));
        assert!(!interner.contains_error(value));
    }

    #[test]
    fn interner_snapshots_accept_only_prefix_growth() {
        let mut base = TyInterner::new(ModuleId(0));
        let snapshot = base.clone();
        base.intern(TyKind::GenericParam(nia_symbol::known::ITEM));
        let mut diverged = snapshot.clone();
        diverged.intern(TyKind::GenericParam(nia_symbol::known::OUTPUT));

        assert!(snapshot.is_prefix_of(&base));
        assert!(!base.is_prefix_of(&snapshot));
        assert!(!base.is_prefix_of(&diverged));
        assert!(!diverged.is_prefix_of(&base));
    }

    #[test]
    #[should_panic(expected = "reentrant access to the same type store module shard")]
    fn type_store_rejects_same_thread_module_reentry() {
        let store = TypeStore::new();
        store.with_module_interner_for_semantic_migration(ModuleId(0), |_| {
            store.with_module_interner_for_semantic_migration(ModuleId(0), |_| {});
        });
    }

    #[test]
    fn try_import_type_reports_source_interner_mismatch() {
        let source = TyInterner::new(ModuleId(0));
        let mut target = TyInterner::new(ModuleId(1));
        let other = TyInterner::new(ModuleId(2));
        let ty = other.primitive(PrimitiveTy::I32);

        let err = try_import_type_into(&mut target, &source, ty).unwrap_err();

        assert_eq!(err.source_interner, source.interner_id());
        assert_eq!(err.target_interner, target.interner_id());
        assert_eq!(err.ty, ty);
    }

    #[test]
    #[should_panic(expected = "type import failed")]
    fn import_type_panics_on_source_interner_mismatch() {
        let source = TyInterner::new(ModuleId(0));
        let mut target = TyInterner::new(ModuleId(1));
        let other = TyInterner::new(ModuleId(2));

        import_type_into(&mut target, &source, other.primitive(PrimitiveTy::I32));
    }

    #[test]
    fn import_type_reinterns_layout_builtin_array_length_operand() {
        let mut source = TyInterner::new(ModuleId(0));
        let mut target = TyInterner::new(ModuleId(1));
        let source_i32 = source.primitive(PrimitiveTy::I32);
        let source_array = source.intern(TyKind::Array {
            len: ArrayLenTy::Builtin {
                builtin: LayoutBuiltin::Size,
                ty: source_i32,
            },
            elem: source_i32,
        });

        let imported = import_type_into(&mut target, &source, source_array);

        let Some(TyKind::Array {
            len:
                ArrayLenTy::Builtin {
                    ty: imported_len_ty,
                    ..
                },
            elem,
        }) = target.get(imported)
        else {
            panic!("expected imported array type");
        };
        assert_eq!(imported_len_ty.store_id, target.interner_id().store_id());
        assert_eq!(*imported_len_ty, target.primitive(PrimitiveTy::I32));
        assert_eq!(*elem, target.primitive(PrimitiveTy::I32));
    }
}
