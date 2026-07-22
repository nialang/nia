// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ids::{GlobalDefId, InternedTyId};
use nia_ty::IntConst;
use std::collections::HashSet;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticInitRefs {
    pub functions: HashSet<GlobalDefId>,
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
    StaticArrayPointer {
        array_ty: InternedTyId,
        array_init: Box<StaticInit>,
    },
}

impl StaticInit {
    pub fn refs(&self) -> StaticInitRefs {
        let mut refs = StaticInitRefs::default();
        self.collect_refs(&mut refs);
        refs
    }

    fn collect_refs(&self, refs: &mut StaticInitRefs) {
        match self {
            Self::Array(elements) => {
                for element in elements {
                    element.collect_refs(refs);
                }
            }
            Self::Repeat { value, count } => {
                if *count != 0 {
                    value.collect_refs(refs);
                }
            }
            Self::Struct(fields) => {
                for field in fields {
                    field.value.collect_refs(refs);
                }
            }
            Self::AddrOfGlobal { global, .. } => {
                refs.globals.insert(*global);
            }
            Self::AddrOfFunction { function, .. } => {
                refs.functions.insert(*function);
            }
            Self::StaticArrayPointer { array_init, .. } => array_init.collect_refs(refs),
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
}
