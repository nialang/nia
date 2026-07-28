// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn const_enum_values_from_check(
    const_eval: &nia_const_check::ConstCheck,
) -> nia_const_check::ConstEnumValues {
    nia_const_check::ConstEnumValues {
        values: const_eval.enum_values.clone(),
        typed_values: const_eval.typed_enum_values.clone(),
        diagnostics: Vec::new(),
    }
}

pub(super) fn active_item_tree(module: &nia_ast::Module) -> ActiveModuleItemTree {
    let item_tree = ModuleItemTree::from_module(module);
    ActiveModuleItemTree::new(item_tree.active_items_without_const(), Default::default())
}

pub(super) fn global_def_id_by_name(defs: &nia_defs::DefCollection, name: &str) -> GlobalDefId {
    let name_symbol = sym(name);
    defs.defs
        .iter()
        .find_map(|(def_id, def)| {
            (def.name == name_symbol).then_some(GlobalDefId {
                module_id: defs.module_id,
                def_id,
            })
        })
        .unwrap_or_else(|| panic!("missing def `{name}`"))
}

pub(super) fn nominal_type_by_def(
    type_store: &nia_ty::TypeStore,
    lowering: &TypeLowering,
    target: GlobalDefId,
) -> InternedTyId {
    nominal_type_by_def_with_args(type_store, lowering, target, &[])
}

pub(super) fn nominal_type_by_def_with_args(
    type_store: &nia_ty::TypeStore,
    lowering: &TypeLowering,
    target: GlobalDefId,
    target_args: &[InternedTyId],
) -> InternedTyId {
    lowering
        .explicit_type_roots()
        .into_iter()
        .find(|ty| {
            matches!(
                type_store.get(*ty),
                Some(nia_ty::TyKind::Nominal {
                    def_id,
                    args,
                    ..
                }) if *def_id == target && args == target_args
            )
        })
        .unwrap_or_else(|| panic!("missing nominal type {target:?} with args {target_args:?}"))
}

pub(super) fn first_terminal_value(
    body: &nia_function_ir::FunctionBody,
) -> &nia_function_ir::FunctionExpr {
    body.blocks
        .iter()
        .find_map(|block| match &block.terminator {
            FunctionTerminator::Return {
                value: Some(value), ..
            }
            | FunctionTerminator::Tail {
                value: Some(value), ..
            } => Some(value),
            _ => None,
        })
        .expect("terminal value")
}

pub(super) fn first_terminal_value_mut(
    body: &mut nia_function_ir::FunctionBody,
) -> &mut nia_function_ir::FunctionExpr {
    body.blocks
        .iter_mut()
        .find_map(|block| match &mut block.terminator {
            FunctionTerminator::Return {
                value: Some(value), ..
            }
            | FunctionTerminator::Tail {
                value: Some(value), ..
            } => Some(value),
            _ => None,
        })
        .expect("terminal value")
}
