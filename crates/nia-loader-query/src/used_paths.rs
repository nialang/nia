use nia_ast::{
    Expr, ExprKind, Item, ItemKind, Stmt, StmtKind, TypeKind, TypeRef, UsingGroupItem, UsingItem,
    UsingSelector,
};
use nia_ast_walk::{Visitor, walk_expr, walk_item, walk_module, walk_stmt, walk_type};
use nia_diagnostic::Diagnostic;
use nia_imports::{ModuleMap, ResolvedModuleDeclaration};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind};
use std::collections::HashMap;

pub(crate) fn collect_used_modules(
    item_tree: &ActiveModuleItemTree,
    module_map: &ModuleMap,
) -> (Vec<String>, Vec<UsedModulePath>) {
    let mut packages = Vec::new();
    let mut paths = Vec::new();
    let local_module_names = item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Module(module) => Some(module.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let using_aliases = module_using_aliases(item_tree, module_map, &local_module_names);
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
        packages: &mut packages,
        paths: &mut paths,
        locals: Vec::new(),
    };
    walk_module(&mut collector, &module);
    packages.sort();
    packages.dedup();
    paths.sort();
    paths.dedup();
    for path in &paths {
        if let UsedModulePath::Package { package, .. } = path {
            packages.push(package.clone());
        }
    }
    packages.sort();
    packages.dedup();
    (packages, paths)
}

struct QualifiedPathModuleCollector<'a> {
    module_map: &'a ModuleMap,
    local_module_names: &'a [String],
    using_aliases: &'a HashMap<String, UsedModulePath>,
    packages: &'a mut Vec<String>,
    paths: &'a mut Vec<UsedModulePath>,
    locals: Vec<HashMap<String, String>>,
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

    fn collect_path_segments(&mut self, segments: Vec<String>) {
        self.collect_path_segments_with_processing(
            segments,
            UsedModulePathProcessing::IfSelectedItem,
        );
    }

    fn collect_path_segments_with_processing(
        &mut self,
        segments: Vec<String>,
        processing: UsedModulePathProcessing,
    ) {
        let Some((first, rest)) = segments.split_first() else {
            return;
        };
        if let Some(alias) = self.using_aliases.get(first) {
            self.paths
                .push(alias.with_appended_segments_with_processing_mode(rest, false, processing));
            return;
        }
        if first == nia_imports::PACKAGE_MODULE_MAP_NAME {
            self.paths.push(UsedModulePath::PackageRelative {
                segments: rest.to_vec(),
                include_declared_children: false,
                processing: if processing == UsedModulePathProcessing::IfSelectedItem {
                    UsedModulePathProcessing::Always
                } else {
                    processing
                },
            });
            return;
        }
        if first == nia_imports::ENTRY_MODULE_MAP_NAME {
            return;
        }
        if !self.local_module_names.contains(first) && self.module_map.get(first).is_some() {
            self.packages.push(first.clone());
            self.paths.push(UsedModulePath::Package {
                package: first.clone(),
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
        self.collect_path_segments_with_processing(
            segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect(),
            UsedModulePathProcessing::IfProvidesTraitImpl {
                trait_name: last.name.clone(),
            },
        );
    }

    fn collect_trait_method_provider(&mut self, target_type_name: Option<&str>, name: &str) {
        for alias in self.using_aliases.values() {
            self.paths
                .push(alias.with_appended_segments_with_processing_mode(
                    &[],
                    false,
                    UsedModulePathProcessing::IfProvidesTraitMethod {
                        target_type_name: target_type_name.map(ToString::to_string),
                        associated_name: name.to_string(),
                    },
                ));
        }
    }

    fn collect_inherent_provider_for_type(&mut self, target: &TypeRef, associated_name: &str) {
        let TypeKind::Path { segments } = &target.kind else {
            return;
        };
        let Some(last) = segments.last() else {
            return;
        };
        self.collect_path_segments_with_processing(
            segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect(),
            UsedModulePathProcessing::IfProvidesInherentAssociated {
                target_type_name: last.name.clone(),
                associated_name: associated_name.to_string(),
            },
        );
    }
}

pub(crate) fn module_using_aliases(
    item_tree: &ActiveModuleItemTree,
    module_map: &ModuleMap,
    local_module_names: &[String],
) -> HashMap<String, UsedModulePath> {
    let mut aliases: HashMap<String, UsedModulePath> = HashMap::new();
    let mut packages = Vec::new();
    for item in &item_tree.items {
        let ItemTreeNodeKind::Using(using) = &item.kind else {
            continue;
        };
        if !using.host.is_empty()
            && let Some((first, rest)) = using.host.split_first()
            && let Some(alias) = aliases.get(&first.name).cloned()
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
        self.locals.push(HashMap::new());
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
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let ExprKind::Call { callee, args } = &expr.kind {
            if let ExprKind::Field { name, .. } = &callee.kind {
                let target_type_name = method_receiver_local_type_name(callee)
                    .and_then(|local_name| self.local_type_name(local_name));
                self.collect_trait_method_provider(target_type_name.as_deref(), name);
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
                    .map(|segment| segment.name.clone())
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
        self.locals.push(HashMap::new());
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
        self.locals.push(HashMap::new());
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

    fn record_local_type(&mut self, name: &str, ty: &TypeRef) {
        let Some(type_name) = type_ref_last_name(ty) else {
            return;
        };
        if let Some(scope) = self.locals.last_mut() {
            scope.insert(name.to_string(), type_name.to_string());
        }
    }

    fn local_type_name(&self, name: &str) -> Option<String> {
        self.locals
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }
}

fn method_receiver_local_type_name(callee: &Expr) -> Option<&str> {
    let ExprKind::Field { lhs, .. } = &callee.kind else {
        return None;
    };
    match &lhs.kind {
        ExprKind::Ident(name) => Some(name.as_str()),
        _ => None,
    }
}

struct ExtendSelfMethodCollector<'a, 'b> {
    target: &'a TypeRef,
    module_collector: &'a mut QualifiedPathModuleCollector<'b>,
}

impl<'ast> Visitor<'ast> for ExtendSelfMethodCollector<'_, '_> {
    fn visit_block(&mut self, block: &'ast nia_ast::Block) {
        self.module_collector.locals.push(HashMap::new());
        nia_ast_walk::walk_block(self, block);
        self.module_collector.locals.pop();
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let ExprKind::Field { lhs, name } = &expr.kind
            && matches!(&lhs.kind, ExprKind::Ident(lhs_name) if lhs_name == "self")
        {
            self.module_collector
                .collect_inherent_provider_for_type(self.target, name);
        }
        if let ExprKind::Call { callee, args } = &expr.kind {
            if let ExprKind::Field { name, .. } = &callee.kind {
                let target_type_name = if matches!(
                    method_receiver_local_type_name(callee),
                    Some(local_name) if local_name == "self"
                ) {
                    type_ref_last_name(self.target).map(ToString::to_string)
                } else {
                    method_receiver_local_type_name(callee)
                        .and_then(|local_name| self.module_collector.local_type_name(local_name))
                };
                self.module_collector
                    .collect_trait_method_provider(target_type_name.as_deref(), name);
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
        walk_stmt(self, stmt);
    }
}

fn expr_qualified_segments(expr: &Expr) -> Option<Vec<String>> {
    fn collect(expr: &Expr, segments: &mut Vec<String>) -> Option<()> {
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
    local_module_names: &[String],
    aliases: &HashMap<String, UsedModulePath>,
    packages: &mut Vec<String>,
    paths: &mut Vec<UsedModulePath>,
) {
    if host.is_empty() {
        collect_root_group_modules(selector, module_map, local_module_names, packages, paths);
        return;
    }
    if let Some((first, rest)) = host.split_first()
        && let Some(alias) = aliases.get(&first.name)
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
    local_module_names: &[String],
    packages: &mut Vec<String>,
    aliases: &mut HashMap<String, UsedModulePath>,
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
    aliases: &mut HashMap<String, UsedModulePath>,
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
                name.alias.clone().unwrap_or_else(|| name.name.clone()),
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
    aliases: &mut HashMap<String, UsedModulePath>,
) {
    match selector {
        UsingSelector::SelfName => {
            if let Some(name) = host_path.last_segment_name() {
                insert_using_alias(aliases, name.to_string(), host_path);
            }
        }
        UsingSelector::Wildcard { .. } => {}
        UsingSelector::Single(name) => {
            insert_using_alias(
                aliases,
                name.alias.clone().unwrap_or_else(|| name.name.clone()),
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
    aliases: &mut HashMap<String, UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            insert_using_alias(
                aliases,
                name.alias.clone().unwrap_or_else(|| name.name.clone()),
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
    aliases: &mut HashMap<String, UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            insert_using_alias(
                aliases,
                name.alias.clone().unwrap_or_else(|| name.name.clone()),
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
    aliases: &mut HashMap<String, UsedModulePath>,
    name: String,
    path: UsedModulePath,
) {
    aliases.entry(name).or_insert(path);
}

pub(crate) fn using_host_path(
    host: &[nia_ast::UsingHostSegment],
    module_map: &ModuleMap,
    local_module_names: &[String],
    aliases: &HashMap<String, UsedModulePath>,
) -> Option<UsedModulePath> {
    let first = host.first()?;
    if let Some(alias) = aliases.get(&first.name) {
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

fn collect_root_group_modules(
    selector: &UsingSelector,
    module_map: &ModuleMap,
    local_module_names: &[String],
    packages: &mut Vec<String>,
    paths: &mut Vec<UsedModulePath>,
) {
    let UsingSelector::Group(items) = selector else {
        return;
    };
    for item in items {
        match item {
            UsingGroupItem::Name(name) => {
                if name.name != nia_imports::ENTRY_MODULE_MAP_NAME
                    && name.name != nia_imports::PACKAGE_MODULE_MAP_NAME
                    && !local_module_names.contains(&name.name)
                    && module_map.get(&name.name).is_some()
                {
                    packages.push(name.name.clone());
                    paths.push(UsedModulePath::Package {
                        package: name.name.clone(),
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
                    &HashMap::new(),
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
    pub(crate) package_roots: Vec<String>,
    pub(crate) used_module_paths: Vec<UsedModulePath>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum UsedModulePath {
    Package {
        package: String,
        segments: Vec<String>,
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    },
    PackageRelative {
        segments: Vec<String>,
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    },
    Local {
        segments: Vec<String>,
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    },
}

impl UsedModulePath {
    pub(crate) fn with_appended_segments(
        &self,
        extra: &[String],
        include_declared_children: bool,
    ) -> Self {
        self.with_appended_segments_with_processing(extra, include_declared_children, true)
    }

    pub(crate) fn with_appended_segments_with_processing(
        &self,
        extra: &[String],
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
        extra: &[String],
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    ) -> Self {
        match self {
            UsedModulePath::Package {
                package, segments, ..
            } => UsedModulePath::Package {
                package: package.clone(),
                segments: joined_segments(segments, extra),
                include_declared_children,
                processing,
            },
            UsedModulePath::PackageRelative { segments, .. } => UsedModulePath::PackageRelative {
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

    pub(crate) fn segments(&self) -> &[String] {
        match self {
            UsedModulePath::Package { segments, .. }
            | UsedModulePath::PackageRelative { segments, .. }
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
            | UsedModulePath::Local { processing, .. } => processing.clone(),
        }
    }

    pub(crate) fn last_segment_name(&self) -> Option<&str> {
        match self {
            UsedModulePath::Package {
                package, segments, ..
            } => segments
                .last()
                .map_or(Some(package.as_str()), |segment| Some(segment.as_str())),
            UsedModulePath::PackageRelative { segments, .. }
            | UsedModulePath::Local { segments, .. } => segments.last().map(String::as_str),
        }
    }

    pub(crate) fn activates_package_facade(&self) -> Option<&str> {
        match self {
            UsedModulePath::Package {
                package,
                segments,
                include_declared_children,
                ..
            } if segments.is_empty() && *include_declared_children => Some(package),
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
        trait_name: String,
    },
    IfProvidesTraitMethod {
        target_type_name: Option<String>,
        associated_name: String,
    },
    IfProvidesInherentAssociated {
        target_type_name: String,
        associated_name: String,
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
            Self::IfProvidesTraitMethod {
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
    Package { package: String, base: Vec<String> },
    PackageRelative { base: Vec<String> },
    Local { base: Vec<String> },
}

impl UsedModuleRoot {
    fn from_host(
        host: &[nia_ast::UsingHostSegment],
        module_map: &ModuleMap,
        local_module_names: &[String],
        packages: &mut Vec<String>,
    ) -> Option<Self> {
        let first = host.first()?;
        if first.name == nia_imports::ENTRY_MODULE_MAP_NAME {
            return Some(Self::Package {
                package: nia_imports::ENTRY_MODULE_MAP_NAME.to_string(),
                base: host_segments(&host[1..]),
            });
        }
        if first.name == nia_imports::PACKAGE_MODULE_MAP_NAME {
            return Some(Self::PackageRelative {
                base: host_segments(&host[1..]),
            });
        }
        if local_module_names.contains(&first.name) {
            return Some(Self::Local {
                base: host_segments(host),
            });
        }
        if module_map.get(&first.name).is_some() {
            packages.push(first.name.clone());
            return Some(Self::Package {
                package: first.name.clone(),
                base: host_segments(&host[1..]),
            });
        }
        Some(Self::Local {
            base: host_segments(host),
        })
    }

    fn path(
        &self,
        extra: &[String],
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
        extra: &[String],
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    ) -> UsedModulePath {
        match self {
            UsedModuleRoot::Package { package, base } => UsedModulePath::Package {
                package: package.clone(),
                segments: joined_segments(base, extra),
                include_declared_children,
                processing,
            },
            UsedModuleRoot::PackageRelative { base } => UsedModulePath::PackageRelative {
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

    fn last_segment_name(&self) -> Option<String> {
        match self {
            UsedModuleRoot::Package { package, base } => {
                Some(base.last().cloned().unwrap_or_else(|| package.clone()))
            }
            UsedModuleRoot::PackageRelative { base } | UsedModuleRoot::Local { base } => {
                base.last().cloned()
            }
        }
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
            paths.push(used_root.path_with_processing_mode(
                std::slice::from_ref(&name.name),
                false,
                UsedModulePathProcessing::IfSelectedItem,
            ));
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
                UsedModulePathProcessing::IfSelectedItem,
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
            paths.push(root.path_with_processing_mode(
                std::slice::from_ref(&name.name),
                false,
                UsedModulePathProcessing::IfSelectedItem,
            ));
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
                UsedModulePathProcessing::IfSelectedItem,
            ));
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested =
                root.with_appended_segments_with_processing(&host_segments(host), false, false);
            collect_selector_modules_from_path(nested, selector, paths);
        }
    }
}

fn root_with_extra(root: &UsedModuleRoot, extra: &[String]) -> UsedModuleRoot {
    match root {
        UsedModuleRoot::Package { package, base } => UsedModuleRoot::Package {
            package: package.clone(),
            base: joined_segments(base, extra),
        },
        UsedModuleRoot::PackageRelative { base } => UsedModuleRoot::PackageRelative {
            base: joined_segments(base, extra),
        },
        UsedModuleRoot::Local { base } => UsedModuleRoot::Local {
            base: joined_segments(base, extra),
        },
    }
}

pub(crate) fn host_segments(host: &[nia_ast::UsingHostSegment]) -> Vec<String> {
    host.iter().map(|segment| segment.name.clone()).collect()
}

pub(crate) fn type_ref_last_name(ty: &TypeRef) -> Option<&str> {
    match &ty.kind {
        TypeKind::Path { segments } => segments.last().map(|segment| segment.name.as_str()),
        _ => None,
    }
}

pub(crate) fn joined_segments(base: &[String], extra: &[String]) -> Vec<String> {
    let mut segments = Vec::with_capacity(base.len() + extra.len());
    segments.extend_from_slice(base);
    segments.extend_from_slice(extra);
    segments
}
