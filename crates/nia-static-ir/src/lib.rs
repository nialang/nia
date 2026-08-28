// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed static initializer IR and relocation reachability extraction.
//!
//! Static values remain target-independent until backend validation. Reference
//! walkers intentionally model only materialized elements, so zero-length
//! repeats do not retain unreachable functions or globals.
use nia_function_ir::{FunctionBodyRefs, FunctionInstanceRef};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_span::Span;
use nia_ty::IntConst;
use std::collections::HashSet;

/// Direct function/global relocation targets found in a static initializer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticInitRefs {
    /// Runtime functions retained because a static initializer stores their address.
    pub functions: HashSet<GlobalDefId>,
    /// Globals retained because a static initializer stores their address.
    pub globals: HashSet<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
/// A typed constant initializer consumed by backend data emission.
///
/// The backend validator checks that each scalar variant matches its declared
/// destination type before LLVM materializes the constant; this enum is not a
/// substitute for that validation.
pub enum StaticInit {
    /// Zero-filled fallback emitted while lowering an already-diagnosed value.
    ///
    /// `StaticInit` is not a validation result: callers must gate code
    /// generation on the owning checker diagnostics. In particular, this
    /// fallback must never turn an unsupported initializer into a silently
    /// accepted zero-valued static.
    Zero,
    /// Target-typed integer constant.
    Int(IntConst),
    /// Floating-point source spelling.
    Float(String),
    /// Boolean constant.
    Bool(bool),
    /// Unicode scalar constant.
    Char(u32),
    /// Byte constant.
    Byte(u8),
    /// Unicode scalar array constant.
    Chars(Vec<u32>),
    /// Byte array constant.
    Bytes(Vec<u8>),
    /// Aggregate elements in declaration order.
    Array(Vec<StaticInit>),
    /// Fixed SIMD-vector lanes in lane order.
    Vector(Vec<StaticInit>),
    /// Repeated aggregate value and materialized count.
    Repeat {
        /// Value repeated for each materialized element.
        value: Box<StaticInit>,
        /// Number of elements.
        count: u64,
    },
    /// Declaration-identified aggregate fields.
    ///
    /// Structs initialize every declared field exactly once; unions initialize
    /// exactly one declared field. Backend validation enforces that distinction
    /// before layout-ordered LLVM constant construction.
    Struct(Vec<StaticFieldInit>),
    /// Null data pointer constant.
    NullPtr,
    /// The address of a global or one of its aggregate fields.
    AddrOfGlobal {
        /// Global whose address is taken.
        global: GlobalDefId,
        /// Aggregate field/index path to the address.
        path: Vec<StaticAddressElem>,
    },
    /// The address of a concrete function or function instance.
    AddrOfFunction {
        /// Function definition whose address is taken.
        function: GlobalDefId,
        /// Type arguments for a concrete function instance.
        args: Vec<InternedTyId>,
        /// Const arguments for a concrete function instance.
        const_args: Vec<nia_ty::ConstGenericArg>,
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
            Self::Array(elements) | Self::Vector(elements) => {
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
            Self::AddrOfFunction {
                function,
                args,
                const_args,
            } => {
                sink.function(*function, args, const_args);
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
    fn function(
        &mut self,
        function: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    );
}

impl StaticInitRefSink for StaticInitRefs {
    fn global(&mut self, global: GlobalDefId) {
        self.globals.insert(global);
    }

    fn function(
        &mut self,
        function: GlobalDefId,
        _args: &[InternedTyId],
        _const_args: &[nia_ty::ConstGenericArg],
    ) {
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

    fn function(
        &mut self,
        function: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) {
        self.refs.types.extend(args.iter().copied());
        self.refs.types.extend(const_args.iter().map(|arg| arg.ty));
        if args.is_empty() && const_args.is_empty() {
            self.refs.functions.insert(function);
        } else {
            self.refs.function_instances.push(FunctionInstanceRef {
                def_id: function,
                arg_module_id: self.module_id,
                self_arg: None,
                args: args.to_vec(),
                const_args: const_args.to_vec(),
                span: Span::default(),
            });
        }
    }
}

/// One projection in a static global address path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticAddressElem {
    /// Aggregate field projection.
    Field(GlobalDefId),
    /// Array index projection.
    Index(u64),
    /// Recovery path element rejected before codegen.
    Error,
}

/// Declaration-identified static aggregate field initializer.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticFieldInit {
    /// Declared field identity, absent for positional syntax.
    pub field: Option<GlobalDefId>,
    /// Field initializer value.
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
                    const_args: Vec::new(),
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
                const_args: Vec::new(),
            },
            StaticInit::AddrOfFunction {
                function,
                args: Vec::new(),
                const_args: Vec::new(),
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
                const_args: Vec::new(),
            },
            StaticInit::Repeat {
                value: Box::new(StaticInit::AddrOfFunction {
                    function,
                    args: vec![arg],
                    const_args: Vec::new(),
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

    #[test]
    fn typed_refs_preserve_function_instance_const_identity() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let function = GlobalDefId {
            module_id,
            def_id: DefId(0),
        };
        let types = nia_ty::TypeStore::new();
        let arg_ty = types
            .append_for_module(module_id)
            .primitive(nia_ty::PrimitiveTy::Usize);
        let init = StaticInit::AddrOfFunction {
            function,
            args: Vec::new(),
            const_args: vec![nia_ty::ConstGenericArg {
                ty: arg_ty,
                value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(3)),
            }],
        };

        let refs = init.value_refs(module_id);
        assert!(refs.functions.is_empty());
        assert_eq!(refs.types, std::collections::BTreeSet::from([arg_ty]));
        assert_eq!(refs.function_instances.len(), 1);
        assert_eq!(
            refs.function_instances[0].const_args[0].value,
            nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(3))
        );
    }
}
