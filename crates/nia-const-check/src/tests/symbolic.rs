// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::ConstValue;
use nia_ty::{ArrayLenTy, ConstGenericValue};

#[test]
fn symbolic_local_aliases_are_facts_before_body_checking() {
    let fixture = check_source(
        r#"
fun lengths[N: usize]() usize {
    const M = N;
    const K: usize = M;
    let bytes: [u8; K] = [0; M];
    K
}
fun flags[N: bool]() bool { const M = N; M }
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(fixture.checked.symbolic_locals.len(), 3);
    assert_eq!(fixture.checked.symbolic_array_lengths.len(), 1);
    assert!(
        fixture
            .checked
            .symbolic_array_lengths
            .values()
            .all(|len| matches!(len, ArrayLenTy::GenericParam(name) if *name == sym("N")))
    );
    for (id, arg) in fixture.checked.symbolic_locals.iter() {
        assert_eq!(arg.value, ConstGenericValue::GenericParam(sym("N")));
        assert!(!fixture.checked.values.contains_key(&ConstKey::Local(*id)));
        assert!(!fixture.checked.failed.contains(&ConstKey::Local(*id)));
    }
    assert!(
        fixture.checked.symbolic_locals.values().any(
            |arg| fixture.type_store.get(arg.ty) == Some(&TyKind::Primitive(PrimitiveTy::Bool))
        )
    );
}

#[test]
fn const_function_locals_execute_with_each_invocations_substitution() {
    let fixture = check_source(
        r#"
const fun length[N: usize]() usize { const M = N; M }
const A: usize = length[3]();
const B: usize = length[7]();
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    let values = fixture
        .checked
        .values
        .iter()
        .filter_map(|(key, value)| matches!(key, ConstKey::Global(_)).then_some(value))
        .collect::<Vec<_>>();
    assert!(
        values
            .iter()
            .any(|value| matches!(value, ConstValue::Int(n) if n.bits() == 3)),
        "{values:?}"
    );
    assert!(
        values
            .iter()
            .any(|value| matches!(value, ConstValue::Int(n) if n.bits() == 7)),
        "{values:?}"
    );
}

#[test]
fn extension_const_parameters_have_their_declared_type_in_local_aliases() {
    let fixture = check_source(
        r#"
struct Buffer[N: usize] {}
extend[N: usize] Buffer[N] {
    fun length() usize { const M = N; M }
}
trait Flag[B: bool] {
    fun flag() bool { const M = B; M }
}
"#,
    );
    assert!(
        fixture.checked.diagnostics.is_empty(),
        "{:?}",
        fixture.checked.diagnostics
    );
    assert_eq!(fixture.checked.symbolic_locals.len(), 2);
    assert!(
        fixture.checked.symbolic_locals.values().any(
            |arg| fixture.type_store.get(arg.ty) == Some(&TyKind::Primitive(PrimitiveTy::Bool))
        )
    );
    assert!(
        fixture
            .checked
            .symbolic_locals
            .values()
            .any(|arg| fixture.type_store.get(arg.ty)
                == Some(&TyKind::Primitive(PrimitiveTy::Usize)))
    );
}
