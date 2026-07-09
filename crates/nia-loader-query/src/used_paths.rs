use nia_ast::{
    Expr, ExprKind, Item, ItemKind, PathSegmentKind, Stmt, StmtKind, TypeKind, TypePathSegment,
    TypeRef, UsingGroupItem, UsingHostSegment, UsingItem, UsingSelector,
};
use nia_ast_walk::{Visitor, walk_expr, walk_item, walk_module, walk_stmt, walk_type};
use nia_diagnostic::{Diagnostic, codes};
use nia_imports::{ModuleMap, ModuleRootSegment, ResolvedModuleDeclaration, Visibility};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
use nia_symbol::{SymbolId, SymbolMap, ToSymbolId};

pub(crate) fn collect_used_modules(
    item_tree: &ActiveModuleItemTree,
    module_map: &ModuleMap,
) -> UsedModuleCollection {
    let mut packages = Vec::new();
    let mut paths = Vec::new();
    let local_module_names = item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Module(module) => Some(module.name),
            _ => None,
        })
        .collect::<Vec<_>>();
    let using_aliases = module_using_aliases(item_tree, module_map, &local_module_names);
    let explicit_imports =
        module_explicit_imports(item_tree, module_map, &local_module_names, &using_aliases);
    let mut used_aliases = Vec::new();
    for item in &item_tree.items {
        let ItemTreeNodeKind::Using(using) = &item.kind else {
            continue;
        };
        collect_using_modules(
            &using.host,
            &using.selector,
            module_map,
            &local_module_names,
            &using_aliases,
            &mut packages,
            &mut paths,
        );
    }
    let module = item_tree.to_module();
    let mut collector = QualifiedPathModuleCollector {
        module_map,
        local_module_names: &local_module_names,
        using_aliases: &using_aliases,
        used_aliases: &mut used_aliases,
        packages: &mut packages,
        paths: &mut paths,
        locals: Vec::new(),
    };
    walk_module(&mut collector, &module);
    packages.sort();
    packages.dedup();
    paths.sort();
    paths.dedup();
    used_aliases.sort();
    used_aliases.dedup();
    for path in &paths {
        if let UsedModulePath::Package { package, .. } = path {
            packages.push(*package);
        }
    }
    packages.sort();
    packages.dedup();
    UsedModuleCollection {
        package_roots: packages,
        used_module_paths: paths,
        explicit_imports,
        used_aliases,
    }
}

struct QualifiedPathModuleCollector<'a> {
    module_map: &'a ModuleMap,
    local_module_names: &'a [SymbolId],
    using_aliases: &'a SymbolMap<UsedModulePath>,
    used_aliases: &'a mut Vec<SymbolId>,
    packages: &'a mut Vec<SymbolId>,
    paths: &'a mut Vec<UsedModulePath>,
    locals: Vec<SymbolMap<SymbolId>>,
}

impl QualifiedPathModuleCollector<'_> {
    fn collect_using(&mut self, using: &UsingItem) {
        collect_using_modules(
            &using.host,
            &using.selector,
            self.module_map,
            self.local_module_names,
            self.using_aliases,
            self.packages,
            self.paths,
        );
    }

    fn collect_path_segments(&mut self, segments: Vec<SymbolId>) {
        self.collect_path_segments_with_processing(
            segments,
            UsedModulePathProcessing::IfSelectedItem,
        );
    }

    fn collect_path_segments_with_processing(
        &mut self,
        segments: Vec<SymbolId>,
        processing: UsedModulePathProcessing,
    ) {
        let Some((first, rest)) = segments.split_first() else {
            return;
        };
        if let Some(alias) = self.using_aliases.get(first) {
            self.used_aliases.push(*first);
            self.paths
                .push(alias.with_appended_segments_with_processing_mode(rest, false, processing));
            return;
        }
        if nia_imports::is_entry_module_root(*first) {
            return;
        }
        if !self.local_module_names.contains(first) && self.module_map.contains_root(*first) {
            self.packages.push(*first);
            self.paths.push(UsedModulePath::Package {
                package: *first,
                segments: rest.to_vec(),
                include_declared_children: false,
                processing: if processing == UsedModulePathProcessing::IfSelectedItem {
                    UsedModulePathProcessing::Always
                } else {
                    processing
                },
            });
        }
    }

    fn collect_trait_provider_for_type(&mut self, ty: &TypeRef) {
        let TypeKind::Path { segments } = &ty.kind else {
            return;
        };
        let Some(last) = segments.last() else {
            return;
        };
        let Some(segments) = type_path_names(segments) else {
            return;
        };
        let Some(trait_name) = type_path_segment_name(last) else {
            return;
        };
        self.collect_path_segments_with_processing(
            segments,
            UsedModulePathProcessing::IfProvidesTraitImpl { trait_name },
        );
    }

    fn collect_trait_method_provider(
        &mut self,
        target_type_name: Option<SymbolId>,
        name: &SymbolId,
    ) {
        for alias in self.using_aliases.values() {
            self.paths
                .push(alias.with_appended_segments_with_processing_mode(
                    &[],
                    false,
                    UsedModulePathProcessing::IfProvidesTraitMethod {
                        target_type_name,
                        associated_name: *name,
                    },
                ));
        }
    }

    fn collect_implicit_trait_provider(&mut self, trait_name: SymbolId) {
        for alias in self.using_aliases.values() {
            self.paths
                .push(alias.with_appended_segments_with_processing_mode(
                    &[],
                    false,
                    UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name },
                ));
        }
    }

    fn collect_inherent_provider_for_type(&mut self, target: &TypeRef, associated_name: &SymbolId) {
        let TypeKind::Path { segments } = &target.kind else {
            return;
        };
        let Some(last) = segments.last() else {
            return;
        };
        let Some(segments) = type_path_names(segments) else {
            return;
        };
        let Some(target_type_name) = type_path_segment_name(last) else {
            return;
        };
        self.collect_path_segments_with_processing(
            segments,
            UsedModulePathProcessing::IfProvidesInherentAssociated {
                target_type_name,
                associated_name: *associated_name,
            },
        );
    }
}

pub(crate) fn module_using_aliases(
    item_tree: &ActiveModuleItemTree,
    module_map: &ModuleMap,
    local_module_names: &[SymbolId],
) -> SymbolMap<UsedModulePath> {
    let mut aliases: SymbolMap<UsedModulePath> = SymbolMap::default();
    let mut packages = Vec::new();
    for item in &item_tree.items {
        let ItemTreeNodeKind::Using(using) = &item.kind else {
            continue;
        };
        if !using.host.is_empty()
            && let Some((first, rest)) = using.host.split_first()
            && let Some(first_name) = using_host_segment_name(first)
            && let Some(alias) = aliases.get(&first_name).cloned()
        {
            let root =
                alias.with_appended_segments_with_processing(&host_segments(rest), false, false);
            collect_selector_aliases_from_path(root, &using.selector, &mut aliases);
            continue;
        }
        collect_using_aliases(
            &using.host,
            &using.selector,
            module_map,
            local_module_names,
            &mut packages,
            &mut aliases,
        );
    }
    aliases
}

impl<'ast> Visitor<'ast> for QualifiedPathModuleCollector<'_> {
    fn visit_function(&mut self, function: &'ast nia_ast::FunctionItem) {
        self.visit_function_with_optional_body(function, true);
    }

    fn visit_block(&mut self, block: &'ast nia_ast::Block) {
        self.locals.push(SymbolMap::default());
        nia_ast_walk::walk_block(self, block);
        self.locals.pop();
    }

    fn visit_item(&mut self, item: &'ast Item) {
        let ItemKind::Extend(extend) = &item.kind else {
            walk_item(self, item);
            return;
        };
        self.visit_type(&extend.target);
        if let Some(trait_ref) = &extend.trait_ref {
            self.visit_type(trait_ref);
        }
        nia_ast_walk::walk_where_clause(self, &extend.where_clause);
        for associated_type in &extend.associated_types {
            self.visit_type(&associated_type.ty);
        }
        for associated_value in &extend.associated_values {
            if let Some(ty) = &associated_value.binding.ty {
                self.visit_type(ty);
            }
            if let Some(value) = &associated_value.binding.value {
                self.visit_expr(value);
            }
        }
        for method in &extend.methods {
            self.visit_extend_method(extend, &method.function);
        }
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let StmtKind::Using(using) = &stmt.kind {
            self.collect_using(using);
        }
        if let StmtKind::Binding(binding) = &stmt.kind {
            if let Some(ty) = &binding.ty {
                self.visit_type(ty);
                self.record_pattern_type(&binding.pattern, ty);
            }
            if let Some(value) = &binding.value {
                self.visit_expr(value);
            }
            return;
        }
        if let StmtKind::ForIn(_) = &stmt.kind {
            self.collect_implicit_trait_provider(nia_ids::BuiltinTrait::Iterable.symbol_id());
            walk_stmt(self, stmt);
            return;
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let ExprKind::Call { callee, args } = &expr.kind {
            if let ExprKind::Field { name, .. } = &callee.kind {
                let target_type_name = match method_receiver_local_type_name(callee) {
                    Some(MethodReceiverName::Local(local_name)) => self.local_type_name(local_name),
                    Some(MethodReceiverName::SelfValue) | None => None,
                };
                self.collect_trait_method_provider(target_type_name, name);
            }
            if let Some(segments) = expr_qualified_segments(callee) {
                self.collect_path_segments(segments);
                for arg in args {
                    self.visit_expr(arg);
                }
                return;
            }
        }
        if let Some(segments) = expr_qualified_segments(expr) {
            self.collect_path_segments(segments);
            return;
        }
        walk_expr(self, expr);
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        if let TypeKind::Path { segments } = &ty.kind {
            self.collect_path_segments(
                segments
                    .iter()
                    .filter_map(type_path_segment_name)
                    .collect::<Vec<_>>(),
            );
            for segment in segments {
                for arg in &segment.args {
                    match arg {
                        nia_ast::TypeArg::Type(ty)
                        | nia_ast::TypeArg::AssocBinding { ty, .. }
                        | nia_ast::TypeArg::TypeOrConst { ty, .. } => {
                            self.collect_trait_provider_for_type(ty);
                        }
                        nia_ast::TypeArg::Const(_) => {}
                    }
                }
            }
        }
        walk_type(self, ty);
    }
}

impl QualifiedPathModuleCollector<'_> {
    fn visit_function_with_optional_body(
        &mut self,
        function: &nia_ast::FunctionItem,
        visit_body: bool,
    ) {
        self.locals.push(SymbolMap::default());
        self.visit_function_signature(function);
        if visit_body && let Some(body) = &function.body {
            self.visit_block(body);
        }
        self.locals.pop();
    }

    fn visit_extend_method(
        &mut self,
        extend: &nia_ast::ExtendItem,
        function: &nia_ast::FunctionItem,
    ) {
        self.locals.push(SymbolMap::default());
        self.visit_function_signature(function);
        if let Some(body) = &function.body {
            let mut collector = ExtendSelfMethodCollector {
                target: &extend.target,
                module_collector: self,
            };
            collector.visit_block(body);
        }
        self.locals.pop();
    }

    fn visit_function_signature(&mut self, function: &nia_ast::FunctionItem) {
        nia_ast_walk::walk_where_clause(self, &function.where_clause);
        for param in &function.params {
            if let Some(ty) = &param.ty {
                self.visit_type(ty);
                if let Some(name) = &param.name {
                    self.record_local_type(name, ty);
                }
            }
        }
        if let Some(return_type) = &function.return_type {
            self.visit_type(return_type);
        }
    }

    fn record_pattern_type(&mut self, pattern: &nia_ast::Pattern, ty: &TypeRef) {
        if let nia_ast::PatternKind::Bind { name, .. } = &pattern.kind {
            self.record_local_type(name, ty);
        }
    }

    fn record_local_type(&mut self, name: &SymbolId, ty: &TypeRef) {
        let Some(type_name) = type_ref_last_name(ty) else {
            return;
        };
        if let Some(scope) = self.locals.last_mut() {
            scope.insert(*name, type_name);
        }
    }

    fn local_type_name(&self, name: &SymbolId) -> Option<SymbolId> {
        self.locals
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MethodReceiverName<'a> {
    Local(&'a SymbolId),
    SelfValue,
}

fn method_receiver_local_type_name(callee: &Expr) -> Option<MethodReceiverName<'_>> {
    let ExprKind::Field { lhs, .. } = &callee.kind else {
        return None;
    };
    match &lhs.kind {
        ExprKind::Ident(name) => Some(MethodReceiverName::Local(name)),
        ExprKind::SelfValue => Some(MethodReceiverName::SelfValue),
        _ => None,
    }
}

struct ExtendSelfMethodCollector<'a, 'b> {
    target: &'a TypeRef,
    module_collector: &'a mut QualifiedPathModuleCollector<'b>,
}

impl<'ast> Visitor<'ast> for ExtendSelfMethodCollector<'_, '_> {
    fn visit_block(&mut self, block: &'ast nia_ast::Block) {
        self.module_collector.locals.push(SymbolMap::default());
        nia_ast_walk::walk_block(self, block);
        self.module_collector.locals.pop();
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let ExprKind::Field { lhs, name } = &expr.kind
            && matches!(&lhs.kind, ExprKind::SelfValue)
        {
            self.module_collector
                .collect_inherent_provider_for_type(self.target, name);
        }
        if let ExprKind::Call { callee, args } = &expr.kind {
            if let ExprKind::Field { name, .. } = &callee.kind {
                let target_type_name = if matches!(
                    method_receiver_local_type_name(callee),
                    Some(MethodReceiverName::SelfValue)
                ) {
                    type_ref_last_name(self.target)
                } else {
                    match method_receiver_local_type_name(callee) {
                        Some(MethodReceiverName::Local(local_name)) => {
                            self.module_collector.local_type_name(local_name)
                        }
                        Some(MethodReceiverName::SelfValue) | None => None,
                    }
                };
                self.module_collector
                    .collect_trait_method_provider(target_type_name, name);
            }
            if let Some(segments) = expr_qualified_segments(callee) {
                self.module_collector.collect_path_segments(segments);
                for arg in args {
                    self.visit_expr(arg);
                }
                return;
            }
        }
        if let Some(segments) = expr_qualified_segments(expr) {
            self.module_collector.collect_path_segments(segments);
            return;
        }
        walk_expr(self, expr);
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        self.module_collector.visit_type(ty);
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let StmtKind::Using(using) = &stmt.kind {
            self.module_collector.collect_using(using);
        }
        if let StmtKind::Binding(binding) = &stmt.kind {
            if let Some(ty) = &binding.ty {
                self.module_collector.visit_type(ty);
                self.module_collector
                    .record_pattern_type(&binding.pattern, ty);
            }
            if let Some(value) = &binding.value {
                self.visit_expr(value);
            }
            return;
        }
        if let StmtKind::ForIn(_) = &stmt.kind {
            self.module_collector
                .collect_implicit_trait_provider(nia_ids::BuiltinTrait::Iterable.symbol_id());
            walk_stmt(self, stmt);
            return;
        }
        walk_stmt(self, stmt);
    }
}

fn expr_qualified_segments(expr: &Expr) -> Option<Vec<SymbolId>> {
    fn collect(expr: &Expr, segments: &mut Vec<SymbolId>) -> Option<()> {
        match &expr.kind {
            ExprKind::Ident(name) => {
                segments.push(name.clone());
                Some(())
            }
            ExprKind::Qualified { lhs, name } => {
                collect(lhs, segments)?;
                segments.push(name.clone());
                Some(())
            }
            _ => None,
        }
    }

    let mut segments = Vec::new();
    collect(expr, &mut segments)?;
    Some(segments)
}

fn collect_using_modules(
    host: &[nia_ast::UsingHostSegment],
    selector: &UsingSelector,
    module_map: &ModuleMap,
    local_module_names: &[SymbolId],
    aliases: &SymbolMap<UsedModulePath>,
    packages: &mut Vec<SymbolId>,
    paths: &mut Vec<UsedModulePath>,
) {
    if host.is_empty() {
        collect_root_group_modules(selector, module_map, local_module_names, packages, paths);
        return;
    }
    if let Some((first, rest)) = host.split_first()
        && let Some(first_name) = using_host_segment_name(first)
        && let Some(alias) = aliases.get(&first_name)
    {
        let host_path =
            alias.with_appended_segments_with_processing(&host_segments(rest), false, false);
        collect_selector_modules_from_path(host_path, selector, paths);
        return;
    }
    let Some(root) = UsedModuleRoot::from_host(host, module_map, local_module_names, packages)
    else {
        return;
    };
    collect_selector_modules(root, selector, paths);
}

fn collect_using_aliases(
    host: &[nia_ast::UsingHostSegment],
    selector: &UsingSelector,
    module_map: &ModuleMap,
    local_module_names: &[SymbolId],
    packages: &mut Vec<SymbolId>,
    aliases: &mut SymbolMap<UsedModulePath>,
) {
    if host.is_empty() {
        return;
    }
    let Some(root) = UsedModuleRoot::from_host(host, module_map, local_module_names, packages)
    else {
        return;
    };
    collect_selector_aliases(root, selector, aliases);
}

fn collect_selector_aliases(
    used_root: UsedModuleRoot,
    selector: &UsingSelector,
    aliases: &mut SymbolMap<UsedModulePath>,
) {
    match selector {
        UsingSelector::SelfName => {
            if let Some(name) = used_root.last_segment_name() {
                insert_using_alias(aliases, name, used_root.path(&[], false, false));
            }
        }
        UsingSelector::Wildcard { .. } => {}
        UsingSelector::Single(name) => {
            insert_using_alias(
                aliases,
                name.alias.unwrap_or(name.name),
                used_root.path(std::slice::from_ref(&name.name), false, false),
            );
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_group_item_aliases(&used_root, item, aliases);
            }
        }
    }
}

fn collect_selector_aliases_from_path(
    host_path: UsedModulePath,
    selector: &UsingSelector,
    aliases: &mut SymbolMap<UsedModulePath>,
) {
    match selector {
        UsingSelector::SelfName => {
            if let Some(name) = host_path.last_segment_name() {
                insert_using_alias(aliases, name, host_path);
            }
        }
        UsingSelector::Wildcard { .. } => {}
        UsingSelector::Single(name) => {
            insert_using_alias(
                aliases,
                name.alias.unwrap_or(name.name),
                host_path.with_appended_segments_with_processing(
                    std::slice::from_ref(&name.name),
                    false,
                    false,
                ),
            );
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_group_item_aliases_from_path(&host_path, item, aliases);
            }
        }
    }
}

fn collect_group_item_aliases(
    root: &UsedModuleRoot,
    item: &UsingGroupItem,
    aliases: &mut SymbolMap<UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            insert_using_alias(
                aliases,
                name.alias.unwrap_or(name.name),
                root.path(std::slice::from_ref(&name.name), false, false),
            );
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested_root = root_with_extra(root, &host_segments(host));
            collect_selector_aliases(nested_root, selector, aliases);
        }
    }
}

fn collect_group_item_aliases_from_path(
    root: &UsedModulePath,
    item: &UsingGroupItem,
    aliases: &mut SymbolMap<UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            insert_using_alias(
                aliases,
                name.alias.unwrap_or(name.name),
                root.with_appended_segments_with_processing(
                    std::slice::from_ref(&name.name),
                    false,
                    false,
                ),
            );
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested =
                root.with_appended_segments_with_processing(&host_segments(host), false, false);
            collect_selector_aliases_from_path(nested, selector, aliases);
        }
    }
}

fn insert_using_alias(
    aliases: &mut SymbolMap<UsedModulePath>,
    name: SymbolId,
    path: UsedModulePath,
) {
    aliases.entry(name).or_insert(path);
}

pub(crate) fn using_host_path(
    host: &[nia_ast::UsingHostSegment],
    module_map: &ModuleMap,
    local_module_names: &[SymbolId],
    aliases: &SymbolMap<UsedModulePath>,
) -> Option<UsedModulePath> {
    let first = host.first()?;
    if let Some(first_name) = using_host_segment_name(first)
        && let Some(alias) = aliases.get(&first_name)
    {
        return Some(alias.with_appended_segments_with_processing_mode(
            &host_segments(&host[1..]),
            false,
            UsedModulePathProcessing::IfSelectedItem,
        ));
    }
    let mut packages = Vec::new();
    let root = UsedModuleRoot::from_host(host, module_map, local_module_names, &mut packages)?;
    Some(root.path(&[], false, true))
}

fn module_explicit_imports(
    item_tree: &ActiveModuleItemTree,
    module_map: &ModuleMap,
    local_module_names: &[SymbolId],
    aliases: &SymbolMap<UsedModulePath>,
) -> Vec<ExplicitUsingImport> {
    let mut imports = Vec::new();
    for item in &item_tree.items {
        let ItemTreeNodeKind::Using(using) = &item.kind else {
            continue;
        };
        if item.visibility != Visibility::Private {
            continue;
        }
        collect_explicit_imports_from_using(
            item.span,
            using,
            module_map,
            local_module_names,
            aliases,
            &mut imports,
        );
    }
    imports
}

fn collect_explicit_imports_from_using(
    span: nia_span::Span,
    using: &UsingItem,
    module_map: &ModuleMap,
    local_module_names: &[SymbolId],
    aliases: &SymbolMap<UsedModulePath>,
    imports: &mut Vec<ExplicitUsingImport>,
) {
    if using.host.is_empty() {
        let UsingSelector::Group(items) = &using.selector else {
            return;
        };
        for item in items {
            collect_explicit_imports_from_root_group_item(
                span,
                item,
                module_map,
                local_module_names,
                imports,
            );
        }
        return;
    }
    let Some(host_path) = using_host_path(&using.host, module_map, local_module_names, aliases)
    else {
        return;
    };
    collect_explicit_imports_from_selector(span, host_path, &using.selector, imports);
}

fn collect_explicit_imports_from_root_group_item(
    span: nia_span::Span,
    item: &UsingGroupItem,
    module_map: &ModuleMap,
    local_module_names: &[SymbolId],
    imports: &mut Vec<ExplicitUsingImport>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            if !nia_imports::is_entry_module_root(name.name)
                && !local_module_names.contains(&name.name)
                && module_map.contains_root(name.name)
            {
                imports.push(ExplicitUsingImport {
                    span,
                    alias: name.alias.unwrap_or(name.name),
                    path: UsedModulePath::Package {
                        package: name.name,
                        segments: Vec::new(),
                        include_declared_children: false,
                        processing: UsedModulePathProcessing::Never,
                    },
                });
            }
        }
        UsingGroupItem::Nested { host, selector } => {
            let aliases = SymbolMap::default();
            let Some(host_path) = using_host_path(host, module_map, local_module_names, &aliases)
            else {
                return;
            };
            collect_explicit_imports_from_selector(span, host_path, selector, imports);
        }
    }
}

fn collect_explicit_imports_from_selector(
    span: nia_span::Span,
    host_path: UsedModulePath,
    selector: &UsingSelector,
    imports: &mut Vec<ExplicitUsingImport>,
) {
    match selector {
        UsingSelector::SelfName => {
            if let Some(alias) = host_path.last_segment_name() {
                imports.push(ExplicitUsingImport {
                    span,
                    alias,
                    path: host_path,
                });
            }
        }
        UsingSelector::Wildcard { .. } => {}
        UsingSelector::Single(name) => {
            imports.push(ExplicitUsingImport {
                span,
                alias: name.alias.unwrap_or(name.name),
                path: host_path.with_appended_segments_with_processing_mode(
                    std::slice::from_ref(&name.name),
                    false,
                    UsedModulePathProcessing::Never,
                ),
            });
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_explicit_imports_from_group_item(span, &host_path, item, imports);
            }
        }
    }
}

fn collect_explicit_imports_from_group_item(
    span: nia_span::Span,
    host_path: &UsedModulePath,
    item: &UsingGroupItem,
    imports: &mut Vec<ExplicitUsingImport>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            imports.push(ExplicitUsingImport {
                span,
                alias: name.alias.unwrap_or(name.name),
                path: host_path.with_appended_segments_with_processing_mode(
                    std::slice::from_ref(&name.name),
                    false,
                    UsedModulePathProcessing::Never,
                ),
            });
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested = host_path.with_appended_segments_with_processing(
                &host_segments(host),
                false,
                false,
            );
            collect_explicit_imports_from_selector(span, nested, selector, imports);
        }
    }
}

fn collect_root_group_modules(
    selector: &UsingSelector,
    module_map: &ModuleMap,
    local_module_names: &[SymbolId],
    packages: &mut Vec<SymbolId>,
    paths: &mut Vec<UsedModulePath>,
) {
    let UsingSelector::Group(items) = selector else {
        return;
    };
    for item in items {
        match item {
            UsingGroupItem::Name(name) => {
                if !nia_imports::is_entry_module_root(name.name)
                    && !local_module_names.contains(&name.name)
                    && module_map.contains_root(name.name)
                {
                    packages.push(name.name);
                    paths.push(UsedModulePath::Package {
                        package: name.name,
                        segments: Vec::new(),
                        include_declared_children: false,
                        processing: UsedModulePathProcessing::Never,
                    });
                }
            }
            UsingGroupItem::Nested { host, selector } => {
                collect_using_modules(
                    host,
                    selector,
                    module_map,
                    local_module_names,
                    &SymbolMap::default(),
                    packages,
                    paths,
                );
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModuleDeclarations {
    pub(crate) declarations: Vec<ResolvedModuleDeclaration>,
    pub(crate) package_roots: Vec<SymbolId>,
    pub(crate) used_module_paths: Vec<UsedModulePath>,
    pub(crate) explicit_imports: Vec<ExplicitUsingImport>,
    pub(crate) used_import_aliases: Vec<SymbolId>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsedModuleCollection {
    pub(crate) package_roots: Vec<SymbolId>,
    pub(crate) used_module_paths: Vec<UsedModulePath>,
    pub(crate) explicit_imports: Vec<ExplicitUsingImport>,
    pub(crate) used_aliases: Vec<SymbolId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExplicitUsingImport {
    pub(crate) span: nia_span::Span,
    pub(crate) alias: SymbolId,
    pub(crate) path: UsedModulePath,
}

impl ExplicitUsingImport {
    pub(crate) fn warning(&self, symbols: &dyn nia_symbol::SymbolText) -> Diagnostic {
        Diagnostic::user_warning_at(
            codes::UNUSED_IMPORT,
            self.span,
            format!(
                "unused import `{}`",
                nia_symbol::symbol_text_or_unresolved(symbols, self.alias)
            ),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum UsedModulePath {
    Package {
        package: SymbolId,
        segments: Vec<SymbolId>,
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    },
    PackageRelative {
        segments: Vec<SymbolId>,
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    },
    ParentRelative {
        segments: Vec<SymbolId>,
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    },
    Local {
        segments: Vec<SymbolId>,
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    },
}

impl UsedModulePath {
    pub(crate) fn with_appended_segments(
        &self,
        extra: &[SymbolId],
        include_declared_children: bool,
    ) -> Self {
        self.with_appended_segments_with_processing(extra, include_declared_children, true)
    }

    pub(crate) fn with_appended_segments_with_processing(
        &self,
        extra: &[SymbolId],
        include_declared_children: bool,
        process_used_paths: bool,
    ) -> Self {
        self.with_appended_segments_with_processing_mode(
            extra,
            include_declared_children,
            UsedModulePathProcessing::from_bool(process_used_paths),
        )
    }

    pub(crate) fn with_appended_segments_with_processing_mode(
        &self,
        extra: &[SymbolId],
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    ) -> Self {
        match self {
            UsedModulePath::Package {
                package, segments, ..
            } => UsedModulePath::Package {
                package: *package,
                segments: joined_segments(segments, extra),
                include_declared_children,
                processing,
            },
            UsedModulePath::PackageRelative { segments, .. } => UsedModulePath::PackageRelative {
                segments: joined_segments(segments, extra),
                include_declared_children,
                processing,
            },
            UsedModulePath::ParentRelative { segments, .. } => UsedModulePath::ParentRelative {
                segments: joined_segments(segments, extra),
                include_declared_children,
                processing,
            },
            UsedModulePath::Local { segments, .. } => UsedModulePath::Local {
                segments: joined_segments(segments, extra),
                include_declared_children,
                processing,
            },
        }
    }

    pub(crate) fn with_declared_children_and_processing(
        &self,
        include_declared_children: bool,
        process_used_paths: bool,
    ) -> Self {
        self.with_appended_segments_with_processing(
            &[],
            include_declared_children,
            process_used_paths,
        )
    }

    pub(crate) fn segments(&self) -> &[SymbolId] {
        match self {
            UsedModulePath::Package { segments, .. }
            | UsedModulePath::PackageRelative { segments, .. }
            | UsedModulePath::ParentRelative { segments, .. }
            | UsedModulePath::Local { segments, .. } => segments,
        }
    }

    pub(crate) fn include_declared_children(&self) -> bool {
        match self {
            UsedModulePath::Package {
                include_declared_children,
                ..
            }
            | UsedModulePath::PackageRelative {
                include_declared_children,
                ..
            }
            | UsedModulePath::ParentRelative {
                include_declared_children,
                ..
            }
            | UsedModulePath::Local {
                include_declared_children,
                ..
            } => *include_declared_children,
        }
    }

    pub(crate) fn processing(&self) -> UsedModulePathProcessing {
        match self {
            UsedModulePath::Package { processing, .. }
            | UsedModulePath::PackageRelative { processing, .. }
            | UsedModulePath::ParentRelative { processing, .. }
            | UsedModulePath::Local { processing, .. } => processing.clone(),
        }
    }

    pub(crate) fn last_segment_name(&self) -> Option<SymbolId> {
        match self {
            UsedModulePath::Package {
                package, segments, ..
            } => segments.last().cloned().or_else(|| Some(*package)),
            UsedModulePath::PackageRelative { segments, .. }
            | UsedModulePath::ParentRelative { segments, .. }
            | UsedModulePath::Local { segments, .. } => segments.last().cloned(),
        }
    }

    pub(crate) fn activates_package_facade(&self) -> Option<SymbolId> {
        match self {
            UsedModulePath::Package {
                package,
                segments,
                include_declared_children,
                ..
            } if segments.is_empty() && *include_declared_children => Some(*package),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum UsedModulePathProcessing {
    Never,
    Always,
    IfSelectedItem,
    IfProvidesExtensions,
    IfProvidesTraitImpl {
        trait_name: SymbolId,
    },
    IfProvidesImplicitTraitImpl {
        trait_name: SymbolId,
    },
    IfProvidesTraitMethod {
        target_type_name: Option<SymbolId>,
        associated_name: SymbolId,
    },
    IfProvidesInherentAssociated {
        target_type_name: SymbolId,
        associated_name: SymbolId,
    },
}

impl UsedModulePathProcessing {
    fn from_bool(process_used_paths: bool) -> Self {
        if process_used_paths {
            Self::Always
        } else {
            Self::Never
        }
    }

    pub(crate) fn is_replayable_provider_request(&self) -> bool {
        matches!(
            self,
            Self::IfProvidesImplicitTraitImpl { .. }
                | Self::IfProvidesTraitMethod {
                    target_type_name: None,
                    ..
                }
        )
    }

    pub(crate) fn should_process_module(self) -> bool {
        matches!(self, Self::Always | Self::IfSelectedItem)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UsedModuleRoot {
    Package {
        package: SymbolId,
        base: Vec<SymbolId>,
    },
    PackageRelative {
        base: Vec<SymbolId>,
    },
    ParentRelative {
        base: Vec<SymbolId>,
    },
    Local {
        base: Vec<SymbolId>,
    },
}

impl UsedModuleRoot {
    fn from_host(
        host: &[UsingHostSegment],
        module_map: &ModuleMap,
        local_module_names: &[SymbolId],
        packages: &mut Vec<SymbolId>,
    ) -> Option<Self> {
        let first = host.first()?;
        match module_root_segment_from_path_segment(first.kind) {
            ModuleRootSegment::Current => {
                return Some(Self::Local {
                    base: host_segments(&host[1..]),
                });
            }
            ModuleRootSegment::Parent => {
                return Some(Self::ParentRelative {
                    base: host_segments(&host[1..]),
                });
            }
            ModuleRootSegment::PackageRelative => {
                return Some(Self::PackageRelative {
                    base: host_segments(&host[1..]),
                });
            }
            ModuleRootSegment::Named(name) if nia_imports::is_entry_module_root(name) => {
                return Some(Self::Package {
                    package: name,
                    base: host_segments(&host[1..]),
                });
            }
            ModuleRootSegment::Named(name) => {
                if local_module_names.contains(&name) {
                    return Some(Self::Local {
                        base: host_segments(host),
                    });
                }
                if module_map.contains_root(name) {
                    packages.push(name);
                    return Some(Self::Package {
                        package: name,
                        base: host_segments(&host[1..]),
                    });
                }
            }
        }
        Some(Self::Local {
            base: host_segments(host),
        })
    }

    fn path(
        &self,
        extra: &[SymbolId],
        include_declared_children: bool,
        process_used_paths: bool,
    ) -> UsedModulePath {
        self.path_with_processing_mode(
            extra,
            include_declared_children,
            UsedModulePathProcessing::from_bool(process_used_paths),
        )
    }

    fn path_with_processing_mode(
        &self,
        extra: &[SymbolId],
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    ) -> UsedModulePath {
        match self {
            UsedModuleRoot::Package { package, base } => UsedModulePath::Package {
                package: *package,
                segments: joined_segments(base, extra),
                include_declared_children,
                processing,
            },
            UsedModuleRoot::PackageRelative { base } => UsedModulePath::PackageRelative {
                segments: joined_segments(base, extra),
                include_declared_children,
                processing,
            },
            UsedModuleRoot::ParentRelative { base } => UsedModulePath::ParentRelative {
                segments: joined_segments(base, extra),
                include_declared_children,
                processing,
            },
            UsedModuleRoot::Local { base } => UsedModulePath::Local {
                segments: joined_segments(base, extra),
                include_declared_children,
                processing,
            },
        }
    }

    fn last_segment_name(&self) -> Option<SymbolId> {
        match self {
            UsedModuleRoot::Package { package, base } => {
                Some(base.last().copied().unwrap_or(*package))
            }
            UsedModuleRoot::PackageRelative { base }
            | UsedModuleRoot::ParentRelative { base }
            | UsedModuleRoot::Local { base } => base.last().cloned(),
        }
    }
}

fn module_root_segment_from_path_segment(kind: PathSegmentKind) -> ModuleRootSegment {
    match kind {
        PathSegmentKind::SelfValue => ModuleRootSegment::Current,
        PathSegmentKind::Super => ModuleRootSegment::Parent,
        PathSegmentKind::Package => ModuleRootSegment::PackageRelative,
        PathSegmentKind::Name(name) => ModuleRootSegment::Named(name),
    }
}

fn collect_selector_modules(
    used_root: UsedModuleRoot,
    selector: &UsingSelector,
    paths: &mut Vec<UsedModulePath>,
) {
    match selector {
        UsingSelector::SelfName => {
            paths.push(used_root.path_with_processing_mode(
                &[],
                false,
                UsedModulePathProcessing::IfProvidesExtensions,
            ));
        }
        UsingSelector::Wildcard { .. } => {
            paths.push(used_root.path(&[], true, true));
        }
        UsingSelector::Single(name) => {
            paths.push(used_root.path(std::slice::from_ref(&name.name), false, false));
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_group_item_modules(&used_root, item, paths);
            }
        }
    }
}

fn collect_selector_modules_from_path(
    host_path: UsedModulePath,
    selector: &UsingSelector,
    paths: &mut Vec<UsedModulePath>,
) {
    match selector {
        UsingSelector::SelfName => {
            paths.push(host_path.with_appended_segments_with_processing_mode(
                &[],
                false,
                UsedModulePathProcessing::IfProvidesExtensions,
            ));
        }
        UsingSelector::Wildcard { .. } => {
            paths.push(host_path.with_declared_children_and_processing(true, true));
        }
        UsingSelector::Single(name) => {
            paths.push(host_path.with_appended_segments_with_processing_mode(
                std::slice::from_ref(&name.name),
                false,
                UsedModulePathProcessing::Never,
            ));
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_group_item_modules_from_path(&host_path, item, paths);
            }
        }
    }
}

fn collect_group_item_modules(
    root: &UsedModuleRoot,
    item: &UsingGroupItem,
    paths: &mut Vec<UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            paths.push(root.path(std::slice::from_ref(&name.name), false, false));
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested_root = root_with_extra(root, &host_segments(host));
            collect_selector_modules(nested_root, selector, paths);
        }
    }
}

fn collect_group_item_modules_from_path(
    root: &UsedModulePath,
    item: &UsingGroupItem,
    paths: &mut Vec<UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            paths.push(root.with_appended_segments_with_processing_mode(
                std::slice::from_ref(&name.name),
                false,
                UsedModulePathProcessing::Never,
            ));
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested =
                root.with_appended_segments_with_processing(&host_segments(host), false, false);
            collect_selector_modules_from_path(nested, selector, paths);
        }
    }
}

fn root_with_extra(root: &UsedModuleRoot, extra: &[SymbolId]) -> UsedModuleRoot {
    match root {
        UsedModuleRoot::Package { package, base } => UsedModuleRoot::Package {
            package: *package,
            base: joined_segments(base, extra),
        },
        UsedModuleRoot::PackageRelative { base } => UsedModuleRoot::PackageRelative {
            base: joined_segments(base, extra),
        },
        UsedModuleRoot::ParentRelative { base } => UsedModuleRoot::ParentRelative {
            base: joined_segments(base, extra),
        },
        UsedModuleRoot::Local { base } => UsedModuleRoot::Local {
            base: joined_segments(base, extra),
        },
    }
}

pub(crate) fn host_segments(host: &[UsingHostSegment]) -> Vec<SymbolId> {
    host.iter().filter_map(using_host_segment_name).collect()
}

pub(crate) fn type_ref_last_name(ty: &TypeRef) -> Option<SymbolId> {
    match &ty.kind {
        TypeKind::Path { segments } => segments.last().and_then(type_path_segment_name),
        _ => None,
    }
}

fn using_host_segment_name(segment: &UsingHostSegment) -> Option<SymbolId> {
    match segment.kind {
        PathSegmentKind::Name(name) => Some(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
    }
}

fn type_path_segment_name(segment: &TypePathSegment) -> Option<SymbolId> {
    match segment.kind {
        PathSegmentKind::Name(name) => Some(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
    }
}

fn type_path_names(segments: &[TypePathSegment]) -> Option<Vec<SymbolId>> {
    segments.iter().map(type_path_segment_name).collect()
}

pub(crate) fn joined_segments(base: &[SymbolId], extra: &[SymbolId]) -> Vec<SymbolId> {
    let mut segments = Vec::with_capacity(base.len() + extra.len());
    segments.extend_from_slice(base);
    segments.extend_from_slice(extra);
    segments
}
