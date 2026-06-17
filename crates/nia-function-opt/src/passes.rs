use std::collections::{HashMap, HashSet};

use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBlockId, FunctionBody, FunctionCallee,
    FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionInlineAsm,
    FunctionLocalKind, FunctionMemoryIntrinsicSource, FunctionOp, FunctionPlace, FunctionPlaceBase,
    FunctionPlaceElem, FunctionRange, FunctionSliceRange, FunctionTerminator,
};
use nia_ids::{InternedTyId, LocalId};

mod casts;
mod cfg;
mod cfg_passes;
mod const_prop;
mod copy_prop;
mod dce;
mod local_analysis;
mod purity;
mod traversal;

pub(crate) use casts::*;
pub(crate) use cfg::*;
pub(crate) use cfg_passes::*;
pub(crate) use const_prop::*;
pub(crate) use copy_prop::*;
pub(crate) use dce::*;
pub(crate) use local_analysis::*;
pub(crate) use purity::*;
pub(crate) use traversal::*;
