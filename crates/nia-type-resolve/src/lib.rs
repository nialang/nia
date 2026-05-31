// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::Visibility;
use nia_ast::{
    ArrayLen, FunctionItem, Item, ItemKind, Module, TypeArg, TypeKind, TypePathSegment, TypeRef,
};
use nia_ast_walk::{Visitor, walk_function, walk_item, walk_module};
use nia_defs::{DefCollection, DefKind, ModuleUsingScope, PublicNamespace, PublicSurfaces};
use nia_diagnostic::Diagnostic;
pub use nia_ids::DefId;
use nia_ids::{GlobalDefId, ModuleId};
use nia_imports::ImportAliasMap;
use nia_span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct TypeResolution {
    pub type_names: HashMap<Span, TypeNameResolution>,
    pub qualified_type_names: HashMap<Span, GlobalDefId>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeNameResolution {
    Primitive(PrimitiveType),
    Def(DefId),
    External(GlobalDefId),
    GenericParam,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveType {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    F32,
    F64,
    Bool,
    Char,
    Void,
    Never,
}

#[derive(Debug, Clone, Copy)]
pub struct ProgramDefsContext<'a> {
    pub defs: Option<&'a HashMap<ModuleId, DefCollection>>,
}

impl<'a> ProgramDefsContext<'a> {
    pub fn empty() -> Self {
        Self { defs: None }
    }
}

pub fn resolve_module_types(module: &Module, defs: &DefCollection) -> TypeResolution {
    resolve_module_types_inner(module, defs, None, ProgramDefsContext::empty(), None, None)
}

pub fn resolve_module_types_with_imports(
    module: &Module,
    defs: &DefCollection,
    imports: &ImportAliasMap,
    program_defs: ProgramDefsContext<'_>,
) -> TypeResolution {
    resolve_module_types_inner(module, defs, Some(imports), program_defs, None, None)
}

pub fn resolve_module_types_with_context(
    module: &Module,
    defs: &DefCollection,
    imports: &ImportAliasMap,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: &PublicSurfaces,
    using_scope: &ModuleUsingScope,
) -> TypeResolution {
    resolve_module_types_inner(
        module,
        defs,
        Some(imports),
        program_defs,
        Some(public_surfaces),
        Some(using_scope),
    )
}

fn resolve_module_types_inner(
    module: &Module,
    defs: &DefCollection,
    imports: Option<&ImportAliasMap>,
    program_defs: ProgramDefsContext<'_>,
    public_surfaces: Option<&PublicSurfaces>,
    using_scope: Option<&ModuleUsingScope>,
) -> TypeResolution {
    let mut resolver = TypeResolver {
        defs,
        imports,
        program_defs,
        public_surfaces,
        using_scope,
        type_names: HashMap::new(),
        qualified_type_names: HashMap::new(),
        diagnostics: Vec::new(),
        generic_stack: Vec::new(),
        self_type_stack: Vec::new(),
        suppress_unknown_type_errors: false,
    };
    walk_module(&mut resolver, module);
    TypeResolution {
        type_names: resolver.type_names,
        qualified_type_names: resolver.qualified_type_names,
        diagnostics: resolver.diagnostics,
    }
}

struct TypeResolver<'a> {
    defs: &'a DefCollection,
    imports: Option<&'a ImportAliasMap>,
    program_defs: ProgramDefsContext<'a>,
    public_surfaces: Option<&'a PublicSurfaces>,
    using_scope: Option<&'a ModuleUsingScope>,
    type_names: HashMap<Span, TypeNameResolution>,
    qualified_type_names: HashMap<Span, GlobalDefId>,
    diagnostics: Vec<Diagnostic>,
    generic_stack: Vec<Vec<String>>,
    self_type_stack: Vec<Span>,
    suppress_unknown_type_errors: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedNamespace {
    Module(ModuleId),
    Type(GlobalDefId),
}

impl<'ast> Visitor<'ast> for TypeResolver<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        match &item.kind {
            ItemKind::Struct(item_struct) => {
                self.with_generics(&item_struct.generics, |resolver| walk_item(resolver, item));
            }
            ItemKind::Union(item_union) => {
                self.with_generics(&item_union.generics, |resolver| walk_item(resolver, item));
            }
            ItemKind::Trait(item_trait) => {
                self.with_generics(&item_trait.generics, |resolver| {
                    resolver.with_self_type(item.span, |resolver| walk_item(resolver, item));
                });
            }
            ItemKind::Extend(extend) => {
                self.with_generics(&extend.generics, |resolver| {
                    resolver
                        .with_self_type(extend.target.span, |resolver| walk_item(resolver, item));
                });
            }
            ItemKind::TypeAlias(alias) => {
                self.with_generics(&alias.generics, |resolver| walk_item(resolver, item));
            }
            _ => walk_item(self, item),
        }
    }

    fn visit_function(&mut self, function: &'ast FunctionItem) {
        self.with_generics(&function.generics, |resolver| {
            walk_function(resolver, function);
        });
    }

    fn visit_expr(&mut self, expr: &'ast nia_ast::Expr) {
        match &expr.kind {
            nia_ast::ExprKind::BracketSuffix { callee, args } => {
                self.visit_expr(callee);
                for arg in args {
                    if let Some(expr) = &arg.expr {
                        nia_ast_walk::walk_expr(self, expr);
                    }
                    if let Some(ty) = &arg.ty {
                        self.with_suppressed_unknown_type_errors(|resolver| {
                            resolver.visit_type(ty);
                        });
                    }
                }
            }
            _ => nia_ast_walk::walk_expr(self, expr),
        }
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        match &ty.kind {
            TypeKind::Error | TypeKind::Infer | TypeKind::Void | TypeKind::Never => {}
            TypeKind::SelfType => {
                if self.self_type_stack.is_empty() {
                    self.diagnostics.push(Diagnostic::error(
                        ty.span,
                        "`Self` is only valid in traits and extend blocks",
                    ));
                }
            }
            TypeKind::Pointer { elem, .. } | TypeKind::Slice { elem, .. } => {
                self.visit_type(elem);
            }
            TypeKind::Array { len, elem } => {
                if let ArrayLen::Expr(expr) = len {
                    nia_ast_walk::walk_expr(self, expr);
                }
                self.visit_type(elem);
            }
            TypeKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                for param in params {
                    self.visit_type(param);
                }
                if let Some(return_type) = return_type {
                    self.visit_type(return_type);
                }
            }
            TypeKind::Path { segments } => self.resolve_type_path(ty.span, segments),
        }
    }
}

impl<'a> TypeResolver<'a> {
    fn resolve_type_path(&mut self, span: Span, segments: &[TypePathSegment]) {
        let Some(first) = segments.first() else {
            return;
        };
        if segments.len() > 1 {
            let resolution = self.resolve_qualified_type_path(span, segments);
            self.type_names.insert(span, resolution);
            for segment in segments {
                for arg in &segment.args {
                    if let TypeArg::Type(ty) = arg {
                        self.visit_type(ty);
                    }
                }
            }
            return;
        }
        let resolution = self.resolve_type_name(first, span);
        self.type_names.insert(span, resolution);
        for segment in segments {
            for arg in &segment.args {
                if let TypeArg::Type(ty) = arg {
                    self.visit_type(ty);
                }
            }
        }
    }

    fn resolve_qualified_type_path(
        &mut self,
        span: Span,
        segments: &[TypePathSegment],
    ) -> TypeNameResolution {
        let Some((last, prefix)) = segments.split_last() else {
            return TypeNameResolution::Error;
        };
        let Some(namespace) = self.resolve_namespace_path(prefix) else {
            return TypeNameResolution::Error;
        };
        match namespace {
            ResolvedNamespace::Module(module_id) => {
                let path_text = type_path_text(segments);
                self.resolve_module_type(span, module_id, last, &path_text)
            }
            ResolvedNamespace::Type(_) => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "type namespaces do not contain nested types",
                ));
                TypeNameResolution::Error
            }
        }
    }

    fn resolve_namespace_path(
        &mut self,
        segments: &[TypePathSegment],
    ) -> Option<ResolvedNamespace> {
        let first = segments.first()?;
        let mut namespace = self.resolve_root_namespace(first)?;
        for segment in &segments[1..] {
            namespace = self.resolve_child_namespace(namespace, segment)?;
        }
        Some(namespace)
    }

    fn resolve_root_namespace(&mut self, segment: &TypePathSegment) -> Option<ResolvedNamespace> {
        if let Some(imports) = self.imports
            && let Some(import) = imports.get(self.defs.module_id, &segment.name)
        {
            return Some(ResolvedNamespace::Module(import.target));
        }
        if let Some(scope) = self.using_scope
            && let Some(module_id) = scope.lookup_module(&segment.name)
        {
            return Some(ResolvedNamespace::Module(module_id));
        }
        if let Some(def_id) = self.defs.module_scope.types.get(&segment.name) {
            return Some(ResolvedNamespace::Type(GlobalDefId {
                module_id: self.defs.module_id,
                def_id,
            }));
        }
        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_type(&segment.name)
        {
            return Some(ResolvedNamespace::Type(GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            }));
        }
        self.diagnostics.push(Diagnostic::error(
            segment_span(segment),
            format!("unknown namespace `{}`", segment.name),
        ));
        None
    }

    fn resolve_child_namespace(
        &mut self,
        namespace: ResolvedNamespace,
        segment: &TypePathSegment,
    ) -> Option<ResolvedNamespace> {
        match namespace {
            ResolvedNamespace::Module(module_id) => {
                if let Some(surfaces) = self.public_surfaces
                    && let Some(surface) = surfaces.get(module_id)
                {
                    if let Some(child_module) = surface.lookup_module(&segment.name) {
                        return Some(ResolvedNamespace::Module(child_module));
                    }
                    if let Some(item) = surface.lookup_type(&segment.name) {
                        return Some(ResolvedNamespace::Type(GlobalDefId {
                            module_id: item.target_module,
                            def_id: item.target_def_id,
                        }));
                    }
                }
                let target_defs = self.defs_for_module(module_id)?;
                let def_id = target_defs.module_scope.types.get(&segment.name)?;
                let def = target_defs.defs.get(def_id)?;
                if module_id != self.defs.module_id && def.visibility != Visibility::Public {
                    self.diagnostics.push(Diagnostic::error(
                        segment_span(segment),
                        format!("type `{}` is private", segment.name),
                    ));
                    return None;
                }
                Some(ResolvedNamespace::Type(GlobalDefId { module_id, def_id }))
            }
            ResolvedNamespace::Type(_) => None,
        }
    }

    fn resolve_module_type(
        &mut self,
        span: Span,
        module_id: ModuleId,
        segment: &TypePathSegment,
        path_text: &str,
    ) -> TypeNameResolution {
        if let Some(surfaces) = self.public_surfaces
            && let Some(surface) = surfaces.get(module_id)
            && let Some(item) = surface.lookup_type(&segment.name)
        {
            let global = GlobalDefId {
                module_id: item.target_module,
                def_id: item.target_def_id,
            };
            self.qualified_type_names.insert(span, global);
            return TypeNameResolution::External(global);
        }
        let Some(target_defs) = self.defs_for_module(module_id) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "module namespace refers to an unloaded module",
            ));
            return TypeNameResolution::Error;
        };
        let Some(def_id) = target_defs.module_scope.types.get(&segment.name) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("unknown type `{}`", segment.name),
            ));
            return TypeNameResolution::Error;
        };
        let Some(def) = target_defs.defs.get(def_id) else {
            return TypeNameResolution::Error;
        };
        if !matches!(
            def.kind,
            DefKind::Struct | DefKind::Union | DefKind::Trait | DefKind::Enum | DefKind::TypeAlias
        ) {
            return TypeNameResolution::Error;
        }
        if module_id != self.defs.module_id && def.visibility != Visibility::Public {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("type `{path_text}` is private"),
            ));
            return TypeNameResolution::Error;
        }
        self.qualified_type_names
            .insert(span, GlobalDefId { module_id, def_id });
        if module_id == self.defs.module_id {
            TypeNameResolution::Def(def_id)
        } else {
            TypeNameResolution::External(GlobalDefId { module_id, def_id })
        }
    }

    fn resolve_type_name(&mut self, segment: &TypePathSegment, span: Span) -> TypeNameResolution {
        if let Some(primitive) = primitive_type(&segment.name) {
            return TypeNameResolution::Primitive(primitive);
        }
        if self.is_generic_param(&segment.name) {
            return TypeNameResolution::GenericParam;
        }
        if let Some(def_id) = self.defs.module_scope.types.get(&segment.name) {
            let Some(def) = self.defs.defs.get(def_id) else {
                return TypeNameResolution::Error;
            };
            if matches!(
                def.kind,
                DefKind::Struct
                    | DefKind::Union
                    | DefKind::Trait
                    | DefKind::Enum
                    | DefKind::TypeAlias
            ) {
                return TypeNameResolution::Def(def_id);
            }
        }
        if let Some(scope) = self.using_scope
            && let Some(entry) = scope.lookup_type(&segment.name)
            && entry.namespace == PublicNamespace::Type
        {
            let global = GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            };
            self.qualified_type_names.insert(span, global);
            return TypeNameResolution::External(global);
        }
        if !self.suppress_unknown_type_errors {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("unknown type `{}`", segment.name),
            ));
        }
        TypeNameResolution::Error
    }

    fn with_suppressed_unknown_type_errors(&mut self, f: impl FnOnce(&mut Self)) {
        let previous = self.suppress_unknown_type_errors;
        self.suppress_unknown_type_errors = true;
        f(self);
        self.suppress_unknown_type_errors = previous;
    }

    fn with_generics(&mut self, generics: &[String], f: impl FnOnce(&mut Self)) {
        self.generic_stack.push(generics.to_vec());
        f(self);
        self.generic_stack.pop();
    }

    fn with_self_type(&mut self, span: Span, f: impl FnOnce(&mut Self)) {
        self.self_type_stack.push(span);
        f(self);
        self.self_type_stack.pop();
    }

    fn is_generic_param(&self, name: &str) -> bool {
        self.generic_stack
            .iter()
            .rev()
            .any(|generics| generics.iter().any(|generic| generic == name))
    }

    fn defs_for_module(&self, module_id: ModuleId) -> Option<&DefCollection> {
        if module_id == self.defs.module_id {
            Some(self.defs)
        } else {
            self.program_defs.defs?.get(&module_id)
        }
    }
}

fn segment_span(_segment: &TypePathSegment) -> Span {
    Span::default()
}

fn type_path_text(segments: &[TypePathSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join("::")
}

fn primitive_type(name: &str) -> Option<PrimitiveType> {
    Some(match name {
        "i8" => PrimitiveType::I8,
        "i16" => PrimitiveType::I16,
        "i32" => PrimitiveType::I32,
        "i64" => PrimitiveType::I64,
        "i128" => PrimitiveType::I128,
        "isize" => PrimitiveType::Isize,
        "u8" => PrimitiveType::U8,
        "u16" => PrimitiveType::U16,
        "u32" => PrimitiveType::U32,
        "u64" => PrimitiveType::U64,
        "u128" => PrimitiveType::U128,
        "usize" => PrimitiveType::Usize,
        "f32" => PrimitiveType::F32,
        "f64" => PrimitiveType::F64,
        "bool" => PrimitiveType::Bool,
        "char" => PrimitiveType::Char,
        "void" => PrimitiveType::Void,
        "!" => PrimitiveType::Never,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::{ModuleId, collect_module_defs};
    use nia_parser::parse_module;

    #[test]
    fn resolves_primitive_nominal_and_generic_types() {
        let (module, errors) = parse_module(
            r#"
struct Box[T] {
    value: T,
}

type Byte = u8;

fn make(value: i32) Box[i32] {
    var tmp: Byte = 1;
    { value: value }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        assert!(defs.diagnostics.is_empty(), "{:?}", defs.diagnostics);
        let resolved = resolve_module_types(&module, &defs);
        assert!(
            resolved.diagnostics.is_empty(),
            "{:?}",
            resolved.diagnostics
        );
        assert!(
            resolved
                .type_names
                .values()
                .any(|resolution| matches!(resolution, TypeNameResolution::GenericParam))
        );
        assert!(
            resolved
                .type_names
                .values()
                .any(|resolution| matches!(resolution, TypeNameResolution::Primitive(_)))
        );
        assert!(
            resolved
                .type_names
                .values()
                .any(|resolution| matches!(resolution, TypeNameResolution::Def(_)))
        );
    }

    #[test]
    fn reports_unknown_types_without_resolving_values() {
        let (module, errors) = parse_module(
            r#"
fn main() Missing {
    var value = MissingValue;
    0
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let defs = collect_module_defs(ModuleId(0), &module);
        let resolved = resolve_module_types(&module, &defs);
        assert_eq!(resolved.diagnostics.len(), 1);
        assert!(
            resolved.diagnostics[0]
                .message
                .contains("unknown type `Missing`")
        );
    }
}
