// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn parses_explicit_generic_function_instantiation() {
    let (module, errors) = parse_module(
        r#"
fn id[T](value: T) T { value }
fn main() i32 {
    id[i32](1)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    assert!(matches!(callee.kind, ExprKind::BracketSuffix { .. }));
}

#[test]
fn parses_generic_type_prefix_associated_call() {
    let (module, errors) = parse_module(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }
}

fn main() Box[i32] {
    Box[i32]::make(1)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[2].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    let ExprKind::Qualified { lhs, .. } = &callee.kind else {
        panic!("expected qualified callee");
    };
    assert!(matches!(lhs.kind, ExprKind::BracketSuffix { .. }));
}

#[test]
fn parses_structural_type_target_associated_call() {
    let (module, errors) = parse_module(
        r#"
extend[T] &T {
    fn is_null(self) bool {
        self as usize == 0
    }
}

fn main(ptr: &u8) bool {
    [&u8]::is_null(ptr)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    let ExprKind::Qualified { lhs, .. } = &callee.kind else {
        panic!("expected qualified callee");
    };
    assert!(matches!(lhs.kind, ExprKind::TypeTarget { .. }));
}

#[test]
fn parses_trait_target_associated_comptime_projection() {
    let (module, errors) = parse_module(
        r#"
trait Simd {
    comptime Lanes: usize;
}

fn lanes[T]() usize where T: Simd {
    [T as Simd]::Lanes
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Qualified { lhs, name } = &tail.kind else {
        panic!("expected qualified projection");
    };
    assert_eq!(name, "Lanes");
    assert!(matches!(lhs.kind, ExprKind::TraitTarget { .. }));
}

#[test]
fn parses_deep_pointer_structural_type_target_associated_call() {
    let (module, errors) = parse_module(
        r#"
extend &&&&&& &&i32 {
    fn is_null(self) bool {
        self as usize == 0
    }
}

fn main(ptr: &&&&&& &&i32) bool {
    [&&&&&& &&i32]::is_null(ptr)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    let ExprKind::Qualified { lhs, .. } = &callee.kind else {
        panic!("expected qualified callee");
    };
    assert!(matches!(lhs.kind, ExprKind::TypeTarget { .. }));
}

#[test]
fn parses_array_structural_type_target_associated_call() {
    let (module, errors) = parse_module(
        r#"
extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

fn main(triple: [3]i32) i32 {
    [[3]i32]::first(triple)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    let ExprKind::Qualified { lhs, .. } = &callee.kind else {
        panic!("expected qualified callee");
    };
    assert!(matches!(lhs.kind, ExprKind::TypeTarget { .. }));
}

#[test]
fn parses_array_structural_type_target_in_binary_expr() {
    let (module, errors) = parse_module(
        r#"
extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

fn zero() i32 {
    0
}

fn main(triple: [3]i32) i32 {
    [[3]i32]::first(triple) + zero() as i32
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[2].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    assert!(matches!(tail.kind, ExprKind::Binary { .. }));
}

#[test]
fn parses_explicit_associated_type_projection() {
    let (module, errors) = parse_module(
        r#"
trait Source {
    type Item;

    fn get(&self) [Self as Source]::Item;
}

fn read[T](value: &T) [T as Source]::Item
where T: Source {
    value.get()
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Trait(item_trait) = &module.items[0].kind else {
        panic!("expected trait");
    };
    let return_ty = item_trait.methods[0]
        .function
        .return_type
        .as_ref()
        .expect("expected return type");
    assert!(matches!(return_ty.kind, TypeKind::Projection { .. }));
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let return_ty = function.return_type.as_ref().expect("expected return type");
    assert!(matches!(return_ty.kind, TypeKind::Projection { .. }));
}

#[test]
fn parses_generic_trait_associated_type_projection() {
    let (module, errors) = parse_module(
        r#"
trait Add[Rhs] {
    type Output;

    fn add(&self, rhs: Rhs) [Self as Add[Rhs]]::Output;
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Trait(item_trait) = &module.items[0].kind else {
        panic!("expected trait");
    };
    let return_ty = item_trait.methods[0]
        .function
        .return_type
        .as_ref()
        .expect("expected return type");
    let TypeKind::Projection {
        trait_ref, name, ..
    } = &return_ty.kind
    else {
        panic!("expected projection");
    };
    assert_eq!(name, "Output");
    let TypeKind::Path { segments } = &trait_ref.kind else {
        panic!("expected trait path");
    };
    assert_eq!(segments[0].name, "Add");
    assert_eq!(segments[0].args.len(), 1);
}

#[test]
fn parses_range_types_and_expressions() {
    let (module, errors) = parse_module(
        r#"
trait Slice[R] {
    type Output;
}

fn take[S](items: S, end: usize) [S as Slice[usize..usize]]::Output
where S: Slice[..] {
    0..end
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let return_ty = function.return_type.as_ref().expect("expected return type");
    let TypeKind::Projection { trait_ref, .. } = &return_ty.kind else {
        panic!("expected projection");
    };
    let TypeKind::Path { segments } = &trait_ref.kind else {
        panic!("expected trait path");
    };
    assert!(matches!(
        segments[0].args[0],
        TypeArg::Type(TypeRef {
            kind: TypeKind::Range { .. },
            ..
        })
    ));
    let bound = &function.where_clause.predicates[0].bounds[0];
    let TypeKind::Path { segments } = &bound.kind else {
        panic!("expected bound path");
    };
    assert!(matches!(
        segments[0].args[0],
        TypeArg::Type(TypeRef {
            kind: TypeKind::Range {
                start: None,
                end: None,
                ..
            },
            ..
        })
    ));
    let body = function.body.as_ref().expect("expected body");
    assert!(matches!(
        body.tail.as_ref().map(|tail| &tail.kind),
        Some(ExprKind::Range(_))
    ));
}

#[test]
fn parses_associated_type_bindings_in_where_bounds() {
    let (module, errors) = parse_module(
        r#"
trait Add[Rhs] {
    type Output;
}

trait Mapper[A, B] {
    type C;
    type D;
}

fn add_same[T](a: T, b: T) T
where T: Add[T, Output = T] {
    a
}

fn mapped[T, A, B](value: T) T
where T: Mapper[A, B, C = i32, D = bool] {
    value
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(add_same) = &module.items[2].kind else {
        panic!("expected function");
    };
    let add_bound = &add_same.where_clause.predicates[0].bounds[0];
    let TypeKind::Path { segments } = &add_bound.kind else {
        panic!("expected path bound");
    };
    assert_eq!(segments[0].args.len(), 2);
    assert!(matches!(
        segments[0].args[1],
        TypeArg::AssocBinding {
            key: nia_ast::AssocBindingKey::Name(ref name),
            ..
        } if name == "Output"
    ));
    let ItemKind::Function(mapped) = &module.items[3].kind else {
        panic!("expected function");
    };
    let mapper_bound = &mapped.where_clause.predicates[0].bounds[0];
    let TypeKind::Path { segments } = &mapper_bound.kind else {
        panic!("expected path bound");
    };
    assert_eq!(segments[0].args.len(), 4);
    assert!(matches!(
        segments[0].args[2],
        TypeArg::AssocBinding {
            key: nia_ast::AssocBindingKey::Name(ref name),
            ..
        } if name == "C"
    ));
    assert!(matches!(
        segments[0].args[3],
        TypeArg::AssocBinding {
            key: nia_ast::AssocBindingKey::Name(ref name),
            ..
        } if name == "D"
    ));
}

#[test]
fn parses_trait_associated_comptime_requirements() {
    let (module, errors) = parse_module(
        r#"
trait Simd {
    type Lane;
    comptime Lanes: usize;
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Trait(item_trait) = &module.items[0].kind else {
        panic!("expected trait");
    };
    assert_eq!(item_trait.associated_types.len(), 1);
    assert_eq!(item_trait.associated_values.len(), 1);
    assert_eq!(item_trait.associated_values[0].name, "Lanes");
    assert!(matches!(
        item_trait.associated_values[0].ty.kind,
        TypeKind::Path { .. }
    ));

    let (_, errors) = parse_module(
        r#"
trait Bad {
    comptime Value: usize = 1usize;
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("cannot have initializers")),
        "{errors:?}"
    );
}

#[test]
fn parses_structural_type_targets_after_if_statements() {
    let (module, errors) = parse_module(
        r#"
extend[T] &T {
    fn is_null(self) bool {
        self as usize == 0
    }
}

extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

fn zero() usize {
    0usize
}

fn main(ptr: &u8, triple: [3]i32) i32 {
    if [&u8]::is_null(ptr) {}
    [[3]i32]::first(triple) + zero() as i32
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[3].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    assert!(matches!(tail.kind, ExprKind::Binary { .. }));
}

#[test]
fn parses_index_before_field_as_index_not_generic_instantiation() {
    let (module, errors) = parse_module(
        r#"
fn main(items: &[i32], i: usize) i32 {
    items[i].value
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Field { lhs, .. } = &tail.kind else {
        panic!("expected field");
    };
    assert!(matches!(lhs.kind, ExprKind::BracketSuffix { .. }));
}

#[test]
fn parses_lowercase_generic_associated_call_with_colon_colon() {
    let (module, errors) = parse_module(
        r#"
struct box[T] {
    value: T,
}

extend[T] box[T] {
    fn make(value: T) box[T] {
        { value: value }
    }
}

fn main() box[i32] {
    box[i32]::make(1)
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[2].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected call");
    };
    let ExprKind::Qualified { lhs, .. } = &callee.kind else {
        panic!("expected qualified callee");
    };
    assert!(matches!(lhs.kind, ExprKind::BracketSuffix { .. }));
}
