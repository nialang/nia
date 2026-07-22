// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU32, Ordering},
};

use nia_ids::GlobalDefId;

use crate::FunctionBody;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionBodyStoreId(u32);

impl FunctionBodyStoreId {
    fn fresh() -> Self {
        static NEXT_STORE_ID: AtomicU32 = AtomicU32::new(1);
        Self(
            NEXT_STORE_ID
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("function body store identity space exhausted"),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionBodyId {
    store_id: FunctionBodyStoreId,
    index: u32,
}

#[derive(Debug)]
pub struct FunctionBodyStore {
    id: FunctionBodyStoreId,
    by_def: HashMap<GlobalDefId, FunctionBodyId>,
    bodies: Vec<FunctionBody>,
}

#[derive(Debug)]
pub struct FunctionBodyStoreBuilder {
    id: FunctionBodyStoreId,
    by_def: HashMap<GlobalDefId, FunctionBodyId>,
    bodies: Vec<FunctionBody>,
}

impl FunctionBodyStore {
    pub fn new() -> Self {
        Self::builder().finish()
    }

    pub fn builder() -> FunctionBodyStoreBuilder {
        FunctionBodyStoreBuilder {
            id: FunctionBodyStoreId::fresh(),
            by_def: HashMap::new(),
            bodies: Vec::new(),
        }
    }

    pub fn id(&self) -> FunctionBodyStoreId {
        self.id
    }

    pub fn id_for_def(&self, def_id: GlobalDefId) -> Option<FunctionBodyId> {
        self.by_def.get(&def_id).copied()
    }

    pub fn get(&self, id: FunctionBodyId) -> Option<&FunctionBody> {
        (id.store_id == self.id)
            .then(|| self.bodies.get(id.index as usize))
            .flatten()
    }

    pub fn body(&self, def_id: GlobalDefId) -> Option<&FunctionBody> {
        self.id_for_def(def_id).and_then(|id| self.get(id))
    }

    pub fn contains_def(&self, def_id: GlobalDefId) -> bool {
        self.by_def.contains_key(&def_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (GlobalDefId, FunctionBodyId, &FunctionBody)> + '_ {
        self.by_def.iter().map(|(def_id, id)| {
            (
                *def_id,
                *id,
                self.get(*id)
                    .expect("function body store index must resolve in its owner"),
            )
        })
    }

    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }
}

impl Default for FunctionBodyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for FunctionBodyStore {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(def_id, _, body)| other.body(def_id) == Some(body))
    }
}

impl FunctionBodyStoreBuilder {
    pub fn insert(&mut self, def_id: GlobalDefId, body: FunctionBody) -> FunctionBodyId {
        let index = u32::try_from(self.bodies.len()).expect("function body store exhausted");
        let id = FunctionBodyId {
            store_id: self.id,
            index,
        };
        let previous = self.by_def.insert(def_id, id);
        assert!(
            previous.is_none(),
            "function body inserted twice for {def_id:?}"
        );
        self.bodies.push(body);
        id
    }

    pub fn finish(self) -> FunctionBodyStore {
        FunctionBodyStore {
            id: self.id,
            by_def: self.by_def,
            bodies: self.bodies,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FunctionBlockId;
    use nia_ids::{DefId, ModuleIdAllocator};
    use nia_span::Span;
    use nia_ty::{PrimitiveTy, TyKind, TypeStore};

    fn body_fixture() -> (GlobalDefId, FunctionBody) {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let types = TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .intern(TyKind::Primitive(PrimitiveTy::I32));
        (
            GlobalDefId {
                module_id,
                def_id: DefId(1),
            },
            FunctionBody {
                span: Span::default(),
                locals: Vec::new(),
                scopes: Vec::new(),
                blocks: Vec::new(),
                entry: FunctionBlockId(0),
                ty,
            },
        )
    }

    #[test]
    fn function_body_ids_are_word_sized_owner_handles() {
        assert_eq!(std::mem::size_of::<FunctionBodyId>(), 8);
    }

    #[test]
    fn function_body_store_resolves_only_its_own_handles() {
        let (def_id, body) = body_fixture();
        let mut first = FunctionBodyStore::builder();
        let first_id = first.insert(def_id, body.clone());
        let first = first.finish();
        let mut second = FunctionBodyStore::builder();
        let second_id = second.insert(def_id, body.clone());
        let second = second.finish();

        assert_eq!(first.id_for_def(def_id), Some(first_id));
        assert_eq!(first.get(first_id), Some(&body));
        assert_eq!(first.get(second_id), None);
        assert_eq!(second.get(first_id), None);
        assert_eq!(first, second);
    }
}
