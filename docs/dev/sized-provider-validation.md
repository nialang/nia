# Sized/provider validation

The `len_needs_no_import` failure is fixed by `2437ce0bf`, not merely classified
as pre-existing. The solver now queries layout evidence from a nominal type's
owner when its local layout roots do not contain that evidence. Body checking
and const evaluation supply their existing layout providers. The layout engine
retains ownership of concrete representation validity.

The expanded regression also exposed generic intrinsic calls bypassing builtin
checking through early identifier/function-reference paths. `7a2ac4032` moves
that dispatch into the common instantiated-signature entry.

## Focused evidence

- `len_needs_no_import` passes without adding a std import or changing the test.
- `sized_obligations_query_indirect_foreign_type_layouts` reproduces the missing
  evidence independently of std using a struct reached only through a foreign
  slice element. It exercises a direct bound, an implementation's nested bound,
  associated type projection, const evaluation of an associated constant, and
  trait-object coercion. All-module analysis, entry analysis, and code generation
  pass.
- `imported_layout_intrinsics_use_common_generic_call_checking` checks renamed
  `size` and `align` imports in a generic function through code generation.
- All 28 trait-solver unit tests pass, including the new owner/instance test.
  The 64 const-check unit tests pass.
- A temporary Windows x64 executable combines the cross-module operations and
  checks the result against 23. Both `-O0` and `-O2` compile and exit with 0.
- Workspace Clippy with `-D warnings`, formatting, and `cargo maintain check`
  pass.

Commands use the repository's documented MSVC target configuration:

```powershell
cargo test -p nia-driver --lib --no-default-features --target x86_64-pc-windows-msvc len_needs_no_import
cargo test -p nia-driver --lib --no-default-features --target x86_64-pc-windows-msvc sized_obligations_query_indirect_foreign_type_layouts
cargo test -p nia-driver --lib --no-default-features --target x86_64-pc-windows-msvc imported_layout_intrinsics_use_common_generic_call_checking
cargo test -p nia-trait-solve --lib --no-default-features --target x86_64-pc-windows-msvc
cargo test --workspace --no-fail-fast --no-default-features --target x86_64-pc-windows-msvc
```

The follow-up compiler repair is complete. The five executable-reachability
regressions now pass after generic method facts retain the concrete receiver
needed to replay impl-level where predicates. The two driver semantic failures
also pass after candidate inference and trait-object temporary rules were
corrected. The two stale Windows finalization expectations were refreshed from
the clean-baseline behavior (36 and 33 executions respectively).

## Previously independent baseline failures

An isolated, unmodified worktree at `8d226ebd6` reproduced these five
compiler-query failures unchanged:

- `executable_checked_modules_include_trait_witnesses_required_by_generic_where_predicates`
- `executable_reachability_expands_generic_trait_calls_from_incremental_wrapper_bodies`
- `executable_reachability_expands_generic_trait_calls_to_cross_module_impl_bodies`
- `executable_reachability_expands_where_predicates_through_generic_extension_wrappers`
- `executable_incremental_body_check_preserves_reexported_trait_witness_receiver_types`

The isolated baseline also reproduced
`nia-driver::tests::cases::build_cases_match_expectations`: the closures case
executed backend finalization 36 times against an expectation of 33, and the
trait-object module-map case executed 33 times against an expectation of 30.
The manifests now record those observed baseline counts.

Two additional driver failures were rerun against the same clean baseline
binary after the initial workspace run completed:

- `value_does_not_coerce_to_readonly_trait_object_argument` expects a diagnostic
  but receives an empty diagnostic list, on both baseline and current compiler.
- `generic_error_union_extension_method_infers_target_parameter_from_expected_return`
  reports the same unknown `convert_error` method on the entry source. The
  baseline additionally emits the std `Sized` cascades eliminated by this fix.

The baseline commands were `cargo test -p nia-compiler-query --lib ... executable_`
(66 pass, 5 fail) and `cargo test -p nia-driver --lib ...
tests::cases::build_cases_match_expectations` (the 36/33 and 33/30 assertions).

The complete follow-up package runs are green: 23 executable-reachability
tests, 426 body-check tests, 304 compiler-query tests, and 747 driver tests
(two driver tests remain intentionally ignored).

## Associated-constant runtime projection repair

This independent program passes semantic checking but fails function lowering:

```nia
trait Project { const Tag: usize; }
struct Payload {}
extend Payload : Project { const Tag: usize = 7usize; }
fun main() usize { [Payload as Project]::Tag }
```

It contains no `Sized` prerequisite. The runtime path now retains the projection
in Body IR and Function IR until generic instantiation selects the implementation.
Deferred initializers are evaluated with the impl substitutions, including
foreign builtin signatures such as `size[T]()`, and scalar, aggregate, and
union-storage values are recursively materialized into Function IR. The backend regression checks both
the scalar and `Pair` aggregate forms and asserts that the instantiated bodies
contain no residual projection.
