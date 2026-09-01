use super::*;

pub(super) fn substitute_ty_generics(
    interner: &ConstTypeCx<'_>,
    ty: InternedTyId,
    lookup: &impl Fn(&SymbolId) -> Option<InternedTyId>,
) -> InternedTyId {
    nia_ty::substitute_ty(
        interner.store,
        &interner.append,
        ty,
        lookup,
        &|_| None,
        None,
    )
}

pub(super) fn substitute_ty_generics_and_consts(
    interner: &ConstTypeCx<'_>,
    ty: InternedTyId,
    type_lookup: &impl Fn(&SymbolId) -> Option<InternedTyId>,
    const_lookup: &impl Fn(&SymbolId) -> Option<ConstGenericArg>,
) -> InternedTyId {
    nia_ty::substitute_ty(
        interner.store,
        &interner.append,
        ty,
        type_lookup,
        const_lookup,
        None,
    )
}
