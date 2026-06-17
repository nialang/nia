use super::*;

pub(crate) struct FunctionCfg {
    blocks_by_id: HashMap<FunctionBlockId, usize>,
    predecessors: HashMap<FunctionBlockId, Vec<FunctionBlockId>>,
}

impl FunctionCfg {
    pub(crate) fn new(blocks: &[FunctionBlock]) -> Self {
        let blocks_by_id = blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block.id, index))
            .collect::<HashMap<_, _>>();
        let mut predecessors: HashMap<FunctionBlockId, Vec<FunctionBlockId>> = HashMap::new();
        for block in blocks {
            predecessors.entry(block.id).or_default();
            for target in terminator_referenced_blocks(&block.terminator) {
                if blocks_by_id.contains_key(&target) {
                    predecessors.entry(target).or_default().push(block.id);
                }
            }
        }
        Self {
            blocks_by_id,
            predecessors,
        }
    }

    pub(crate) fn block(&self, id: FunctionBlockId) -> Option<usize> {
        self.blocks_by_id.get(&id).copied()
    }

    pub(crate) fn predecessors(&self, id: FunctionBlockId) -> &[FunctionBlockId] {
        self.predecessors.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn referenced_blocks(
        &self,
        terminator: &FunctionTerminator,
    ) -> Vec<FunctionBlockId> {
        terminator_referenced_blocks(terminator)
            .into_iter()
            .filter(|id| self.blocks_by_id.contains_key(id))
            .collect()
    }

    pub(crate) fn reachable_from(
        &self,
        blocks: &[FunctionBlock],
        entry: FunctionBlockId,
    ) -> HashSet<FunctionBlockId> {
        let mut reachable = HashSet::new();
        let mut stack = vec![entry];
        while let Some(id) = stack.pop() {
            if !reachable.insert(id) {
                continue;
            }
            let Some(index) = self.block(id) else {
                continue;
            };
            stack.extend(self.referenced_blocks(&blocks[index].terminator));
        }
        reachable
    }
}

#[derive(Debug)]
pub(crate) struct DeferCfg {
    blocks_by_id: HashMap<FunctionBlockId, usize>,
}

impl DeferCfg {
    pub(crate) fn new(blocks: &[FunctionBlock]) -> Self {
        Self {
            blocks_by_id: blocks
                .iter()
                .enumerate()
                .map(|(index, block)| (block.id, index))
                .collect(),
        }
    }

    pub(crate) fn block(&self, id: FunctionBlockId) -> Option<usize> {
        self.blocks_by_id.get(&id).copied()
    }

    pub(crate) fn referenced_blocks(
        &self,
        terminator: &FunctionTerminator,
    ) -> Vec<FunctionBlockId> {
        terminator_referenced_blocks(terminator)
            .into_iter()
            .filter(|id| self.blocks_by_id.contains_key(id))
            .collect()
    }

    pub(crate) fn reachable_from(
        &self,
        blocks: &[FunctionBlock],
        entry: FunctionBlockId,
    ) -> HashSet<FunctionBlockId> {
        let mut reachable = HashSet::new();
        let mut stack = vec![entry];
        while let Some(id) = stack.pop() {
            if !reachable.insert(id) {
                continue;
            }
            let Some(index) = self.block(id) else {
                continue;
            };
            stack.extend(self.referenced_blocks(&blocks[index].terminator));
        }
        reachable
    }
}
