use crate::{
    ConstCheck, ConstInput, ConstKey, ConstModuleInput, ConstModuleLowering, ConstProgramContext,
    ConstValueType, check_module_const, lower_module_const,
};
use nia_const_ir::{EarlyConstExpr, EarlyConstExprKind, EarlyConstTypeArg};
use nia_defs::{DefCollection, DefKind, ModuleId, collect_module_defs};
use nia_ids::{GlobalDefId, ModuleIdAllocator};
use nia_item_signatures::{ItemSignatureInput, ItemSignatureSource, collect_item_signatures};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_local_resolve::{LocalResolution, resolve_module_locals};
use nia_parser::parse_module_with_symbols;
use nia_sema_ir::SemanticUseTable;
use nia_source::SourcePath;
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use nia_symbol_table::SymbolTable;
use nia_ty::{PrimitiveTy, TyKind, TypeStore};
use nia_type_lower::{
    TypeLowering, TypeLoweringContext, lower_module_types_from_item_tree_with_context,
    lower_module_types_with_context,
};
use nia_type_resolve::resolve_module_types_with_symbols;
use nia_value_resolve::resolve_module_values;
use std::collections::HashMap;

#[path = "tests/resolution_contracts.rs"]
mod resolution_contracts;
#[path = "tests/test_support.rs"]
mod test_support;
use test_support::*;
#[path = "tests/typed_values.rs"]
mod typed_values;
