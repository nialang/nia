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
