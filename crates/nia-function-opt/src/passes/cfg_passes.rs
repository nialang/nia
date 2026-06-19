use super::*;

pub(crate) fn fold_constant_bool_branches(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= fold_constant_bool_branches(&mut body.blocks);
            }
        }
        if let FunctionTerminator::If {
            cond,
            then_target,
            else_target,
            span,
        } = &block.terminator
            && let FunctionExprKind::Bool(value) = cond.kind
        {
            block.terminator = FunctionTerminator::Branch {
                target: if value { *then_target } else { *else_target },
                span: *span,
            };
            changed = true;
        }
    }
    changed
}

pub(crate) fn fold_constant_switches(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= fold_constant_switches(&mut body.blocks);
            }
        }
        if let FunctionTerminator::Switch {
            target,
            arms,
            default,
            fallback,
            span,
        } = &block.terminator
            && let Some(target_value) = switch_constant_value(target)
            && let Some(selected) = constant_switch_target(&target_value, arms, *default, *fallback)
        {
            block.terminator = FunctionTerminator::Branch {
                target: selected,
                span: *span,
            };
            changed = true;
        }
    }
    changed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitchConstantValue {
    Integer(i128),
    Bool(bool),
    Char(u32),
    Byte(u8),
    EnumVariant(nia_ids::GlobalDefId),
}

pub(crate) fn constant_switch_target(
    target: &SwitchConstantValue,
    arms: &[nia_function_ir::FunctionSwitchArm],
    default: Option<FunctionBlockId>,
    fallback: FunctionBlockId,
) -> Option<FunctionBlockId> {
    let patterns = arms
        .iter()
        .map(|arm| switch_constant_value(&arm.pattern))
        .collect::<Option<Vec<_>>>()?;
    for (arm, pattern) in arms.iter().zip(patterns) {
        if pattern == *target {
            return Some(arm.target);
        }
    }
    Some(default.unwrap_or(fallback))
}

pub(crate) fn switch_constant_value(expr: &FunctionExpr) -> Option<SwitchConstantValue> {
    match &expr.kind {
        FunctionExprKind::Integer(text) => nia_literals::eval_int_literal(text)
            .ok()
            .map(SwitchConstantValue::Integer),
        FunctionExprKind::Bool(value) => Some(SwitchConstantValue::Bool(*value)),
        FunctionExprKind::Char(value) => Some(SwitchConstantValue::Char(*value)),
        FunctionExprKind::ByteChar(text) => {
            decode_byte_char_literal(text).map(SwitchConstantValue::Byte)
        }
        FunctionExprKind::EnumVariant(def_id) => Some(SwitchConstantValue::EnumVariant(*def_id)),
        FunctionExprKind::Error
        | FunctionExprKind::Trap
        | FunctionExprKind::Float(_)
        | FunctionExprKind::String(_)
        | FunctionExprKind::ByteString(_)
        | FunctionExprKind::Local(_)
        | FunctionExprKind::Global(_)
        | FunctionExprKind::Function(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::BuiltinValue(_)
        | FunctionExprKind::Discard(_)
        | FunctionExprKind::Range(_)
        | FunctionExprKind::RangeBound { .. }
        | FunctionExprKind::ArrayLiteral { .. }
        | FunctionExprKind::StructLiteral { .. }
        | FunctionExprKind::UnionLiteral { .. }
        | FunctionExprKind::Null
        | FunctionExprKind::OptionalSome { .. }
        | FunctionExprKind::ErrorOk { .. }
        | FunctionExprKind::ErrorErr { .. }
        | FunctionExprKind::TaggedUnionTag { .. }
        | FunctionExprKind::TaggedUnionPayload { .. }
        | FunctionExprKind::Try { .. }
        | FunctionExprKind::Unary { .. }
        | FunctionExprKind::LoadUnaligned { .. }
        | FunctionExprKind::Splat { .. }
        | FunctionExprKind::Bitmask { .. }
        | FunctionExprKind::BitIntrinsic { .. }
        | FunctionExprKind::ExtractElement { .. }
        | FunctionExprKind::InsertElement { .. }
        | FunctionExprKind::Binary { .. }
        | FunctionExprKind::Cast { .. }
        | FunctionExprKind::InlineAsm(_)
        | FunctionExprKind::StaticArrayPointer { .. }
        | FunctionExprKind::AddrOf(_)
        | FunctionExprKind::Assign { .. }
        | FunctionExprKind::TraitObjectUpcast { .. }
        | FunctionExprKind::TraitObjectCoercion { .. }
        | FunctionExprKind::Call { .. }
        | FunctionExprKind::Atomic(_)
        | FunctionExprKind::Field { .. }
        | FunctionExprKind::Index { .. }
        | FunctionExprKind::Slice { .. } => None,
    }
}

pub(crate) fn decode_byte_char_literal(text: &str) -> Option<u8> {
    let inner = text.strip_prefix("b'")?.strip_suffix('\'')?;
    let mut chars = inner.chars();
    let ch = match chars.next()? {
        '\\' => match chars.next()? {
            'n' => b'\n',
            'r' => b'\r',
            't' => b'\t',
            '\\' => b'\\',
            '\'' => b'\'',
            '"' => b'"',
            '0' => b'\0',
            'x' => {
                let hi = chars.next()?.to_digit(16)?;
                let lo = chars.next()?.to_digit(16)?;
                ((hi << 4) | lo) as u8
            }
            _ => return None,
        },
        ch if ch.is_ascii() => ch as u8,
        _ => return None,
    };
    chars.next().is_none().then_some(ch)
}

pub(crate) fn simplify_trivial_branches(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= simplify_trivial_branches(&mut body.blocks);
            }
        }
        if let FunctionTerminator::If {
            cond,
            then_target,
            else_target,
            span,
        } = &block.terminator
            && then_target == else_target
            && is_pure_discardable_expr(cond)
        {
            block.terminator = FunctionTerminator::Branch {
                target: *then_target,
                span: *span,
            };
            changed = true;
        }
    }
    changed
}

pub(crate) fn simplify_same_target_switches(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= simplify_same_target_switches(&mut body.blocks);
            }
        }
        if let FunctionTerminator::Switch {
            target,
            arms,
            default,
            fallback,
            span,
        } = &block.terminator
            && is_pure_discardable_expr(target)
            && arms
                .iter()
                .all(|arm| is_pure_discardable_expr(&arm.pattern))
            && switch_targets_same(arms, *default, *fallback)
        {
            block.terminator = FunctionTerminator::Branch {
                target: *fallback,
                span: *span,
            };
            changed = true;
        }
    }
    changed
}

pub(crate) fn switch_targets_same(
    arms: &[nia_function_ir::FunctionSwitchArm],
    default: Option<FunctionBlockId>,
    fallback: FunctionBlockId,
) -> bool {
    arms.iter().all(|arm| arm.target == fallback) && default.is_none_or(|target| target == fallback)
}

pub(crate) fn merge_empty_jump_blocks(body: &mut FunctionBody) -> bool {
    let mut any_changed = false;
    loop {
        let empty_jumps = empty_jump_targets(&body.blocks, Some(body.entry));
        if empty_jumps.is_empty() {
            break;
        }
        let mut changed = false;
        for block in &mut body.blocks {
            changed |= retarget_terminator(&mut block.terminator, &empty_jumps);
        }
        if !changed {
            break;
        }
        any_changed = true;
    }
    any_changed
}

pub(crate) fn empty_jump_targets(
    blocks: &[FunctionBlock],
    protected_entry: Option<FunctionBlockId>,
) -> HashMap<FunctionBlockId, FunctionBlockId> {
    let cfg = FunctionCfg::new(blocks);
    blocks
        .iter()
        .filter(|block| Some(block.id) != protected_entry && block.ops.is_empty())
        .filter(|block| !cfg.predecessors(block.id).is_empty())
        .filter_map(|block| {
            let target = jump_target(&block.terminator)?;
            let target_block = &blocks[cfg.block(target)?];
            (target != block.id && target_block.scope == block.scope).then_some((block.id, target))
        })
        .collect()
}

pub(crate) fn jump_target(terminator: &FunctionTerminator) -> Option<FunctionBlockId> {
    match terminator {
        FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
            Some(*target)
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::If { .. }
        | FunctionTerminator::Switch { .. }
        | FunctionTerminator::Try { .. }
        | FunctionTerminator::Loop { .. }
        | FunctionTerminator::Return { .. }
        | FunctionTerminator::Tail { .. } => None,
    }
}

pub(crate) fn retarget_terminator(
    terminator: &mut FunctionTerminator,
    empty_jumps: &HashMap<FunctionBlockId, FunctionBlockId>,
) -> bool {
    match terminator {
        FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
            retarget_block_id(target, empty_jumps)
        }
        FunctionTerminator::If {
            then_target,
            else_target,
            ..
        } => {
            let then_changed = retarget_block_id(then_target, empty_jumps);
            let else_changed = retarget_block_id(else_target, empty_jumps);
            then_changed || else_changed
        }
        FunctionTerminator::Switch {
            arms,
            default,
            fallback,
            ..
        } => {
            let mut changed = false;
            for arm in arms {
                changed |= retarget_block_id(&mut arm.target, empty_jumps);
            }
            if let Some(default) = default {
                changed |= retarget_block_id(default, empty_jumps);
            }
            changed |= retarget_block_id(fallback, empty_jumps);
            changed
        }
        FunctionTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } => {
            let body_changed = retarget_block_id(body, empty_jumps);
            let continue_changed = retarget_block_id(continue_target, empty_jumps);
            let break_changed = retarget_block_id(break_target, empty_jumps);
            body_changed || continue_changed || break_changed
        }
        FunctionTerminator::Try { success_target, .. } => {
            retarget_block_id(success_target, empty_jumps)
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Return { .. }
        | FunctionTerminator::Tail { .. } => false,
    }
}

pub(crate) fn retarget_block_id(
    target: &mut FunctionBlockId,
    empty_jumps: &HashMap<FunctionBlockId, FunctionBlockId>,
) -> bool {
    let mut changed = false;
    let mut seen = HashSet::new();
    while let Some(next) = empty_jumps.get(target).copied() {
        if !seen.insert(*target) {
            break;
        }
        *target = next;
        changed = true;
    }
    changed
}

pub(crate) fn remove_unreachable_blocks(body: &mut FunctionBody) -> bool {
    let reachable = reachable_blocks(body);
    if reachable.len() == body.blocks.len() {
        return false;
    }
    body.blocks.retain(|block| reachable.contains(&block.id));
    true
}

pub(crate) fn reachable_blocks(body: &FunctionBody) -> HashSet<FunctionBlockId> {
    FunctionCfg::new(&body.blocks).reachable_from(&body.blocks, body.entry)
}

pub(crate) fn reachable_defer_blocks(body: &FunctionDeferBody) -> HashSet<FunctionBlockId> {
    DeferCfg::new(&body.blocks).reachable_from(&body.blocks, body.entry)
}

pub(crate) fn terminator_referenced_blocks(
    terminator: &FunctionTerminator,
) -> Vec<FunctionBlockId> {
    match terminator {
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Return { .. }
        | FunctionTerminator::Tail { .. } => Vec::new(),
        FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
            vec![*target]
        }
        FunctionTerminator::Try { success_target, .. } => vec![*success_target],
        FunctionTerminator::If {
            then_target,
            else_target,
            ..
        } => vec![*then_target, *else_target],
        FunctionTerminator::Switch {
            arms,
            default,
            fallback,
            ..
        } => {
            let mut targets = arms.iter().map(|arm| arm.target).collect::<Vec<_>>();
            if let Some(default) = default {
                targets.push(*default);
            }
            targets.push(*fallback);
            targets
        }
        FunctionTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } => vec![*body, *continue_target, *break_target],
    }
}

pub(crate) fn optimize_defer_bodies(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= simplify_trivial_branches(&mut body.blocks);
                changed |= merge_empty_defer_jump_blocks(body);
                changed |= remove_unreachable_defer_blocks(body);
                changed |= optimize_defer_bodies(&mut body.blocks);
            }
        }
    }
    changed
}

pub(crate) fn merge_empty_defer_jump_blocks(body: &mut FunctionDeferBody) -> bool {
    let mut any_changed = false;
    loop {
        let empty_jumps = empty_jump_targets(&body.blocks, Some(body.entry));
        if empty_jumps.is_empty() {
            break;
        }
        let mut changed = false;
        for block in &mut body.blocks {
            changed |= retarget_terminator(&mut block.terminator, &empty_jumps);
        }
        if !changed {
            break;
        }
        any_changed = true;
    }
    any_changed
}

pub(crate) fn remove_unreachable_defer_blocks(body: &mut FunctionDeferBody) -> bool {
    let reachable = reachable_defer_blocks(body);
    if reachable.len() == body.blocks.len() {
        return false;
    }
    body.blocks.retain(|block| reachable.contains(&block.id));
    true
}
