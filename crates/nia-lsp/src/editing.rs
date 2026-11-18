// SPDX-License-Identifier: GPL-3.0-or-later
//! Completion and signature help for the current source revision.

use std::{collections::BTreeMap, path::Path};

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, ParameterInformation, ParameterLabel,
    Position, SignatureHelp, SignatureInformation, TextEdit,
};
use nia_defs::DefKind;
use nia_lexer::{LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_span::Span;
use nia_syntax::{GreenNode, SyntaxKind};

use crate::analysis::{Analysis, File, Symbol};

impl Analysis {
    pub(crate) fn complete(&self, path: &Path, position: Position) -> Option<Vec<CompletionItem>> {
        let file = self.file(path)?;
        let offset = file.lines.offset(&file.text, position)?;
        let start = name_start(&file.text, offset);
        let end = offset
            + file.text[offset..]
                .bytes()
                .take_while(|&byte| is_name_byte(byte))
                .count();
        let prefix = &file.text[start..offset];
        let (qualifier, before) = qualifier(&file.text, start);
        if file.text[..before].trim_end().ends_with('.') {
            return Some(Vec::new());
        }
        if inside_literal_or_comment(&file.text, offset) {
            return Some(Vec::new());
        }
        let mut candidates = BTreeMap::new();
        for name in self.names(file, start, &qualifier).ok()? {
            candidates
                .entry(name.name.to_string())
                .or_insert(name.kind.map(completion_kind));
        }
        if qualifier.is_empty()
            && let Ok(parsed) = nia_syntax::parse(&file.text, None)
        {
            for name in local_names(parsed.tree.green_root(), &file.text, start) {
                candidates.insert(name.to_owned(), Some(CompletionItemKind::VARIABLE));
            }
        }
        let range = file.lines.range(&file.text, Span::new(start, end));
        Some(
            candidates
                .into_iter()
                .filter_map(|(name, kind)| {
                    name.starts_with(prefix).then(|| CompletionItem {
                        label: name.clone(),
                        kind,
                        text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(range, name))),
                        ..CompletionItem::default()
                    })
                })
                .collect(),
        )
    }

    pub(crate) fn signature_help(&self, path: &Path, position: Position) -> Option<SignatureHelp> {
        let file = self.file(path)?;
        let offset = file.lines.offset(&file.text, position)?;
        let call = call_at(&file.text, offset)?;
        let symbol = self.callee(file, &call)?;
        let (source, declaration) = self.declaration_source(symbol)?;
        let parsed = nia_syntax::parse(source, None).ok()?;
        let function = enclosing_function(parsed.tree.green_root(), declaration)?;
        let parameters = function.child(&SyntaxKind::ParamList)?;
        let mut params = parameters
            .child_nodes()
            .filter(|node| *node.kind() == SyntaxKind::Param)
            .peekable();
        if call.method
            && params
                .peek()
                .is_some_and(|param| param.token(&TokenKind::SelfValue).is_some())
        {
            params.next();
        }
        let mut info: Vec<_> = params
            .filter_map(|param| param.token_span())
            .map(|span| ParameterInformation {
                label: ParameterLabel::Simple(source[span.start..span.end].to_owned()),
                documentation: None,
            })
            .collect();
        if parameters.token(&TokenKind::Ellipsis).is_some() {
            info.push(ParameterInformation {
                label: ParameterLabel::Simple("...".to_owned()),
                documentation: None,
            });
        }
        let start = function.first_token()?.span().start;
        let end = function
            .child(&SyntaxKind::Block)
            .and_then(GreenNode::first_token)
            .map_or_else(|| function.span().end, |token| token.span().start);
        let label = source[start..end].trim().trim_end_matches(';').to_owned();
        let active = (!info.is_empty()).then(|| call.argument.min(info.len() as u32 - 1));
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label,
                documentation: None,
                parameters: Some(info),
                active_parameter: active,
            }],
            active_signature: Some(0),
            active_parameter: active,
        })
    }

    fn callee(&self, file: &File, call: &Call) -> Option<Symbol> {
        if let Some(symbol) = self.symbol_at_offset(file, call.name.start) {
            return Some(symbol);
        }
        if call.method {
            return None;
        }
        let name = &file.text[call.name.start..call.name.end];
        let (qualifier, _) = qualifier(&file.text, call.name.start);
        if qualifier.is_empty()
            && let Ok(parsed) = nia_syntax::parse(&file.text, None)
            && local_names(parsed.tree.green_root(), &file.text, call.name.start).contains(&name)
        {
            return None;
        }
        self.names(file, call.name.start, &qualifier)
            .ok()?
            .into_iter()
            .find(|candidate| {
                *candidate.name == *name && candidate.kind == Some(DefKind::Function)
            })?
            .symbol
    }
}

fn completion_kind(kind: DefKind) -> CompletionItemKind {
    match kind {
        DefKind::Module => CompletionItemKind::MODULE,
        DefKind::Function => CompletionItemKind::FUNCTION,
        DefKind::Global => CompletionItemKind::VARIABLE,
        DefKind::Const => CompletionItemKind::CONSTANT,
        DefKind::Struct | DefKind::Union => CompletionItemKind::STRUCT,
        DefKind::Trait => CompletionItemKind::INTERFACE,
        DefKind::Enum => CompletionItemKind::ENUM,
        DefKind::EnumVariant => CompletionItemKind::ENUM_MEMBER,
        DefKind::StructField | DefKind::UnionField | DefKind::EnumVariantField => {
            CompletionItemKind::FIELD
        }
        DefKind::TraitMethod | DefKind::Method => CompletionItemKind::METHOD,
        DefKind::TraitAssociatedType | DefKind::TypeAlias => CompletionItemKind::CLASS,
    }
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn name_start(source: &str, end: usize) -> usize {
    end - source.as_bytes()[..end]
        .iter()
        .rev()
        .take_while(|&&byte| is_name_byte(byte))
        .count()
}

fn qualifier(source: &str, mut start: usize) -> (Vec<&str>, usize) {
    let mut names = Vec::new();
    while let Some(before) = source[..start].trim_end().strip_suffix("::") {
        let end = before.trim_end().len();
        let next = name_start(source, end);
        if next == end {
            break;
        }
        names.push(&source[next..end]);
        start = next;
    }
    names.reverse();
    (names, start)
}

fn inside_literal_or_comment(source: &str, offset: usize) -> bool {
    tokenize_lossless(source)
        .into_iter()
        .find(|token| token.span.start < offset && offset <= token.span.end)
        .is_some_and(|token| match token.kind {
            LosslessTokenKind::LineComment => true,
            LosslessTokenKind::Token(
                TokenKind::String | TokenKind::ByteString | TokenKind::Char | TokenKind::ByteChar,
            ) => offset < token.span.end,
            LosslessTokenKind::Token(TokenKind::Error(_)) => source[token.span.start..offset]
                .trim_start_matches('b')
                .starts_with(['\'', '"']),
            _ => false,
        })
}

fn local_names<'a>(root: &GreenNode, source: &'a str, cursor: usize) -> Vec<&'a str> {
    let mut scope_path = vec![root];
    while let Some(node) = scope_path.last().and_then(|parent| {
        parent.child_nodes().find(|child| {
            let span = child.span();
            span.start <= cursor && cursor < span.end
        })
    }) {
        scope_path.push(node);
    }
    let mut names = Vec::new();
    for (index, scope) in scope_path.iter().enumerate() {
        let inner = scope_path.get(index + 1).map(|node| node.kind());
        match scope.kind() {
            SyntaxKind::Block => {
                for binding in scope
                    .child_nodes()
                    .take_while(|node| node.span().end <= cursor)
                {
                    if *binding.kind() == SyntaxKind::LetStmt {
                        add_pattern_names(binding, source, &mut names);
                    }
                    if *binding.kind() == SyntaxKind::StaticStmt
                        && let Some(binding) = binding.child(&SyntaxKind::BindingDecl)
                    {
                        add_names(binding, source, &mut names);
                    }
                }
            }
            SyntaxKind::FunctionDecl if inner == Some(&SyntaxKind::Block) => {
                add_params(scope.child(&SyntaxKind::ParamList), source, &mut names);
            }
            SyntaxKind::ClosureExpr if inner.is_some_and(SyntaxKind::is_expr) => {
                names.clear();
                add_params(
                    scope.child(&SyntaxKind::ClosureParamList),
                    source,
                    &mut names,
                );
                if let Some(captures) = scope.child(&SyntaxKind::CaptureList) {
                    for capture in captures.child_nodes() {
                        add_names(capture, source, &mut names);
                    }
                }
            }
            SyntaxKind::ForStmt if inner == Some(&SyntaxKind::Block) => {
                add_pattern_names(scope, source, &mut names);
            }
            SyntaxKind::MatchArm
                if scope
                    .token(&TokenKind::FatArrow)
                    .is_some_and(|arrow| arrow.span().end <= cursor) =>
            {
                add_pattern_names(scope, source, &mut names);
            }
            SyntaxKind::IfExpr
                if !scope
                    .token(&TokenKind::Else)
                    .is_some_and(|token| token.span().start < cursor) =>
            {
                for clause in scope
                    .child_nodes()
                    .take_while(|node| node.span().end < cursor)
                {
                    if *clause.kind() == SyntaxKind::IsClause {
                        add_pattern_names(clause, source, &mut names);
                    }
                }
            }
            _ => {}
        }
    }
    names
}

fn add_names<'a>(node: &GreenNode, source: &'a str, names: &mut Vec<&'a str>) {
    if let Some(token) = node
        .child_tokens()
        .find(|token| token.is(&TokenKind::Ident) || token.is(&TokenKind::SelfValue))
    {
        let span = token.span();
        names.push(&source[span.start..span.end]);
    }
}

fn add_params<'a>(list: Option<&GreenNode>, source: &'a str, names: &mut Vec<&'a str>) {
    for param in list.into_iter().flat_map(GreenNode::child_nodes) {
        if *param.kind() == SyntaxKind::Param {
            add_names(param, source, names);
        }
    }
}

fn add_pattern_names<'a>(node: &GreenNode, source: &'a str, names: &mut Vec<&'a str>) {
    if *node.kind() == SyntaxKind::BindPattern {
        add_names(node, source, names);
        return;
    }
    for child in node.child_nodes().filter(|child| {
        child.kind().is_pattern()
            || matches!(
                child.kind(),
                SyntaxKind::NominalNamedFields
                    | SyntaxKind::NominalTupleFields
                    | SyntaxKind::NamedPatternField
            )
    }) {
        if *child.kind() == SyntaxKind::NamedPatternField && child.child_nodes().next().is_none() {
            add_names(child, source, names);
        } else {
            add_pattern_names(child, source, names);
        }
    }
}

fn enclosing_function(node: &GreenNode, name: Span) -> Option<&GreenNode> {
    let span = node.span();
    if name.start < span.start || name.end > span.end {
        return None;
    }
    if *node.kind() == SyntaxKind::FunctionDecl
        && node
            .token(&TokenKind::Ident)
            .is_some_and(|token| token.span() == name)
    {
        return Some(node);
    }
    node.child_nodes()
        .find_map(|child| enclosing_function(child, name))
}

struct Call {
    name: Span,
    method: bool,
    argument: u32,
}

// Unclosed argument lists are recovery regions in the parser. Track delimiters
// on lexer tokens so help survives the first keystroke of a call.
fn call_at(source: &str, cursor: usize) -> Option<Call> {
    use TokenKind::*;
    let tokens: Vec<_> = tokenize_lossless(&source[..cursor])
        .into_iter()
        .filter_map(|token| match token.kind {
            LosslessTokenKind::Token(kind) => Some((kind, token.span)),
            _ => None,
        })
        .collect();
    let mut groups = Vec::new();
    for (index, (kind, _)) in tokens.iter().enumerate() {
        let close = match kind {
            LParen => Some(RParen),
            LBracket => Some(RBracket),
            LBrace => Some(RBrace),
            _ => None,
        };
        if let Some(close) = close {
            groups.push((index, close, 0, false));
        } else if let Some((_, expected, arguments, closure_parameters)) = groups.last_mut() {
            if kind == expected {
                groups.pop();
            } else {
                match kind {
                    Backslash => *closure_parameters = true,
                    ThinArrow => *closure_parameters = false,
                    Comma if !*closure_parameters => *arguments += 1,
                    _ => {}
                }
            }
        }
    }
    for (open, close, argument, _) in groups.into_iter().rev() {
        if close != RParen {
            continue;
        }
        let Some(mut name) = open.checked_sub(1) else {
            continue;
        };
        if tokens[name].0 == RBracket {
            let mut depth = 1;
            while let Some(previous) = name.checked_sub(1) {
                name = previous;
                match tokens[name].0 {
                    RBracket => depth += 1,
                    LBracket => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
            }
            let Some(previous) = name.checked_sub(1) else {
                continue;
            };
            name = previous;
        }
        let (kind, span) = &tokens[name];
        let previous = name.checked_sub(1).map(|index| &tokens[index].0);
        if *kind == Ident && previous != Some(&Fun) {
            return Some(Call {
                name: *span,
                method: previous == Some(&Dot),
                argument,
            });
        }
    }
    None
}
