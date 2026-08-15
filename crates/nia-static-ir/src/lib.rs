// SPDX-License-Identifier: GPL-3.0-or-later
use nia_function_ir::{FunctionBodyRefs, FunctionInstanceRef};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_span::Span;
use nia_ty::IntConst;
use std::collections::HashSet;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticInitRefs {
    /// Runtime functions retained because a static initializer stores their address.
    pub functions: HashSet<GlobalDefId>,
    /// Globals retained because a static initializer stores their address.
    pub globals: HashSet<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StaticInit {
    Zero,
    Int(IntConst),
    Float(String),
    Bool(bool),
    Char(u32),
    Byte(u8),
    Chars(Vec<u32>),
    Bytes(Vec<u8>),
    Array(Vec<StaticInit>),
    Repeat {
        value: Box<StaticInit>,
        count: u64,
    },
    Struct(Vec<StaticFieldInit>),
    NullPtr,
    AddrOfGlobal {
        global: GlobalDefId,
        path: Vec<StaticAddressElem>,
    },
    AddrOfFunction {
        function: GlobalDefId,
        args: Vec<InternedTyId>,
    },
}

impl StaticInit {
    /// Collect direct relocation targets from an initializer.
    ///
    /// A zero-length repeat has no materialized elements, so its value is not
    /// visited. This keeps reachability precise: an unused function/global
    /// address inside `[value; 0]` must not keep code or storage alive.
    pub fn refs(&self) -> StaticInitRefs {
        let mut refs = StaticInitRefs::default();
        self.visit_refs(&mut refs);
        refs
    }

    /// Convert relocations into executable reachability edges.
    ///
    /// Generic function addresses become typed function-instance references;
    /// non-generic addresses stay ordinary function edges. `module_id` is the
    /// owner used when constructing those instance identities.
    pub fn value_refs(&self, module_id: ModuleId) -> FunctionBodyRefs {
        let mut refs = FunctionBodyRefs::default();
        self.visit_refs(&mut FunctionBodyRefSink {
            module_id,
            refs: &mut refs,
        });
        refs
    }

    fn visit_refs(&self, sink: &mut impl StaticInitRefSink) {
        match self {
            Self::Array(elements) => {
                for element in elements {
                    element.visit_refs(sink);
                }
            }
            Self::Repeat { value, count } => {
                if *count != 0 {
                    value.visit_refs(sink);
                }
            }
            Self::Struct(fields) => {
                for field in fields {
                    field.value.visit_refs(sink);
                }
            }
            Self::AddrOfGlobal { global, .. } => {
                sink.global(*global);
            }
            Self::AddrOfFunction { function, args } => {
                sink.function(*function, args);
            }
            Self::Zero
            | Self::Int(_)
            | Self::Float(_)
            | Self::Bool(_)
            | Self::Char(_)
            | Self::Byte(_)
            | Self::Chars(_)
            | Self::Bytes(_)
            | Self::NullPtr => {}
        }
    }
}

trait StaticInitRefSink {
    fn global(&mut self, global: GlobalDefId);
    fn function(&mut self, function: GlobalDefId, args: &[InternedTyId]);
}

impl StaticInitRefSink for StaticInitRefs {
    fn global(&mut self, global: GlobalDefId) {
        self.globals.insert(global);
    }

    fn function(&mut self, function: GlobalDefId, _args: &[InternedTyId]) {
        self.functions.insert(function);
    }
}

struct FunctionBodyRefSink<'a> {
    module_id: ModuleId,
    refs: &'a mut FunctionBodyRefs,
}

impl StaticInitRefSink for FunctionBodyRefSink<'_> {
    fn global(&mut self, global: GlobalDefId) {
        self.refs.globals.insert(global);
    }

    fn function(&mut self, function: GlobalDefId, args: &[InternedTyId]) {
        self.refs.types.extend(args.iter().copied());
        if args.is_empty() {
            self.refs.functions.insert(function);
        } else {
            self.refs.function_instances.push(FunctionInstanceRef {
                def_id: function,
                arg_module_id: self.module_id,
                self_arg: None,
                args: args.to_vec(),
                const_args: Vec::new(),
                span: Span::default(),
            });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticAddressElem {
    Field(GlobalDefId),
    Index(u64),
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticFieldInit {
    pub field: Option<GlobalDefId>,
    pub value: StaticInit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{DefId, ModuleIdAllocator};

    #[test]
    fn empty_repeat_does_not_retain_static_references() {
        let module_id = ModuleIdAllocator::new().allocate();
        let function = GlobalDefId {
            module_id,
            def_id: DefId(0),
        };
        let global = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let init = StaticInit::Array(vec![
            StaticInit::Repeat {
                value: Box::new(StaticInit::AddrOfFunction {
                    function,
                    args: Vec::new(),
                }),
                count: 0,
            },
            StaticInit::Repeat {
                value: Box::new(StaticInit::AddrOfGlobal {
                    global,
                    path: Vec::new(),
                }),
                count: 0,
            },
        ]);

        assert_eq!(init.refs(), StaticInitRefs::default());
    }

    #[test]
    fn nonempty_static_references_are_deduplicated() {
        let module_id = ModuleIdAllocator::new().allocate();
        let function = GlobalDefId {
            module_id,
            def_id: DefId(0),
        };
        let global = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let init = StaticInit::Array(vec![
            StaticInit::AddrOfFunction {
                function,
                args: Vec::new(),
            },
            StaticInit::AddrOfFunction {
                function,
                args: Vec::new(),
            },
            StaticInit::AddrOfGlobal {
                global,
                path: Vec::new(),
            },
        ]);

        let refs = init.refs();
        assert_eq!(refs.functions, HashSet::from([function]));
        assert_eq!(refs.globals, HashSet::from([global]));
    }

    #[test]
    fn typed_refs_preserve_function_instance_identity() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let function = GlobalDefId {
            module_id,
            def_id: DefId(0),
        };
        let types = nia_ty::TypeStore::new();
        let arg = types
            .append_for_module(module_id)
            .primitive(nia_ty::PrimitiveTy::Usize);
        let init = StaticInit::Array(vec![
            StaticInit::AddrOfFunction {
                function,
                args: vec![arg],
            },
            StaticInit::Repeat {
                value: Box::new(StaticInit::AddrOfFunction {
                    function,
                    args: vec![arg],
                }),
                count: 0,
            },
        ]);

        let refs = init.value_refs(module_id);

        assert!(refs.functions.is_empty());
        assert_eq!(refs.types, std::collections::BTreeSet::from([arg]));
        assert_eq!(refs.function_instances.len(), 1);
        assert_eq!(refs.function_instances[0].def_id, function);
        assert_eq!(refs.function_instances[0].arg_module_id, module_id);
        assert_eq!(refs.function_instances[0].args, vec![arg]);
    }
}
