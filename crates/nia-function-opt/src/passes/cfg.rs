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
            for target in block.terminator.referenced_blocks() {
                if blocks_by_id.contains_key(&target) {
                    predecessors.entry(target).or_default().push(block.id);
                }
            }
            // A defer body has its own mini-CFG, but its `break`/`continue`
            // terminators may target blocks in this enclosing CFG. Those
            // edges are not visible through the enclosing terminator, so they
            // must participate in reachability or the target can be deleted
            // as apparently dead. Internal defer targets are filtered by the
            // enclosing block table and are handled by the defer CFG itself.
            for target in defer_referenced_blocks(&block.ops) {
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
        terminator
            .referenced_blocks()
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
            stack.extend(
                defer_referenced_blocks(&blocks[index].ops)
                    .into_iter()
                    .filter(|target| self.blocks_by_id.contains_key(target)),
            );
        }
        reachable
    }
}

fn defer_referenced_blocks(ops: &[FunctionOp]) -> Vec<FunctionBlockId> {
    fn collect(ops: &[FunctionOp], targets: &mut Vec<FunctionBlockId>) {
        for op in ops {
            let FunctionOp::Defer(body) = op else {
                continue;
            };
            for block in &body.blocks {
                targets.extend(block.terminator.referenced_blocks());
                collect(&block.ops, targets);
            }
        }
    }

    let mut targets = Vec::new();
    collect(ops, &mut targets);
    targets
}
