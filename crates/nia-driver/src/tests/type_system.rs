// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::{
    CheckRequest, Driver, DriverError, DriverOutput, EmitLlvmRequest, NiaOptimizationLevel,
};
use nia_ids::ModuleId;

#[test]
fn error_union_structural_extend_supports_conversion_methods() {
    let root = temp_dir("error_union_structural_extend_supports_conversion_methods");
    write(
        &root.join("main.nia"),
        r#"
enum A: i32 {
    Bad = 1,
    _
}

enum B: i32 {
    Other = 2,
    _
}

fn to_b(error: A) B {
    _ = error;
    B::Other
}

extend[T] A!T {
    fn as_b(self) B!T {
        if !value = self {
            !value
        } or err! {
            to_b(err)!
        }
    }
}

fn fail() A!i32 {
    A::Bad!
}

fn main() i32 {
    if !value = fail().as_b() {
        value
    } or err! {
        err as i32
    }
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn checks_bodies_against_normalized_type_aliases() {
    let root = temp_dir("checks_bodies_against_normalized_type_aliases");
    write(
        &root.join("main.nia"),
        r#"
type Byte = u8;
fn id(x: Byte) u8 {
    x
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn checks_bodies_against_normalized_generic_type_aliases() {
    let root = temp_dir("checks_bodies_against_normalized_generic_type_aliases");
    write(
        &root.join("main.nia"),
        r#"
type RawPtr[T] = &T;
fn id(p: RawPtr[u8]) &u8 {
    p
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn struct_where_clause_constrains_nominal_type_arguments() {
    let root = temp_dir("struct_where_clause_constrains_nominal_type_arguments");
    write(
        &root.join("main.nia"),
        r#"
trait Marker {}

extend i32 : Marker {}

struct Box[T]
where T: Marker {
    value: &T,
}

fn main(value: &Box[bool]) void {
    _ = value;
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("trait bound not satisfied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_error_union_extension_method_infers_error_parameter_from_receiver() {
    let root =
        temp_dir("generic_error_union_extension_method_infers_error_parameter_from_receiver");
    write(
        &root.join("main.nia"),
        r#"
enum A: i32 {
    Bad = 1,
    _
}

enum B: i32 {
    Other = 2,
    _
}

trait IntoB {
    fn into_b(self) B;
}

extend A : IntoB {
    fn into_b(self) B {
        _ = self;
        B::Other
    }
}

extend[T, Source] Source!T
where Source: IntoB
{
    fn as_b(self) B!T {
        if !value = self {
            !value
        } or err! {
            err.into_b()!
        }
    }
}

fn fail() A!i32 {
    A::Bad!
}

fn main() i32 {
    if !value = fail().as_b() {
        value
    } or err! {
        err as i32
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_error_union_extension_method_infers_target_parameter_from_expected_return() {
    let root = temp_dir(
        "generic_error_union_extension_method_infers_target_parameter_from_expected_return",
    );
    write(
        &root.join("main.nia"),
        r#"
enum A: i32 {
    Bad = 1,
    _
}

enum B: i32 {
    Other = 2,
    _
}

trait Convert[Target] {
    fn convert(self) Target;
}

extend A : Convert[B] {
    fn convert(self) B {
        _ = self;
        B::Other
    }
}

extend[T, Source, Target] Source!T
where Source: Convert[Target]
{
    fn convert_error(self) Target!T {
        if !value = self {
            !value
        } or err! {
            err.convert()!
        }
    }
}

fn fail() A!i32 {
    A::Bad!
}

fn wrap() B!i32 {
    fail().convert_error()
}

fn main() i32 {
    if !value = wrap() {
        value
    } or err! {
        err as i32
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn supports_alias_to_pointer_extension_methods_without_void_cascades() {
    let root = temp_dir("supports_alias_to_pointer_extension_methods_without_void_cascades");
    write(
        &root.join("main.nia"),
        r#"
type RawPtr[T] = &T;

extend[T] RawPtr[T] {
    fn is_null(self) bool {
        self as usize == 0
    }
}

fn main(ptr: RawPtr[i32]) void {
    if ptr.is_null() {}
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        !program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot cast void to usize")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        !program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot cast <error type> to usize")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn supports_structural_extension_methods() {
    let root = temp_dir("supports_structural_extension_methods");
    write(
        &root.join("main.nia"),
        r#"
type RawPtr[T] = &T;

extend i32 {
    fn is_zero(self) bool { self == 0 }
}

extend void {
    fn unit(self) i32 { 1 }
}

extend[T] RawPtr[T] {
    fn is_null(self) bool { self as usize == 0 }
}

extend[T] [T] {
    fn size(& self) usize { self.len() }
}

extend[T] &[T] {
    fn ref_size(& self) usize { self.*.len() }
}

extend[T] [3]T {
    fn first(self) T { self[0] }
}

extend &fn(i32) i32 {
    fn apply(self, value: i32) i32 { self(value) }
}

fn inc(value: i32) i32 { value + 1 }

fn main(ptr: &i32, xs: & [i32], triple: [3]i32) i32 {
    if 0.is_zero() {}
    if ptr.is_null() {}
    {}.unit() + xs.size() as i32 + xs.ref_size() as i32 + triple.first() + (& inc).apply(1)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn type_alias_to_function_pointer_is_callable_and_assignable() {
    let root = temp_dir("type_alias_to_function_pointer_is_callable_and_assignable");
    write(
        &root.join("main.nia"),
        r#"
type StepFn = &fn(i32) i32;

struct Step {
    run: StepFn,
}

fn inc(value: i32) i32 {
    value + 1
}

fn call_direct(run: StepFn, value: i32) i32 {
    run(value)
}

fn main() i32 {
    let step = Step { run: &inc };
    call_direct(step.run, 1) + (step.run)(1)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn supports_for_discard_pattern_without_binding_local() {
    let root = temp_dir("supports_for_discard_pattern_without_binding_local");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {
    current: i32,
    end: i32,
}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

fn main() i32 {
    let mut total = 0;
    let mut iter = Counter { current: 0, end: 3 };
    for _ in iter {
        total += 1;
    }
    total
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn rejects_for_pointer_pattern_for_non_pointer_items() {
    let root = temp_dir("rejects_for_pointer_pattern_for_non_pointer_items");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        null
    }
}

fn main() void {
    let mut iter = Counter {};
    for &value in iter {}
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("for pattern requires value to be a read-only pointer")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn rejects_for_binding_type_annotations() {
    let root = temp_dir("rejects_for_binding_type_annotations");
    write(
        &root.join("main.nia"),
        r#"
fn main() void {
    for value: usize in 0..3 {}
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("for patterns do not support type annotations")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn rejects_unsized_slice_pointee_as_value_type() {
    let root = temp_dir("rejects_unsized_slice_pointee_as_value_type");
    write(
        &root.join("main.nia"),
        r#"
fn take(xs: [i32]) i32 {
    0
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("slice pointee types are unsized")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn supports_extension_method_specialization() {
    let root = temp_dir("supports_extension_method_specialization");
    write(
        &root.join("main.nia"),
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

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn concrete_slice_extension_is_more_specific_than_generic_slice_extension() {
    let root = temp_dir("concrete_slice_extension_is_more_specific_than_generic_slice_extension");
    write(
        &root.join("main.nia"),
        r#"
extend[T] [T] {
    fn rank(& self) i32 {
        1
    }
}

extend [char] {
    fn rank(& self) i32 {
        2
    }
}

fn main(text: &[char]) i32 {
    text.rank()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn concrete_trait_impl_is_more_specific_than_generic_slice_trait_impl() {
    let root = temp_dir("concrete_trait_impl_is_more_specific_than_generic_slice_trait_impl");
    write(
        &root.join("main.nia"),
        r#"
trait Show {
    fn show(& self) i32;
}

extend[T] [T] : Show
where T: Sized + Show
{
    fn show(& self) i32 {
        1
    }
}

extend [char] : Show {
    fn show(& self) i32 {
        2
    }
}

fn main(text: &[char]) i32 {
    text.show()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn concrete_trait_arg_impl_is_more_specific_than_generic_trait_arg_impl() {
    let root = temp_dir("concrete_trait_arg_impl_is_more_specific_than_generic_trait_arg_impl");
    write(
        &root.join("main.nia"),
        r#"
trait Convert[T] {
    fn convert(& self, value: T) i32;
}

struct Target {}

extend[T] Target : Convert[T]
where T: Sized
{
    fn convert(& self, value: T) i32 {
        _ = value;
        1
    }
}

extend Target : Convert[i32] {
    fn convert(& self, value: i32) i32 {
        value
    }
}

fn main(target: &Target) i32 {
    target.convert(2)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_slice_trait_impl_body_uses_element_trait_bound() {
    let root = temp_dir("generic_slice_trait_impl_body_uses_element_trait_bound");
    write(
        &root.join("main.nia"),
        r#"
trait Show {
    fn show(& self) i32;
}

extend i32 : Show {
    fn show(& self) i32 {
        self.*
    }
}

extend[T] [T] : Show
where T: Sized + Show
{
    fn show(& self) i32 {
        self[0].show()
    }
}

fn main(values: &[i32]) i32 {
    values.show()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_nominal_trait_impl_body_uses_element_trait_bound_with_slice_impls() {
    let root =
        temp_dir("generic_nominal_trait_impl_body_uses_element_trait_bound_with_slice_impls");
    write(
        &root.join("main.nia"),
        r#"
struct Formatter {}

trait Format {
    fn format(& self, formatter: &mut Formatter) void;
}

struct List[T] {
    value: T,
}

extend i32 : Format {
    fn format(& self, formatter: &mut Formatter) void {
        _ = formatter;
        _ = self;
    }
}

extend [char] : Format {
    fn format(& self, formatter: &mut Formatter) void {
        _ = formatter;
        _ = self;
    }
}

extend[T] [T] : Format
where T: Sized + Format
{
    fn format(& self, formatter: &mut Formatter) void {
        self[0].format(formatter);
    }
}

extend[T] List[T] : Format
where T: Sized + Format
{
    fn format(& self, formatter: &mut Formatter) void {
        self.value.format(formatter);
    }
}

fn main(list: List[i32], formatter: &mut Formatter) void {
    list.format(formatter);
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn reports_ambiguous_extension_method_specialization() {
    let root = temp_dir("reports_ambiguous_extension_method_specialization");
    write(
        &root.join("main.nia"),
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

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("ambiguous method `rank`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn supports_structural_extension_associated_functions() {
    let root = temp_dir("supports_structural_extension_associated_functions");
    write(
        &root.join("main.nia"),
        r#"
extend ! {
    fn nope(self) void {}
}

extend i32 {
    fn make() i32 { 0 }
}

fn main() i32 {
    [i32]::make()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("extend target must be an extendable value type")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        !program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("associated functions are not supported")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn reports_generic_type_argument_count_mismatches() {
    let root = temp_dir("reports_generic_type_argument_count_mismatches");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;
struct Point {}
struct Box[T] { value: T }
type Pair[T, U] = T;
fn missing_arg(a: Box) {}
fn extra_arg(a: Box[i32, bool]) {}
fn alias_missing_arg(a: Pair[i32]) {}
fn non_generic_arg(a: Point[i32]) {}
fn qualified_missing_arg(a: math::RemoteBox) {}
fn qualified_extra_arg(a: math::RemoteBox[i32, bool]) {}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub struct RemoteBox[T] {
    value: T,
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let mismatch_count = program
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("generic argument count mismatch")
        })
        .count();
    assert_eq!(mismatch_count, 6, "{:?}", program.diagnostics);
}

#[test]
fn checks_qualified_explicit_generic_function_calls() {
    let root = temp_dir("checks_qualified_explicit_generic_function_calls");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;
fn main(flag: bool) i32 {
    let mut x: i32 = math::id[i32](1);
    _ = math::id[i32](flag);
    x
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub fn id[T](value: T) T {
    value
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        !program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("binding initializer")
        }),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.diagnostic.summary.contains("call argument") })
    );
}

#[test]
fn checks_qualified_inferred_generic_function_calls() {
    let root = temp_dir("checks_qualified_inferred_generic_function_calls");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;
fn main(flag: bool) i32 {
    let mut x: i32 = math::id(1);
    _ = math::choose(1, flag);
    x
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub fn id[T](value: T) T {
    value
}

pub fn choose[T](left: T, right: T) T {
    left
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        !program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("binding initializer")
        }),
        "{:?}",
        program.diagnostics
    );
    assert!(program.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .summary
            .contains("conflicting inferred type for generic parameter `T`")
    }));
}

#[test]
fn collects_unique_monomorphized_instances() {
    let root = temp_dir("collects_unique_monomorphized_instances");
    write(
        &root.join("main.nia"),
        r#"
fn id[T](value: T) T {
    value
}

fn main() i32 {
    let mut a: i32 = id(1);
    let mut b: i32 = id(2);
    a + b
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert_eq!(program.monomorphization.instances.len(), 1);
}

#[test]
fn driver_options_flow_into_checked_program_and_backend_lowering() {
    let root = temp_dir("driver_options_flow_into_checked_program_and_backend_lowering");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    0
}
"#,
    );

    let path = root.join("main.nia").to_string_lossy().into_owned();
    let default_program = codegen_program(path.clone());
    assert!(
        default_program.diagnostics.is_empty(),
        "{:?}",
        default_program.diagnostics
    );
    assert_eq!(
        default_program.optimization,
        NiaOptimizationLevel::default().policy()
    );
    assert_eq!(
        default_program.backend_lowering.optimization,
        NiaOptimizationLevel::default().policy()
    );

    for level in [
        NiaOptimizationLevel::O0,
        NiaOptimizationLevel::O1,
        NiaOptimizationLevel::O2,
        NiaOptimizationLevel::O3,
        NiaOptimizationLevel::Os,
        NiaOptimizationLevel::Oz,
    ] {
        let program = codegen_program_with_options(path.clone(), level);

        assert!(
            program.diagnostics.is_empty(),
            "{level:?}: {:?}",
            program.diagnostics
        );
        assert_eq!(program.optimization, level.policy(), "{level:?}");
        assert_eq!(
            program.backend_lowering.optimization,
            level.policy(),
            "{level:?}"
        );
        assert_eq!(
            program
                .backend_lowering
                .optimization_report
                .enabled_global_passes,
            if level.policy().prefer_size
                || level
                    .policy()
                    .const_fold
                    .at_least(nia_opt::OptimizationDepth::Full)
            {
                vec!["simplify-static-init"]
            } else {
                Vec::new()
            },
            "{level:?}"
        );
    }
}

#[test]
fn driver_facade_emits_llvm_ir_from_source_request() {
    let root = temp_dir("driver_facade_emits_llvm_ir_from_source_request");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    42
}
"#,
    );

    let driver = Driver::new();
    let output = driver.emit_llvm_ir(EmitLlvmRequest::new(CheckRequest::new(
        root.join("main.nia").to_string_lossy().into_owned(),
    )));
    let artifact = output
        .result
        .expect("driver facade should emit llvm ir without diagnostics");

    assert!(
        artifact
            .modules
            .iter()
            .any(|module| module.ir.contains("define i32 @")),
        "{:?}",
        artifact.modules
    );
}

#[test]
fn driver_output_converts_internal_panics_to_diagnostics() {
    let output = DriverOutput::catch_ice(|| -> DriverOutput<()> {
        panic!("Nia ICE: forced driver failure");
    });

    let Err(DriverError::InternalDiagnostic(diagnostic)) = output.result else {
        panic!("expected internal diagnostic");
    };
    assert_eq!(diagnostic.code.as_str(), "I0001");
    assert!(diagnostic.summary.contains("forced driver failure"));
    let rendered = crate::render_driver_error(
        &DriverError::InternalDiagnostic(diagnostic),
        Some("main.nia"),
        Some("fn main() i32 { 0 }\n"),
    );
    assert!(rendered.contains("internal compiler error"), "{rendered}");
}

#[test]
fn driver_facade_formats_inspection_outputs() {
    let tokens = crate::tokens_inspection("fn main() i32 { 0 }\n");
    assert!(tokens.text.contains("Fn"));
    assert!(tokens.text.contains("0..2"));

    let ast = crate::ast_inspection("fn main() i32 { 0 }\n");
    assert!(ast.parse_errors.is_empty(), "{:?}", ast.parse_errors);
    assert!(ast.text.contains("Function"));

    let invalid = crate::ast_inspection("fn main(");
    assert!(!invalid.parse_errors.is_empty());
    let rendered = crate::render_parse_errors("main.nia", "fn main(", &invalid.parse_errors);
    assert!(rendered.contains("parse errors:"), "{rendered}");
    assert!(rendered.contains("main.nia"), "{rendered}");
}

#[test]
fn driver_facade_formats_optimization_report() {
    let root = temp_dir("driver_facade_formats_optimization_report");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    42
}
"#,
    );

    let program = codegen_program_from_output(Driver::new().codegen(CheckRequest::new(
        root.join("main.nia").to_string_lossy().into_owned(),
    )));

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let report = crate::optimization_report(&program);
    assert!(report.contains("backend optimization report:"), "{report}");
    assert!(report.contains("enabled_module_passes="), "{report}");
    assert!(report.ends_with('\n'), "{report:?}");
}

#[test]
fn driver_checks_in_memory_sources() {
    let driver = Driver::new();
    driver.set_source(
        "main.nia",
        r#"
fn main() i32 {
    7
}
"#,
    );

    let program =
        checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));

    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .modules
            .iter()
            .any(|module| module.path.as_str().ends_with("main.nia")),
        "{:?}",
        program.modules
    );
}

#[test]
fn driver_invalidates_reused_loader_sources() {
    let driver = Driver::new();
    driver.set_source("main.nia", "fn main() i32 { 1 }");

    let first =
        checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

    driver.set_source("main.nia", "fn main() i32 { true }");

    let second =
        checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));
    assert!(!second.diagnostics.is_empty());
}

#[test]
fn lowers_monomorphized_function_instances_with_readable_symbols() {
    let root = temp_dir("lowers_monomorphized_function_instances_with_readable_symbols");
    write(
        &root.join("main.nia"),
        r#"
fn id[T](value: T) T {
    value
}

fn main() i32 {
    id(1)
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let module = &program.backend_lowering.program.modules[0];
    assert_eq!(module.function_instances.len(), 1);
    let symbol = &module.function_instances[0].symbol;
    assert!(symbol.starts_with("nia__m0__d"), "{symbol}");
    assert!(symbol.contains("__id__inst__"), "{symbol}");
    assert!(symbol.contains("i32"), "{symbol}");
}

#[test]
fn allows_recursive_generic_instantiations_with_same_type_args() {
    let root = temp_dir("allows_recursive_generic_instantiations_with_same_type_args");
    write(
        &root.join("main.nia"),
        r#"
fn recurse[T](value: T) T {
    recurse[T](value)
}

fn main() i32 {
    recurse(1)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn allows_indirect_recursive_generic_instantiations_with_same_type_args() {
    let root = temp_dir("allows_indirect_recursive_generic_instantiations_with_same_type_args");
    write(
        &root.join("main.nia"),
        r#"
fn a[T](value: T) T {
    b[T](value)
}

fn b[T](value: T) T {
    a[T](value)
}

fn main() i32 {
    a(1)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn reports_polymorphic_recursive_generic_instantiations() {
    let root = temp_dir("reports_polymorphic_recursive_generic_instantiations");
    write(
        &root.join("main.nia"),
        r#"
fn grow[T](value: &T) i32 {
    grow[&T](&value)
}

fn main() i32 {
    let mut value = 1;
    grow[i32](&value)
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    let diagnostic = program
        .diagnostics
        .iter()
        .map(|diagnostic| &diagnostic.diagnostic)
        .find(|diagnostic| {
            diagnostic
                .summary
                .contains("generic instantiation did not converge")
        })
        .expect("generic instantiation convergence diagnostic");
    assert_eq!(diagnostic.code.as_str(), "E0601");
    assert!(
        diagnostic
            .primary_message()
            .is_some_and(|message| message.contains("limit"))
    );
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("already-seen concrete generic instance"))
    );
    assert!(
        diagnostic
            .help
            .iter()
            .any(|help| help.contains("finite set of concrete type arguments"))
    );
}

#[test]
fn computes_layouts_in_check_pipeline() {
    let root = temp_dir("computes_layouts_in_check_pipeline");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    a: u8,
    b: i32,
}

fn main(p: Pair) {}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main = program
        .modules
        .iter()
        .find(|module| module.id == ModuleId(0))
        .expect("main module");
    let pair_id = main
        .defs
        .module_scope
        .types
        .get(&sym("Pair"))
        .expect("Pair def");
    let pair_layout = main.layouts.structs.get(&pair_id).expect("Pair layout");
    assert_eq!(
        pair_layout.layout,
        nia_layout::TypeLayout { size: 8, align: 4 }
    );
    assert_eq!(pair_layout.fields[0].offset, 0);
    assert_eq!(pair_layout.fields[1].offset, 4);
}

#[test]
fn std_linux_statx_layout_matches_kernel_abi() {
    let root = temp_dir("std_linux_statx_layout_matches_kernel_abi");
    write(
        &root.join("main.nia"),
        r#"
using std::os;

fn main() usize {
    os::page_size()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let types_module = program
        .modules
        .iter()
        .find(|module| module.path.as_str().ends_with("std/os/linux/stat.nia"))
        .expect("std::os::linux::stat module");
    let statx_id = types_module
        .defs
        .module_scope
        .types
        .get(&sym("Statx"))
        .expect("Statx def");
    let statx_layout = types_module
        .layouts
        .structs
        .get(&statx_id)
        .expect("Statx layout");
    assert_eq!(
        statx_layout.layout,
        nia_layout::TypeLayout {
            size: 256,
            align: 8
        }
    );
}

#[test]
fn checks_cross_module_struct_literals() {
    let root = temp_dir("checks_cross_module_struct_literals");
    write(
        &root.join("main.nia"),
        r#"
module geom;
using entry::geom;

fn main() i32 {
    let mut p: geom::Point = { x: 40, y: 2 };
    p.x + p.y
}
"#,
    );
    write(
        &root.join("geom.nia"),
        r#"
pub struct Point {
    x: i32,
    y: i32,
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
