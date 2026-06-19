// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use nia_static_ir::StaticInit;
use nia_ty::IntConst;

fn static_int(value: i128) -> StaticInit {
    StaticInit::Int(IntConst::from_i128(value))
}

mod aggregate_layouts;
mod assignments;
mod basics;
mod builtins_and_imports;
mod control_flow;
mod diagnostics_and_traps;
mod generic_inference;
mod strings_and_targets;
mod typed_values;
