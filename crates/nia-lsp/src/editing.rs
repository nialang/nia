// SPDX-License-Identifier: GPL-3.0-or-later
//! Completion and signature help for the current source revision.

use std::{collections::BTreeMap, path::Path};

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Documentation, InlayHint,
    InlayHintKind, ParameterInformation, ParameterLabel, Position, Range, SignatureHelp,
    SignatureInformation, TextEdit,
};
use nia_defs::DefKind;
use nia_lexer::{LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_span::Span;
use nia_syntax::{GreenNode, SyntaxKind};

use crate::analysis::{Analysis, File, Symbol};
use crate::workspace::Cancellation;

pub(crate) struct MemberCompletion {
    dot: usize,
    replace: Span,
    cursor: usize,
}

impl MemberCompletion {
    pub(crate) fn at(source: &str, cursor: usize) -> Option<Self> {
        let start = name_start(source, cursor);
        let before = source[..start].trim_end_matches([' ', '\t']);
        let dot = before.strip_suffix('.')?.len();
        if before[..dot].ends_with('.') || inside_literal_or_comment(source, cursor) {
            return None;
        }
        let end = cursor
            + source[cursor..]
                .bytes()
                .take_while(|&byte| is_name_byte(byte))
                .count();
        Some(Self {
            dot,
            replace: Span::new(start, end),
            cursor,
        })
    }

    pub(crate) fn probe(&self, source: &str) -> String {
        let mut text = source.to_owned();
        text.replace_range(self.replace.start..self.replace.end, "__nia_lsp_member");
        text
    }

    pub(crate) fn receiver(&self, file: &File) -> Option<Span> {
        let root = file.syntax().tree.green_root();
        receiver_at_dot(root, self.dot)
    }

    pub(crate) fn prefix<'a>(&self, source: &'a str) -> &'a str {
        &source[self.replace.start..self.cursor]
    }

    pub(crate) fn items(
        &self,
        source: &str,
        lines: &crate::document::Lines,
        mut candidates: Vec<CompletionItem>,
    ) -> Vec<CompletionItem> {
        let prefix = self.prefix(source);
        let range = lines.range(source, self.replace);
        candidates.sort_unstable_by(|left, right| left.label.cmp(&right.label));
        candidates
            .into_iter()
            .filter(|item| item.label.starts_with(prefix))
            .map(|mut item| {
                item.text_edit = Some(CompletionTextEdit::Edit(TextEdit::new(
                    range,
                    item.label.clone(),
                )));
                item
            })
            .collect()
    }
}

fn receiver_at_dot(node: &GreenNode, dot: usize) -> Option<Span> {
    if matches!(
        node.kind(),
        SyntaxKind::FieldExpr | SyntaxKind::TupleFieldExpr
    ) && node
        .token(&TokenKind::Dot)
        .is_some_and(|token| token.span().start == dot)
    {
        return field_receiver(node);
    }
    node.child_nodes()
        .find(|child| child.span().start <= dot && dot < child.span().end)
        .and_then(|child| receiver_at_dot(child, dot))
}

fn field_receiver(node: &GreenNode) -> Option<Span> {
    node.child_nodes()
        .find(|child| child.kind().is_expr())?
        .token_span()
}

pub(crate) fn field_accesses(node: &GreenNode, visit: &mut impl FnMut(Span, Span)) {
    for child in node.child_nodes() {
        field_accesses(child, visit);
    }
    if node.kind() == &SyntaxKind::FieldExpr
        && let Some(name) = node.token(&TokenKind::Ident)
        && let Some(receiver) = field_receiver(node)
    {
        visit(name.span(), receiver);
    }
}

pub(crate) fn call_target(call: &GreenNode) -> Option<(Span, bool)> {
    let mut callee = call.child_nodes().find(|child| child.kind().is_expr())?;
    while callee.kind() == &SyntaxKind::BracketExpr {
        callee = callee.child_nodes().find(|child| child.kind().is_expr())?;
    }
    match callee.kind() {
        SyntaxKind::NameExpr
        | SyntaxKind::FieldExpr
        | SyntaxKind::QualifiedExpr
        | SyntaxKind::OmittedMemberExpr => Some((
            callee.token(&TokenKind::Ident)?.span(),
            callee.kind() == &SyntaxKind::FieldExpr,
        )),
        _ => None,
    }
}

impl Analysis {
    pub(crate) fn inlay_hints(
        &self,
        path: &Path,
        range: Range,
        cancel: &Cancellation,
    ) -> Option<Vec<InlayHint>> {
        let file = self.file(path)?;
        let start = file.lines.offset(&file.text, range.start)?;
        let end = file.lines.offset(&file.text, range.end)?;
        let syntax = file.syntax();
        let mut pending = vec![syntax.tree.green_root()];
        let mut hints = Vec::new();
        while let Some(node) = pending.pop() {
            cancel.check().ok()?;
            let span = node.span();
            if span.end <= start || span.start >= end {
                continue;
            }
            pending.extend(node.child_nodes());
            if !matches!(
                node.kind(),
                SyntaxKind::CallExpr | SyntaxKind::LetStmt | SyntaxKind::ForStmt
            ) || syntax
                .errors
                .iter()
                .any(|error| span.start <= error.span.start && error.span.start < span.end)
            {
                continue;
            }
            if node.kind() != &SyntaxKind::CallExpr {
                if !node.child_nodes().any(|child| child.kind().is_type())
                    && let Some(pattern) =
                        node.child_nodes().find(|child| child.kind().is_pattern())
                {
                    let mut patterns = vec![pattern];
                    while let Some(pattern) = patterns.pop() {
                        cancel.check().ok()?;
                        patterns.extend(pattern.child_nodes());
                        if pattern.kind() == &SyntaxKind::BindPattern
                            && let Some(name) = pattern.token(&TokenKind::Ident)
                            && (start..end).contains(&name.span().end)
                            && let Some(label) = self.local_type_hint(file, name.span())
                        {
                            hints.push(InlayHint {
                                position: file.lines.position(&file.text, name.span().end),
                                label: format!(": {label}").into(),
                                kind: Some(InlayHintKind::TYPE),
                                text_edits: None,
                                tooltip: None,
                                padding_left: None,
                                padding_right: None,
                                data: None,
                            });
                        }
                    }
                }
                continue;
            }
            let Some(arguments) = node.child(&SyntaxKind::ArgList) else {
                continue;
            };
            let Some((callee, method)) = call_target(node) else {
                continue;
            };
            let Some(parameters) = self
                .symbol_at_offset(file, callee.start)
                .and_then(|symbol| self.callable_declaration(symbol))
                .and_then(|callable| callable.function.child(&SyntaxKind::ParamList))
            else {
                continue;
            };
            let parameters = parameters.child_nodes().filter(|param| {
                param.kind() == &SyntaxKind::Param
                    && !(method && param.token(&TokenKind::SelfValue).is_some())
            });
            let arguments = arguments
                .child_nodes()
                .filter(|argument| argument.kind().is_expr());
            hints.extend(
                arguments
                    .zip(parameters)
                    .filter_map(|(argument, parameter)| {
                        let span = argument.token_span()?;
                        let name = parameter.token(&TokenKind::Ident)?.text();
                        if span.start < start
                            || span.start >= end
                            || &file.text[span.start..span.end] == name
                        {
                            return None;
                        }
                        Some(InlayHint {
                            position: file.lines.position(&file.text, span.start),
                            label: format!("{name}:").into(),
                            kind: Some(InlayHintKind::PARAMETER),
                            text_edits: None,
                            tooltip: None,
                            padding_left: None,
                            padding_right: Some(true),
                            data: None,
                        })
                    }),
            );
        }
        hints.sort_by_key(|hint| hint.position);
        Some(hints)
    }

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
        let root = file.syntax().tree.green_root();
        let context = CompletionContext::at(root, start);
        let mut candidates = BTreeMap::new();
        for name in self.names(file, start, &qualifier).ok()? {
            if context.type_only
                && name.kind.is_some_and(|kind| {
                    !matches!(
                        kind,
                        DefKind::Struct
                            | DefKind::Enum
                            | DefKind::Union
                            | DefKind::Trait
                            | DefKind::TypeAlias
                            | DefKind::TraitAssociatedType
                            | DefKind::Module
                    )
                })
            {
                continue;
            }
            candidates.entry(name.name.to_string()).or_insert((
                name.kind.map(completion_kind),
                name.symbol,
                false,
            ));
        }
        if qualifier.is_empty() && context.type_only {
            for name in context.type_parameters {
                candidates.insert(
                    name.to_owned(),
                    (Some(CompletionItemKind::TYPE_PARAMETER), None, true),
                );
            }
        } else if qualifier.is_empty() {
            for name in local_names(root, &file.text, start) {
                candidates.insert(
                    name.to_owned(),
                    (Some(CompletionItemKind::VARIABLE), None, true),
                );
            }
        }
        let range = file.lines.range(&file.text, Span::new(start, end));
        Some(
            candidates
                .into_iter()
                .filter_map(|(name, (kind, symbol, local))| {
                    if !name.starts_with(prefix) {
                        return None;
                    }
                    let sort_text = format!("{}:{name}", u8::from(!local));
                    let mut item = completion_item(name, kind, range);
                    item.sort_text = Some(sort_text);
                    if matches!(
                        kind,
                        Some(CompletionItemKind::FUNCTION | CompletionItemKind::METHOD)
                    ) && let Some(callable) =
                        symbol.and_then(|symbol| self.callable_declaration(symbol))
                    {
                        callable.describe_completion(&mut item);
                    }
                    Some(item)
                })
                .collect(),
        )
    }

    pub(crate) fn signature_help(&self, path: &Path, position: Position) -> Option<SignatureHelp> {
        let file = self.file(path)?;
        let offset = file.lines.offset(&file.text, position)?;
        let call = call_at(&file.text, offset)?;
        let symbol = self.callee(file, &call)?;
        let callable = self.callable_declaration(symbol)?;
        let source = callable.source;
        let parameters = callable.function.child(&SyntaxKind::ParamList)?;
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
        let active = (!info.is_empty()).then(|| call.argument.min(info.len() as u32 - 1));
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: callable.signature.to_owned(),
                documentation: callable.documentation.map(Documentation::String),
                parameters: Some(info),
                active_parameter: active,
            }],
            active_signature: Some(0),
            active_parameter: active,
        })
    }

    pub(crate) fn callable_declaration(&self, symbol: Symbol) -> Option<CallableDeclaration<'_>> {
        let (file, name) = self.declaration_file(symbol)?;
        CallableDeclaration::at(file, name)
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
            && local_names(file.syntax().tree.green_root(), &file.text, call.name.start)
                .contains(&name)
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

#[derive(Default)]
struct CompletionContext<'a> {
    type_only: bool,
    type_parameters: Vec<&'a str>,
}

impl<'a> CompletionContext<'a> {
    fn at(root: &'a GreenNode, offset: usize) -> Self {
        let mut context = Self::default();
        let mut node = Some(root);
        while let Some(current) = node {
            if current.kind() == &SyntaxKind::TypeOrConstArg {
                // A bare argument path can denote a const as well as a type.
                context.type_only = false;
                break;
            }
            if current.kind().is_type() {
                context.type_only = true;
            } else if current.kind().is_expr() {
                context.type_only = false;
            }
            if let Some(generics) = current.child(&SyntaxKind::GenericParamList) {
                context
                    .type_parameters
                    .extend(generics.child_nodes().filter_map(|parameter| {
                        if parameter.kind() != &SyntaxKind::GenericParam
                            || parameter.token(&TokenKind::Colon).is_some()
                        {
                            return None;
                        }
                        let name = parameter.token(&TokenKind::Ident)?;
                        (name.span().end <= offset).then_some(name.text())
                    }));
            }
            node = current.child_nodes().find(|child| {
                let span = child.span();
                span.start <= offset && offset < span.end
            });
        }
        context
    }
}

pub(crate) fn completion_item(
    name: String,
    kind: Option<CompletionItemKind>,
    range: lsp_types::Range,
) -> CompletionItem {
    CompletionItem {
        label: name.clone(),
        kind,
        text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(range, name))),
        ..CompletionItem::default()
    }
}

pub(crate) fn completion_kind(kind: DefKind) -> CompletionItemKind {
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

pub(crate) fn unqualified_completion(
    file: &File,
    position: Position,
) -> Option<(Span, &str, bool)> {
    let offset = file.lines.offset(&file.text, position)?;
    let start = name_start(&file.text, offset);
    let prefix = &file.text[start..offset];
    if prefix.is_empty()
        || qualifier(&file.text, start).1 != start
        || file.text[..start].trim_end().ends_with('.')
        || inside_literal_or_comment(&file.text, offset)
    {
        return None;
    }
    let end = offset
        + file.text[offset..]
            .bytes()
            .take_while(|&byte| is_name_byte(byte))
            .count();
    let root = file.syntax().tree.green_root();
    let span = Span::new(start, end);
    let mut node = root;
    let mut kind = None;
    while let Some(child) = node.child_nodes().find(|child| {
        let inner = child.span();
        inner.start <= start && end <= inner.end
    }) {
        node = child;
        match node.kind() {
            SyntaxKind::NameExpr if node.token_span() == Some(span) => kind = Some(false),
            SyntaxKind::PathType if node.token_span() == Some(span) => kind = Some(true),
            _ => {}
        }
    }
    Some((span, prefix, kind?))
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

pub(crate) struct CallableDeclaration<'a> {
    source: &'a str,
    function: &'a GreenNode,
    member: bool,
    start: usize,
    pub(crate) name: &'a str,
    pub(crate) signature: &'a str,
    pub(crate) documentation: Option<String>,
}

impl<'a> CallableDeclaration<'a> {
    pub(crate) fn at(file: &'a File, declaration: Span) -> Option<Self> {
        let root = file.syntax().tree.green_root();
        Self::in_tree(root, &file.text, declaration)
    }

    pub(crate) fn in_tree(root: &'a GreenNode, source: &'a str, declaration: Span) -> Option<Self> {
        let (function, container) = enclosing_function(root, declaration, None)?;
        let name = function.token(&TokenKind::Ident)?.text();
        let start = function.first_token()?.span().start;
        let declaration_start = container
            .and_then(GreenNode::first_token)
            .map_or(start, |token| token.span().start);
        let end = function
            .child(&SyntaxKind::Block)
            .and_then(GreenNode::first_token)
            .map_or_else(
                || function.last_token().map(|token| token.span().end),
                |token| Some(token.span().start),
            )?;
        Some(Self {
            source,
            function,
            member: container.is_some_and(|node| node.kind() == &SyntaxKind::Member),
            start: declaration_start,
            name,
            signature: source[start..end].trim().trim_end_matches(';'),
            documentation: leading_documentation(source, declaration_start),
        })
    }

    pub(crate) fn hierarchy_item(
        &self,
        lines: &crate::document::Lines,
        uri: &lsp_types::Uri,
        name: Span,
        context: &crate::workspace::CallHierarchyContext,
    ) -> Option<lsp_types::CallHierarchyItem> {
        let span = Span::new(self.start, self.function.token_span()?.end);
        Some(lsp_types::CallHierarchyItem {
            name: self.name.to_owned(),
            kind: if self.member {
                lsp_types::SymbolKind::METHOD
            } else {
                lsp_types::SymbolKind::FUNCTION
            },
            tags: None,
            detail: Some(self.signature.to_owned()),
            uri: uri.clone(),
            range: lines.range(self.source, span),
            selection_range: lines.range(self.source, name),
            data: Some(serde_json::json!(context)),
        })
    }

    pub(crate) fn describe_completion(self, item: &mut CompletionItem) {
        item.detail = Some(self.signature.to_owned());
        item.documentation = self.documentation.map(Documentation::String);
    }
}

fn enclosing_function<'a>(
    node: &'a GreenNode,
    declaration: Span,
    container: Option<&'a GreenNode>,
) -> Option<(&'a GreenNode, Option<&'a GreenNode>)> {
    let span = node.span();
    if declaration.start >= span.end || declaration.end <= span.start {
        return None;
    }
    if *node.kind() == SyntaxKind::FunctionDecl
        && node.token(&TokenKind::Ident).is_some_and(|token| {
            let name = token.span();
            declaration.start <= name.start && name.end <= declaration.end
        })
    {
        return Some((node, container));
    }
    let container = if matches!(node.kind(), SyntaxKind::Item | SyntaxKind::Member) {
        Some(node)
    } else {
        container
    };
    node.child_nodes()
        .find_map(|child| enclosing_function(child, declaration, container))
}

fn leading_documentation(source: &str, start: usize) -> Option<String> {
    let mut preceding = source[..start].rsplit('\n');
    if !preceding.next()?.trim().is_empty() {
        return None;
    }
    let mut lines: Vec<_> = preceding
        .map_while(|line| {
            let doc = line
                .trim_end_matches('\r')
                .trim_start()
                .strip_prefix("///")?;
            Some(doc.strip_prefix(' ').unwrap_or(doc))
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    Some(lines.join("\n"))
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
