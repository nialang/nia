// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

mod extensions;
mod public_surface;

use nia_ast::{
    EnumItem, ExtendItem, FunctionItem, ImportPath, Item, ItemKind, Module, StructItem,
    TypeAliasItem, UnionItem, UsingItem, Visibility,
};
use nia_diagnostic::Diagnostic;
pub use nia_ids::{DefId, ModuleId};
use nia_span::Span;

pub use extensions::{
    ExtensionMethod, ExtensionMethods, VisibleExtensionMethod, VisibleExtensionMethods,
    VisibleExtensionTarget,
};
pub use public_surface::{
    ModulePublicSurface, ModuleUsingScope, PublicItem, PublicNamespace, PublicSource,
    PublicSurfaces, UsingEntry,
};

#[derive(Debug, Clone, PartialEq)]
pub struct DefCollection {
    pub module_id: ModuleId,
    pub defs: DefMap,
    pub module_scope: ModuleScope,
    pub scopes: DefScopes,
    pub def_spans: DefSpanMap,
    pub module_usings: Vec<ModuleUsing>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleUsing {
    pub visibility: Visibility,
    pub span: Span,
    pub host: Vec<nia_ast::UsingHostSegment>,
    pub selector: nia_ast::UsingSelector,
}

pub fn collect_module_defs(module_id: ModuleId, module: &Module) -> DefCollection {
    Collector::new(module_id).collect(module)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefMap {
    defs: Vec<Def>,
}

impl DefMap {
    pub fn get(&self, id: DefId) -> Option<&Def> {
        self.defs.get(id.0 as usize)
    }

    pub fn iter(&self) -> impl Iterator<Item = (DefId, &Def)> {
        self.defs
            .iter()
            .enumerate()
            .map(|(index, def)| (DefId(index as u32), def))
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    fn push(&mut self, def: Def) -> DefId {
        let id = DefId(self.defs.len() as u32);
        self.defs.push(def);
        id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Def {
    pub name: String,
    pub kind: DefKind,
    pub module_id: ModuleId,
    pub parent: Option<DefId>,
    pub generics: Vec<String>,
    pub visibility: Visibility,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefKind {
    Import,
    Function,
    Global,
    Comptime,
    Struct,
    StructField,
    Union,
    UnionField,
    Trait,
    TraitMethod,
    Method,
    Enum,
    EnumVariant,
    TypeAlias,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleScope {
    pub modules: NameTable,
    pub types: NameTable,
    pub values: NameTable,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MemberScope {
    pub fields: NameTable,
    pub methods: NameTable,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnumScope {
    pub variants: NameTable,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NameTable {
    entries: HashMap<String, NameEntry>,
}

impl NameTable {
    pub fn get(&self, name: &str) -> Option<DefId> {
        self.entries.get(name).map(|entry| entry.def_id)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, DefId)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name.as_str(), entry.def_id))
    }

    fn insert(&mut self, name: String, def_id: DefId, span: Span) -> Result<(), DuplicateName> {
        if let Some(existing) = self.entries.get(&name) {
            return Err(DuplicateName {
                name,
                first_span: existing.span,
                second_span: span,
            });
        }
        self.entries.insert(name, NameEntry { def_id, span });
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateName {
    pub name: String,
    pub first_span: Span,
    pub second_span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefScopes {
    pub struct_members: HashMap<DefId, MemberScope>,
    pub union_members: HashMap<DefId, MemberScope>,
    pub enum_members: HashMap<DefId, EnumScope>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefSpanMap {
    spans: HashMap<Span, DefId>,
}

impl DefSpanMap {
    pub fn get(&self, span: Span) -> Option<DefId> {
        self.spans.get(&span).copied()
    }

    pub fn entries(&self) -> impl Iterator<Item = (Span, DefId)> + '_ {
        self.spans.iter().map(|(span, def_id)| (*span, *def_id))
    }

    fn insert(&mut self, span: Span, def_id: DefId) {
        self.spans.insert(span, def_id);
    }
}

#[derive(Debug, Clone, PartialEq)]
struct NameEntry {
    def_id: DefId,
    span: Span,
}

struct Collector {
    module_id: ModuleId,
    defs: DefMap,
    module_scope: ModuleScope,
    struct_members: HashMap<DefId, MemberScope>,
    union_members: HashMap<DefId, MemberScope>,
    enum_members: HashMap<DefId, EnumScope>,
    def_spans: DefSpanMap,
    module_usings: Vec<ModuleUsing>,
    diagnostics: Vec<Diagnostic>,
}

impl Collector {
    fn new(module_id: ModuleId) -> Self {
        Self {
            module_id,
            defs: DefMap::default(),
            module_scope: ModuleScope::default(),
            struct_members: HashMap::new(),
            union_members: HashMap::new(),
            enum_members: HashMap::new(),
            def_spans: DefSpanMap::default(),
            module_usings: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn collect(mut self, module: &Module) -> DefCollection {
        for item in &module.items {
            self.collect_item(item);
        }
        DefCollection {
            module_id: self.module_id,
            defs: self.defs,
            module_scope: self.module_scope,
            scopes: DefScopes {
                struct_members: self.struct_members,
                union_members: self.union_members,
                enum_members: self.enum_members,
            },
            def_spans: self.def_spans,
            module_usings: self.module_usings,
            diagnostics: self.diagnostics,
        }
    }

    fn collect_item(&mut self, item: &Item) {
        match &item.kind {
            ItemKind::Import(import) => {
                let name = import
                    .alias
                    .clone()
                    .unwrap_or_else(|| import_default_alias(&import.path));
                self.add_module_def(name, DefKind::Import, item.vis, item.span);
            }
            ItemKind::Using(using) => {
                self.collect_using(item, using);
            }
            ItemKind::Struct(item_struct) => self.collect_struct(item, item_struct),
            ItemKind::Union(item_union) => self.collect_union(item, item_union),
            ItemKind::Trait(item_trait) => self.collect_trait(item, item_trait),
            ItemKind::Extend(extend) => self.collect_extend(item, extend),
            ItemKind::Enum(item_enum) => self.collect_enum(item, item_enum),
            ItemKind::TypeAlias(alias) => self.collect_type_alias(item, alias),
            ItemKind::Function(function) => {
                self.check_duplicate_generics(&function.generics, item.span);
                self.add_value_def(
                    function.name.clone(),
                    DefKind::Function,
                    item.vis,
                    item.span,
                    function.generics.clone(),
                );
            }
            ItemKind::Binding(binding) => {
                self.add_value_def(
                    binding.name.clone(),
                    if binding.is_comptime {
                        DefKind::Comptime
                    } else {
                        DefKind::Global
                    },
                    item.vis,
                    item.span,
                    Vec::new(),
                );
            }
        }
    }

    fn collect_using(&mut self, item: &Item, using: &UsingItem) {
        self.module_usings.push(ModuleUsing {
            visibility: item.vis,
            span: item.span,
            host: using.host.clone(),
            selector: using.selector.clone(),
        });
    }

    fn collect_struct(&mut self, item: &Item, item_struct: &StructItem) {
        self.check_duplicate_generics(&item_struct.generics, item.span);
        let struct_id = self.add_type_def(
            item_struct.name.clone(),
            DefKind::Struct,
            item.vis,
            item.span,
            item_struct.generics.clone(),
        );
        let mut members = MemberScope::default();
        for field in &item_struct.fields {
            let field_id = self.push_def(Def {
                name: field.name.clone(),
                kind: DefKind::StructField,
                module_id: self.module_id,
                parent: Some(struct_id),
                generics: Vec::new(),
                visibility: Visibility::Private,
                span: field.span,
            });
            self.def_spans.insert(field.span, field_id);
            self.insert_member(
                &mut members.fields,
                field.name.clone(),
                field_id,
                field.span,
                "duplicate struct field",
            );
        }
        self.struct_members.insert(struct_id, members);
    }

    fn collect_union(&mut self, item: &Item, item_union: &UnionItem) {
        self.check_duplicate_generics(&item_union.generics, item.span);
        let union_id = self.add_type_def(
            item_union.name.clone(),
            DefKind::Union,
            item.vis,
            item.span,
            item_union.generics.clone(),
        );
        let mut members = MemberScope::default();
        for field in &item_union.fields {
            let field_id = self.push_def(Def {
                name: field.name.clone(),
                kind: DefKind::UnionField,
                module_id: self.module_id,
                parent: Some(union_id),
                generics: Vec::new(),
                visibility: Visibility::Private,
                span: field.span,
            });
            self.def_spans.insert(field.span, field_id);
            self.insert_member(
                &mut members.fields,
                field.name.clone(),
                field_id,
                field.span,
                "duplicate union field",
            );
        }
        self.union_members.insert(union_id, members);
    }

    fn collect_extend(&mut self, _item: &Item, extend: &ExtendItem) {
        self.check_duplicate_generics(&extend.generics, extend.target.span);
        let mut members = MemberScope::default();
        for method in &extend.methods {
            self.collect_method(None, &mut members, &method.function, method.vis);
        }
    }

    fn collect_trait(&mut self, item: &Item, item_trait: &nia_ast::TraitItem) {
        self.check_duplicate_generics(&item_trait.generics, item.span);
        let trait_id = self.add_type_def(
            item_trait.name.clone(),
            DefKind::Trait,
            item.vis,
            item.span,
            item_trait.generics.clone(),
        );
        let mut members = MemberScope::default();
        for method in &item_trait.methods {
            self.collect_trait_method(Some(trait_id), &mut members, &method.function);
        }
        self.struct_members.insert(trait_id, members);
    }

    fn collect_trait_method(
        &mut self,
        parent: Option<DefId>,
        members: &mut MemberScope,
        method: &FunctionItem,
    ) {
        self.check_duplicate_generics(&method.generics, method.span);
        let method_id = self.push_def(Def {
            name: method.name.clone(),
            kind: DefKind::TraitMethod,
            module_id: self.module_id,
            parent,
            generics: method.generics.clone(),
            visibility: Visibility::Public,
            span: method.span,
        });
        self.def_spans.insert(method.span, method_id);
        self.insert_member(
            &mut members.methods,
            method.name.clone(),
            method_id,
            method.span,
            "duplicate trait method",
        );
    }

    fn collect_method(
        &mut self,
        parent: Option<DefId>,
        members: &mut MemberScope,
        method: &FunctionItem,
        visibility: Visibility,
    ) {
        self.check_duplicate_generics(&method.generics, method.span);
        let method_id = self.push_def(Def {
            name: method.name.clone(),
            kind: DefKind::Method,
            module_id: self.module_id,
            parent,
            generics: method.generics.clone(),
            visibility,
            span: method.span,
        });
        self.def_spans.insert(method.span, method_id);
        self.insert_member(
            &mut members.methods,
            method.name.clone(),
            method_id,
            method.span,
            "duplicate struct method",
        );
    }

    fn collect_enum(&mut self, item: &Item, item_enum: &EnumItem) {
        let enum_id = self.add_type_def(
            item_enum.name.clone(),
            DefKind::Enum,
            item.vis,
            item.span,
            Vec::new(),
        );
        let mut members = EnumScope::default();
        for variant in &item_enum.variants {
            let variant_id = self.push_def(Def {
                name: variant.name.clone(),
                kind: DefKind::EnumVariant,
                module_id: self.module_id,
                parent: Some(enum_id),
                generics: Vec::new(),
                visibility: Visibility::Public,
                span: variant.span,
            });
            self.def_spans.insert(variant.span, variant_id);
            self.insert_member(
                &mut members.variants,
                variant.name.clone(),
                variant_id,
                variant.span,
                "duplicate enum variant",
            );
        }
        self.enum_members.insert(enum_id, members);
    }

    fn collect_type_alias(&mut self, item: &Item, alias: &TypeAliasItem) {
        self.check_duplicate_generics(&alias.generics, item.span);
        self.add_type_def(
            alias.name.clone(),
            DefKind::TypeAlias,
            item.vis,
            item.span,
            alias.generics.clone(),
        );
    }

    fn add_module_def(
        &mut self,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
    ) -> DefId {
        let def_id = self.push_top_def(name.clone(), kind, visibility, span, Vec::new());
        Self::insert_top(
            &mut self.module_scope.modules,
            &mut self.diagnostics,
            name,
            def_id,
            span,
            "duplicate import name",
        );
        def_id
    }

    fn add_type_def(
        &mut self,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        generics: Vec<String>,
    ) -> DefId {
        let def_id = self.push_top_def(name.clone(), kind, visibility, span, generics);
        Self::insert_top(
            &mut self.module_scope.types,
            &mut self.diagnostics,
            name,
            def_id,
            span,
            "duplicate type definition",
        );
        def_id
    }

    fn add_value_def(
        &mut self,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        generics: Vec<String>,
    ) -> DefId {
        let def_id = self.push_top_def(name.clone(), kind, visibility, span, generics);
        Self::insert_top(
            &mut self.module_scope.values,
            &mut self.diagnostics,
            name,
            def_id,
            span,
            "duplicate value definition",
        );
        def_id
    }

    fn push_top_def(
        &mut self,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
        generics: Vec<String>,
    ) -> DefId {
        let def_id = self.push_def(Def {
            name,
            kind,
            module_id: self.module_id,
            parent: None,
            generics,
            visibility,
            span,
        });
        self.def_spans.insert(span, def_id);
        def_id
    }

    fn push_def(&mut self, def: Def) -> DefId {
        self.defs.push(def)
    }

    fn insert_top(
        table: &mut NameTable,
        diagnostics: &mut Vec<Diagnostic>,
        name: String,
        def_id: DefId,
        span: Span,
        message: &'static str,
    ) {
        if let Err(duplicate) = table.insert(name, def_id, span) {
            diagnostics.push(Diagnostic::error(
                duplicate.second_span,
                format!("{message}: `{}`", duplicate.name),
            ));
        }
    }

    fn insert_member(
        &mut self,
        table: &mut NameTable,
        name: String,
        def_id: DefId,
        span: Span,
        message: &'static str,
    ) {
        if let Err(duplicate) = table.insert(name, def_id, span) {
            self.diagnostics.push(Diagnostic::error(
                duplicate.second_span,
                format!("{message}: `{}`", duplicate.name),
            ));
        }
    }

    fn check_duplicate_generics(&mut self, generics: &[String], span: Span) {
        let mut seen = HashSet::new();
        for generic in generics {
            if !seen.insert(generic) {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("duplicate generic parameter `{generic}`"),
                ));
            }
        }
    }
}

fn import_default_alias(path: &ImportPath) -> String {
    path.segments
        .last()
        .cloned()
        .unwrap_or_else(|| "_".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_parser::parse_module;

    #[test]
    fn collects_top_level_defs_into_separate_namespaces() {
        let (module, errors) = parse_module(
            r#"
import .math;
struct Point { x: i32, y: i32 }
enum Color { Red, Green }
type Byte = u8;
fn Point() i32 { 0 }
var counter = 0;
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_module_defs(ModuleId(0), &module);
        assert!(
            collection.diagnostics.is_empty(),
            "{:?}",
            collection.diagnostics
        );
        assert!(collection.module_scope.modules.get("math").is_some());
        assert!(collection.module_scope.types.get("Point").is_some());
        assert!(collection.module_scope.types.get("Color").is_some());
        assert!(collection.module_scope.types.get("Byte").is_some());
        assert!(collection.module_scope.values.get("Point").is_some());
        assert!(collection.module_scope.values.get("counter").is_some());
        assert!(collection.def_spans.entries().count() >= collection.defs.len());
    }

    #[test]
    fn reports_duplicates_per_namespace() {
        let (module, errors) = parse_module(
            r#"
struct Thing { a: i32, a: i32 }
struct Thing {}
fn f() {}
fn f() {}
enum E { A, A }
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_module_defs(ModuleId(0), &module);
        assert_eq!(collection.diagnostics.len(), 4);
        assert!(
            collection
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate type definition"))
        );
        assert!(
            collection
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate value definition"))
        );
        assert!(
            collection
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate struct field"))
        );
        assert!(
            collection
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate enum variant"))
        );
    }

    #[test]
    fn reports_duplicate_generic_parameters() {
        let (module, errors) = parse_module(
            r#"
struct Box[T, T] { value: T }
type Alias[T, T] = T;
fn id[T, T](x: T) T { x }
struct Methods[T] {
    value: T,
}

extend[T, T] Methods[T] {
    fn get[U, U](self) T { self.value }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let collection = collect_module_defs(ModuleId(0), &module);
        let duplicate_count = collection
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("duplicate generic parameter"))
            .count();
        assert_eq!(duplicate_count, 5, "{:?}", collection.diagnostics);
    }
}
