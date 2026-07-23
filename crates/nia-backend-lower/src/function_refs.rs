// SPDX-License-Identifier: GPL-3.0-or-later
use nia_function_ir::{FunctionBodyRefs, FunctionInstanceRef};
use nia_ids::ModuleId;
use nia_span::Span;
use nia_static_ir::StaticInit;

pub(crate) fn collect_function_refs_from_static_init(
    module_id: ModuleId,
    init: &StaticInit,
    refs: &mut FunctionBodyRefs,
) {
    match init {
        StaticInit::Array(elems) => {
            for elem in elems {
                collect_function_refs_from_static_init(module_id, elem, refs);
            }
        }
        StaticInit::Repeat { value, count } => {
            if *count != 0 {
                collect_function_refs_from_static_init(module_id, value, refs);
            }
        }
        StaticInit::Struct(fields) => {
            for field in fields {
                collect_function_refs_from_static_init(module_id, &field.value, refs);
            }
        }
        StaticInit::StaticArrayPointer { array_init, .. } => {
            collect_function_refs_from_static_init(module_id, array_init, refs);
        }
        StaticInit::AddrOfGlobal { global, .. } => {
            refs.globals.insert(*global);
        }
        StaticInit::AddrOfFunction { function, args } => {
            refs.types.extend(args.iter().copied());
            if args.is_empty() {
                refs.functions.insert(*function);
            } else {
                refs.function_instances.push(FunctionInstanceRef {
                    def_id: *function,
                    arg_module_id: module_id,
                    self_arg: None,
                    args: args.clone(),
                    const_args: Vec::new(),
                    span: Span::default(),
                });
            }
        }
        StaticInit::Zero
        | StaticInit::Int(_)
        | StaticInit::Float(_)
        | StaticInit::Bool(_)
        | StaticInit::Char(_)
        | StaticInit::Byte(_)
        | StaticInit::Chars(_)
        | StaticInit::Bytes(_)
        | StaticInit::NullPtr => {}
    }
}
