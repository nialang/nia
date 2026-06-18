use super::*;

#[test]
fn does_not_merge_entry_block_even_when_it_is_an_empty_jump() {
    let span = Span::default();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Next {
                target: FunctionBlockId(1),
                span,
            },
        },
        FunctionBlock {
            id: FunctionBlockId(1),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);

    merge_empty_jump_blocks(&mut body);
    remove_unreachable_blocks(&mut body);

    assert_eq!(
        body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
        vec![FunctionBlockId(0), FunctionBlockId(1)]
    );
    validate_function_body(&body).expect("entry-preserving merge should remain valid");
}

#[test]
fn does_not_merge_empty_jump_blocks_across_scope_boundaries() {
    let span = Span::default();
    let mut body = test_body_with_scopes(
        vec![
            FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            },
            FunctionScope {
                id: FunctionScopeId(1),
                parent: Some(FunctionScopeId(0)),
                span,
            },
        ],
        vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: FunctionBlockId(1),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(1),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: FunctionBlockId(2),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(2),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ],
    );

    merge_empty_jump_blocks(&mut body);
    remove_unreachable_blocks(&mut body);

    assert_eq!(
        body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
        vec![FunctionBlockId(0), FunctionBlockId(1), FunctionBlockId(2)]
    );
    assert_eq!(
        body.edge_exited_scopes(FunctionBlockId(1), FunctionBlockId(2)),
        Some(vec![FunctionScopeId(1)])
    );
    validate_function_body(&body).expect("scope-preserving merge should remain valid");
}
