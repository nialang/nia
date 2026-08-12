// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn selects_most_specific_extension_method_target() {
    let checked = pipeline(
        r#"
extend[T] T {
    fn rank(self) i32 {
        1
    }
}

extend i32 {
    fn rank(self) i32 {
        2
    }
}

extend[T] &T {
    fn ptr_rank(self) i32 {
        3
    }
}

extend &i32 {
    fn ptr_rank(self) i32 {
        4
    }
}

fn main(value: &i32) i32 {
    1.rank() + value.ptr_rank()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn selects_fixed_array_length_over_const_generic_extension() {
    let checked = pipeline(
        r#"
extend[T, N: usize] [N]T {
    fn rank(&self) i32 {
        1
    }
}

extend[T] [2]T {
    fn rank(&self) i32 {
        2
    }
}

fn main(values: &[2]i32) i32 {
    values.rank()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn selects_repeated_type_parameter_over_independent_parameters() {
    let checked = pipeline(
        r#"
struct Pair[A, B] {
    left: A,
    right: B,
}

extend[A, B] Pair[A, B] {
    fn rank(self) i32 {
        1
    }
}

extend[T] Pair[T, T] {
    fn rank(self) i32 {
        2
    }
}

fn main(value: Pair[i32, i32]) i32 {
    value.rank()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn method_overload_selection_uses_argument_types() {
    let checked = pipeline(
        r#"
struct Box {}

extend Box {
    fn pick[T](self, value: T) T {
        value
    }
}

extend Box {
    fn pick(self, value: i32) i32 {
        value
    }
}

fn main(box: Box) bool {
    box.pick(true)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn invalid_method_overload_call_reports_no_match() {
    let checked = pipeline(
        r#"
struct Box {}

extend Box {
    fn pick(self, value: i32) i32 {
        value
    }
}

extend Box {
    fn pick(self, value: usize) usize {
        value
    }
}

fn main(box: Box) bool {
    box.pick(true)
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("no matching method overload `pick`")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().all(|diagnostic| !diagnostic
            .summary
            .contains("ambiguous method")
            && !diagnostic.summary.contains("unknown struct field")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn self_parameter_specializes_generic_method_parameter() {
    let checked = pipeline(
        r#"
struct Box {}

extend Box {
    fn merge[T](self, other: T) bool {
        true
    }
}

extend Box {
    fn merge(self, other: Self) i32 {
        1
    }
}

fn main(box: Box, other: Box) i32 {
    let result = box.merge(other);
    result
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn trait_method_selection_uses_argument_types() {
    let checked = pipeline(
        r#"
trait PickBool {
    fn pick(self, value: bool) bool;
}

trait PickInt {
    fn pick(self, value: i32) bool;
}

struct Box {}

extend Box : PickBool {
    fn pick(self, value: bool) bool {
        value
    }
}

extend Box : PickInt {
    fn pick(self, value: i32) bool {
        value != 0
    }
}

fn main(box: Box) bool {
    box.pick(true)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn attributes_provider_demand_to_calling_function() {
    let checked = pipeline_without_visible_extensions(
        r#"
struct Value {}

fn main(value: Value) i32 {
    value.missing()
}
"#,
    );
    let owned_provider_demands = checked
        .provider_demands_by_function
        .values()
        .flat_map(|demands| demands.iter().cloned())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(&owned_provider_demands, checked.provider_demands.as_ref());
    assert_eq!(checked.provider_demands_by_function.len(), 1);
    assert_eq!(checked.diagnostic_owners.len(), checked.diagnostics.len());
    assert!(checked.diagnostic_owners.iter().all(Option::is_some));
}

#[test]
fn error_receivers_do_not_emit_provider_demands() {
    let checked = pipeline_without_visible_extensions(
        r#"
fn main() () {
    missing.missing();
}
"#,
    );

    assert!(!checked.diagnostics.is_empty());
    assert!(
        checked.provider_demands.is_empty(),
        "{:?}",
        checked.provider_demands
    );
}

#[test]
fn method_candidate_expected_context_does_not_reject_nested_calls() {
    let checked = pipeline(
        r#"
struct Buffer {
    data: &[i32],
}

extend Buffer {
    fn as_slice(&self) &[i32] {
        self.data
    }
}

fn main(buffer: Buffer) i32 {
    buffer.as_slice()[0]
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn ref_method_receiver_lowering_preserves_pointer_typed_self_lhs() {
    let checked = pipeline(
        r#"
struct Counter {
    value: i32,
}

extend Counter {
    fn bump(&mut self) () {
        self.value += 1;
    }

    fn outer(&mut self) () {
        self.bump();
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let receiver = checked
        .ir
        .function_bodies
        .values()
        .filter_map(|body| match body.stmts.as_slice() {
            [
                nia_body_ir::TypedStmt {
                    kind:
                        nia_body_ir::TypedStmtKind::Expr(nia_body_ir::TypedExpr {
                            kind: nia_body_ir::TypedExprKind::Call { callee, .. },
                            ..
                        }),
                    ..
                },
            ] => match callee {
                nia_body_ir::TypedCallee::Method {
                    receiver_kind,
                    receiver,
                    ..
                } if *receiver_kind == ReceiverKind::Ref => Some(receiver),
                _ => None,
            },
            _ => None,
        })
        .next()
        .expect("outer self.bump() receiver");
    assert!(
        matches!(
            checked.type_store.get(receiver.ty),
            Some(TyKind::Pointer {
                is_readonly: false,
                ..
            })
        ),
        "expected ref method receiver typed as pointer, got {:?}",
        checked.type_store.get(receiver.ty)
    );
}

#[test]
fn mutable_receiver_uses_mutable_pointee_without_reassigning_pointer_binding() {
    let checked = pipeline(
        r#"
struct Counter {
    value: i32,
}

extend Counter {
    fn bump(&mut self) () {
        self.value += 1;
    }
}

fn main() i32 {
    let mut counter = Counter { value: 0 };
    let pointer = &mut counter;
    pointer.bump();
    counter.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn mutable_slice_can_call_readonly_pointee_extension_method() {
    let checked = pipeline(
        r#"
extend[T] [T]
where T: Sized
{
    fn inspect(&self) bool {
        true
    }
}

fn main(values: &mut [i32]) bool {
    values.inspect()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn pointer_arrays_can_call_slice_extension_methods() {
    let checked = pipeline_with_len_provider(
        r#"
trait SliceMetric {
    fn metric(&self) i32;
}

extend[T] [T]
where T: Sized
{
    fn itemCount(&self) usize {
        self.len()
    }

    fn replaceFirst(&mut self, value: T) () {
        self[0] = value;
    }

    fn choose[U](&self, value: U) U {
        value
    }

    fn storageKind(&self) i32 {
        1
    }
}

extend[T] [T] : SliceMetric
where T: Sized
{
    fn metric(&self) i32 {
        self.len() as i32
    }
}

extend[T] [2]T
where T: Sized
{
    fn storageKind(&self) i32 {
        2
    }
}

fn main() i32 {
    let literalCount = (&"nia").itemCount();
    let choices: [2]i32 = [1, 2];
    let selected = (&choices).choose[i32](7);
    let metric = (&choices).metric();
    let storageKind = (&choices).storageKind();
    let mut values: [2]i32 = [3, 4];
    (&mut values).replaceFirst(9);
    literalCount as i32 + selected + metric + storageKind + values[0]
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert_eq!(
        checked
            .facts
            .iter_node_pointer_array_to_slice_coercions()
            .count(),
        4
    );
}

#[test]
fn associated_function_and_field_access_use_extension_owner_type() {
    let checked = pipeline(
        r#"
struct CStr {
    ptr: &u8,
}

extend CStr {
    fn from_ptr(ptr: &u8) CStr {
        { ptr: ptr }
    }

    fn from_bytes(bytes: &[u8]) ?CStr {
        ?CStr::from_ptr(bytes.ptr())
    }

    fn raw_ptr(&self) &u8 {
        self.ptr
    }
}

fn main(bytes: &[u8]) ?&u8 {
    ?CStr::from_bytes(bytes).?.raw_ptr()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn reports_ambiguous_extension_method_specializations() {
    let checked = pipeline(
        r#"
struct Pair[A, B] {
    a: A,
    b: B,
}

extend[T] Pair[T, i32] {
    fn rank(self) i32 {
        1
    }
}

extend[U] Pair[i32, U] {
    fn rank(self) i32 {
        2
    }
}

fn main(pair: Pair[i32, i32]) i32 {
    pair.rank()
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("ambiguous method `rank`")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_assignment_targets_and_let_bindings() {
    let checked = pipeline(
        r#"
static global_let: i32 = 1;
static mut global_mut: i32 = 0;

struct Cell {
    value: i32,
}

fn main(param: i32, read: & i32, write: &mut i32, cell: Cell, read_cell: & Cell, write_cell: &mut Cell) i32 {
    let local_let = 1;
    let mut local_mut = 1;
    local_mut = 2;
    param = 3;
    _ += 1;
    global_mut = 4;
    local_let = 5;
    global_let = 6;
    read.* = 7;
    write.* = 8;
    cell.value = 9;
    read_cell.value = 10;
    write_cell.value = 11;
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("local is let"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("static is immutable"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("pointer is read-only"))
            .count(),
        1
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("local_mut"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("`_` discard only supports plain assignment")
    }));
}

#[test]
fn checks_method_calls_and_receiver_matching() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn get(& self) T {
        self.value
    }

    fn set(&mut self, value: T) {
        self.value = value;
    }
}

fn main(ro: & Box[i32], rw: &mut Box[i32]) i32 {
    let mut box: Box[i32] = { value: 1 };
    let mut x: i32 = box.get();
    let mut y: i32 = ro.get();
    rw.set(2);
    ro.set(3);
    box.set(true);
    box.get(1);
    x + y
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("receiver cannot be matched through read-only `&T`")
                || diagnostic
                    .summary
                    .contains("receiver is not assignable: local is let")
                || diagnostic.summary.contains(
                    "type mismatch in receiver argument: expected Box[i32], got &Box[i32]",
                )
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("argument count mismatch"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
}

#[test]
fn lowers_current_module_extension_self_fields_without_visible_extension_seed() {
    let checked = pipeline_without_visible_extensions(
        r#"
struct Init {
    argc: usize,
    argv: &&u8,
}

extend Init {
    pub fn argc(&self) usize {
        self.argc
    }

    pub fn argv(&self) &&u8 {
        self.argv
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked
            .ir
            .function_bodies
            .values()
            .all(|body| !typed_body_has_error_expr(body)),
        "{:?}",
        checked.ir.function_bodies
    );
}

#[test]
fn resolves_associated_functions_through_type_aliases() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

type Alias[T] = Box[T];

extend[T] Box[T] {
    fn init(value: T) Box[T] {
        { value: value }
    }
}

fn main() i32 {
    let box = Alias[i32]::init(42);
    box.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

fn typed_body_has_error_expr(body: &nia_body_ir::TypedBody) -> bool {
    body.stmts.iter().any(typed_stmt_has_error_expr)
        || body.tail.as_deref().is_some_and(typed_expr_has_error_expr)
}

fn typed_stmt_has_error_expr(stmt: &nia_body_ir::TypedStmt) -> bool {
    match &stmt.kind {
        nia_body_ir::TypedStmtKind::Binding(binding) => binding
            .value
            .as_ref()
            .is_some_and(typed_expr_has_error_expr),
        nia_body_ir::TypedStmtKind::PatternBinding(binding) => {
            typed_expr_has_error_expr(&binding.value)
        }
        nia_body_ir::TypedStmtKind::Expr(expr)
        | nia_body_ir::TypedStmtKind::Defer(expr)
        | nia_body_ir::TypedStmtKind::Return(Some(expr)) => typed_expr_has_error_expr(expr),
        nia_body_ir::TypedStmtKind::ForIn(for_in) => {
            typed_expr_has_error_expr(&for_in.iter) || typed_body_has_error_expr(&for_in.body)
        }
        nia_body_ir::TypedStmtKind::While(while_stmt) => {
            typed_expr_has_error_expr(&while_stmt.cond)
                || typed_body_has_error_expr(&while_stmt.body)
        }
        nia_body_ir::TypedStmtKind::Loop(loop_stmt) => typed_body_has_error_expr(&loop_stmt.body),
        nia_body_ir::TypedStmtKind::Return(None)
        | nia_body_ir::TypedStmtKind::Break
        | nia_body_ir::TypedStmtKind::Continue => false,
    }
}

fn typed_expr_has_error_expr(expr: &nia_body_ir::TypedExpr) -> bool {
    match &expr.kind {
        nia_body_ir::TypedExprKind::Error => true,
        nia_body_ir::TypedExprKind::Unary { expr, .. }
        | nia_body_ir::TypedExprKind::OptionalSome { expr }
        | nia_body_ir::TypedExprKind::ErrorOk { expr }
        | nia_body_ir::TypedExprKind::ErrorErr { expr }
        | nia_body_ir::TypedExprKind::Try { expr, .. }
        | nia_body_ir::TypedExprKind::Discard(expr)
        | nia_body_ir::TypedExprKind::Cast { expr, .. } => typed_expr_has_error_expr(expr),
        nia_body_ir::TypedExprKind::Binary { lhs, rhs, .. } => {
            typed_expr_has_error_expr(lhs) || typed_expr_has_error_expr(rhs)
        }
        nia_body_ir::TypedExprKind::Call { args, .. } => args.iter().any(typed_expr_has_error_expr),
        nia_body_ir::TypedExprKind::Field { lhs, .. } => typed_expr_has_error_expr(lhs),
        nia_body_ir::TypedExprKind::Block(body) => typed_body_has_error_expr(body),
        _ => false,
    }
}

#[test]
fn accepts_local_binding_declarations() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
    y: i32,
}

extend Point {
    fn inspect(& self) i32 { self.x }
    fn init(&self) {}
    fn deinit(&self) {}
}

fn main() {
    let mut p: Point;
    p.init();
    defer p.deinit();
    let origin: Point;
    _ = origin.inspect();
    let n: i32;
    let mut copied: i32 = n;
    let mut borrowed: & i32 = & n;
    _ = copied;
    _ = borrowed;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_mutating_let_uninitialized_bindings() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn init(&mut self) {}
}

fn main() {
    let origin: Point;
    origin.init();
    let n: i32;
    n = 1;
    _ = &n;
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic.summary.contains("receiver is not assignable")
                || diagnostic
                    .summary
                    .contains("reference target is not assignable")
                || diagnostic
                    .summary
                    .contains("assignment target is not assignable: local is let")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("local is let")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_explicit_generic_method_calls() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn replace[U](& self, value: U) U {
        value
    }

    fn get(& self) T {
        self.value
    }
}

fn main(flag: bool) i32 {
    let mut box: Box[i32] = { value: 1 };
    let mut x: i32 = box.replace[i32](2);
    let mut y: bool = box.replace[bool](flag);
    let mut z: i32 = box.get();
    _ = box.replace[i32](flag);
    _ = box.replace();
    _ = box.get[i32]();
    x + z
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("generic argument count mismatch for method"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("cannot infer generic parameter `U`"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn infers_method_generics_from_expected_return_type() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

struct EmptyBox[T] {}

extend[T] Box[T] {
    fn replace[U](& self, value: U) U {
        value
    }

    fn make[U](value: U) U {
        value
    }

}

extend[T] EmptyBox[T] {
    fn empty() EmptyBox[T] {}
}

fn main() i32 {
    let mut box: Box[i32] = { value: 1 };
    let mut a: usize = box.replace(1);
    let mut b: usize = Box[i32]::make(1);
    let mut c: EmptyBox[i32] = EmptyBox::empty();
    _ = c;
    a as i32 + b as i32
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_trait_method_impl_generics_from_method_arguments() {
    let checked = pipeline(
        r#"
trait Writer {}

trait Hash[H]
where H: Writer
{
    fn hash(&self, writer: &mut H) ();
}

struct H {}

extend H {
    fn init() H {
        {}
    }
}

extend H : Writer {}

extend[H] u32 : Hash[H]
where H: Writer
{
    fn hash(&self, writer: &mut H) () {
        _ = self;
        _ = writer;
    }
}

fn main() () {
    let mut hasher = H::init();
    (7u32).hash(&mut hasher);
}
"#,
    );
    assert!(
        checked.diagnostics.iter().all(|diagnostic| !diagnostic
            .summary
            .contains("conflicting inferred type")
            && !diagnostic
                .summary
                .contains("type mismatch in call argument")
            && !diagnostic
                .summary
                .contains("cannot infer generic parameter")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_function_pointer_calls() {
    let checked = pipeline(
        r#"
fn main(cb: &fn(i32, bool) i64, variadic: &fn(i32, ...) (), flag: bool) i64 {
    let mut x: i64 = cb(1, flag);
    _ = cb(flag, flag);
    _ = cb(1);
    variadic(flag, 1);
    x
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("call argument"))
            .count(),
        2
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("argument count mismatch"))
            .count(),
        1
    );
}

#[test]
fn checks_associated_method_function_pointers() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn new(x: i32) Point {
        { x: x }
    }

    fn get(& self) i32 {
        self.x
    }

    fn set(&self, value: i32) {
        self.x = value;
    }
}

fn main() i32 {
    let make: &fn(i32) Point = & Point::new;
    let get: &fn(& Point) i32 = & Point::get;
    let set: &fn(&Point, i32) () = & Point::set;
    let mut p = make(1);
    set(&p, 2);
    get(& p)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_generic_associated_method_function_pointers() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }

    fn replace[U](& self, value: U) U {
        value
    }
}

fn main(flag: bool) i32 {
    let make: &fn(i32) Box[i32] = & Box[i32]::make;
    let replace: &fn(& Box[i32], bool) bool = & Box[i32]::replace[bool];
    let mut b = make(1);
    if replace(& b, flag) { b.value } else { 0 }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_structural_associated_calls_and_function_pointers() {
    let checked = pipeline(
        r#"
extend[T] &T {
    fn is_null(self) bool {
        self as usize == 0
    }

    fn zero() usize {
        0usize
    }
}

extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

fn main(ptr: &u8, triple: [3]i32) i32 {
    let is_null: &fn(&u8) bool = & [&u8]::is_null;
    let zero: &fn() usize = & [&u8]::zero;
    if is_null(ptr) {}
    if [&u8]::is_null(ptr) {}
    [[3]i32]::first(triple) + zero() as i32
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_deep_pointer_structural_associated_calls_and_function_pointers() {
    let checked = pipeline(
        r#"
extend &&&&&& &&i32 {
    fn is_null(self) bool {
        self as usize == 0
    }
}

fn main(ptr: &&&&&& &&i32) bool {
    let is_null: &fn(&&&&&& &&i32) bool = & [&&&&&& &&i32]::is_null;
    is_null(ptr) and [&&&&&& &&i32]::is_null(ptr)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_associated_method_function_pointer_errors() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }

    fn replace[U](& self, value: U) U {
        value
    }
}

fn main() {
    let make: &fn(i32) Box[i32] = & Box::make;
    let bad_replace: &fn(& Box[i32], bool) bool = & Box[i32]::replace;
}
"#,
    );
    assert!(
        checked.diagnostics.iter().all(|diagnostic| !diagnostic
            .summary
            .contains("generic argument count mismatch for `Box`")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("generic argument count mismatch for function pointer")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_associated_function_calls() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn new(x: i32) Point {
        { x: x }
    }

    fn get(& self) i32 {
        self.x
    }
}

fn main(flag: bool) i32 {
    let mut p = Point::new(1);
    let mut value: i32 = Point::get(&p);
    _ = Point::new(flag);
    _ = Point::new();
    _ = Point::get();
    p::get();
    value
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("argument count mismatch"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("receiver method `get` requires")
    }));
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("qualified access is not a value expression")
    }));
}

#[test]
fn checks_unqualified_associated_function_calls_inside_extension_methods() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn helper() i32 {
        1
    }

    fn value(&self) i32 {
        helper()
    }
}

fn main() i32 {
    let point = Point { x: 0 };
    point.value()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_unqualified_associated_function_calls_inside_generic_extension_methods() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn wrap(value: T) Box[T] {
        { value: value }
    }

    fn copy(&self) Box[T] {
        wrap(self.value)
    }
}

fn main() i32 {
    let boxed = Box[i32] { value: 7 };
    boxed.copy().value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn module_function_takes_precedence_over_extension_helper_in_method_body() {
    let checked = pipeline(
        r#"
struct S {}

fn helper() i32 {
    10
}

extend S {
    fn helper() i32 {
        1
    }

    fn method(&self) i32 {
        helper()
    }
}

fn main() i32 {
    let value = S {};
    value.method()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn local_callable_takes_precedence_over_extension_helper_in_method_body() {
    let checked = pipeline(
        r#"
struct S {}

fn chosen() i32 {
    10
}

extend S {
    fn helper() i32 {
        1
    }

    fn method(&self) i32 {
        let helper = &chosen;
        helper()
    }
}

fn main() i32 {
    let value = S {};
    value.method()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_generic_type_prefix_associated_function_calls() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }

    fn empty() Box[T] {
        { value: 0 }
    }
}

fn main(flag: bool) i32 {
    let mut a: Box[i32] = Box[i32]::make(1);
    _ = Box[i32]::make(flag);
    _ = Box[i32, bool]::make(1);
    _ = Box::empty();
    a.value
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("generic argument count mismatch for `Box`")
    }));
}

#[test]
fn checks_lowercase_generic_type_prefix_associated_function_calls() {
    let checked = pipeline(
        r#"
struct box[T] {
    value: T,
}

extend[T] box[T] {
    fn make(value: T) box[T] {
        { value: value }
    }
}

fn main() i32 {
    let mut a: box[i32] = box[i32]::make(1);
    a.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn receiver_method_calls_take_precedence_over_function_pointer_fields() {
    let checked = pipeline(
        r#"
type RunFn = &fn() i32;

struct MethodBox {
    run: RunFn,
}

struct FieldBox {
    run: RunFn,
}

fn field_run() i32 {
    1
}

extend MethodBox {
    fn run(self) i32 {
        _ = self;
        2
    }
}

fn main() i32 {
    let method_box = MethodBox { run: &field_run };
    let field_box = FieldBox { run: &field_run };
    method_box.run() + field_box.run()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let mut saw_method = false;
    let mut saw_function_pointer = false;
    for (_, call) in checked.facts.iter_node_resolved_calls() {
        match call {
            nia_sema_ir::ResolvedCall::Method { .. } => saw_method = true,
            nia_sema_ir::ResolvedCall::FunctionPointer => saw_function_pointer = true,
            _ => {}
        }
    }
    assert!(
        saw_method,
        "{:?}",
        checked.facts.iter_node_resolved_calls().collect::<Vec<_>>()
    );
    assert!(
        saw_function_pointer,
        "{:?}",
        checked.facts.iter_node_resolved_calls().collect::<Vec<_>>()
    );
}
