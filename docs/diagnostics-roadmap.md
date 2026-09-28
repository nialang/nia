<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Diagnostics Roadmap

Nia's terminal diagnostics are a compiler product, not a last-mile string
formatter. A useful report must answer four questions in order:

1. What source construct is wrong?
2. Which compiler rule rejected it?
3. What other location or declaration explains the rule?
4. What can the user change to continue?

The implementation was upgraded in bounded stages. Each stage must keep
the existing source spans and diagnostic codes stable unless the old contract
was actively misleading.

Status: the bounded implementation, workspace acceptance and local Git delivery
are complete as of 2026-09-28. All acceptance-ledger rows are verified.
Completed examples are evidence for their own cases, not proof that an entire
rule family is complete. Non-Linux interactive-terminal validation belongs to
the 0.3.0 multi-platform port and is outside this roadmap's closure criteria.

## Acceptance Ledger

This ledger records the completed work list. `Verified` means the stated boundary has
owner tests and end-to-end evidence; `Open` means the boundary still needs an
audit or implementation. A row may close only against its stated criteria.
Newly discovered gaps receive a named row instead of extending an unbounded
claim of "broader coverage". Progress is tracked by these boundaries, not an
estimated percentage or the number of registered diagnostic codes.

| ID | State | Boundary and owner | Required evidence for closure |
| --- | --- | --- | --- |
| G1 | Verified | Grammar ownership: `nia-syntax`, `nia-parser` | One production grammar; pure AST lowering; old parser/crate and migration markers removed; recovery and parser/driver parity tests. |
| G2 | Verified | Incremental edits: syntax, query, loader | Clean/edit equivalence of syntax, diagnostics and AST origins; actual green sharing; stale/UTF-8 edit rejection; failed publication retry; bounded revision storage. |
| S1 | Verified | Failed callable selection: `nia-body-check` | Unknown, inaccessible, ambiguous and rejected-overload calls retain independent value-argument errors exactly once. Generic trait ambiguity stops field fallback. Name-resolution lookup labels exclude value arguments. Owner matrices and `fail/failed_callable_arguments` verify the behavior; callable-field success/failure controls remain valid. |
| S2 | Verified | Qualified lookup and visibility: value/type resolution, body check | `qualified_callable_visibility_matrix` covers four visibilities across same-module, parent, sibling, same-package and external-package consumers, with ordinary/explicit-generic calls, plain/generic function references, and type references. Both free and associated functions retain dependency-owned declaration locations. Parser tests pin `self::` type paths; recovery tests prevent failed generic callees from misdiagnosing type arguments as values. |
| S3 | Verified | Candidate explanations: `nia-body-check` | Associated, bounded-receiver and dynamic-trait ambiguities retain candidate identities, declaration locations and actionable help. Owner tests cover ordinary parameters, const-generic identities, sorted truncation across reversed discovery order, and successful narrowing controls. `fail/trait_candidate_locations`, `fail/ambiguous_extension_method` and `fail/failed_callable_arguments` pin rendered local and cross-module evidence. |
| P1 | Verified | Recovery cause identity: resolution, diagnostic bundles, compiler reports | Aliased/group imports and repeated value/type/namespace consumers preserve the emitted root through bundles, edits and caches. Block scopes, const evaluation, executable lookup, private/public imports and unavailable loader hosts have owner and driver coverage; independent errors remain visible. Bundle/report tests cover missing, duplicate and cyclic cause evidence. |
| P2 | Verified | Generated/source ownership: build runner and driver | Runner filtering preserves dependency roots and attaches only explicitly proven generated context. Checked/codegen reports retain diagnostic source snapshots through artifacts; dependency overlays, cold/cached/verified checks, LLVM/object preparation failures, missing sources and old reports after edits/file removal are covered by `report_sources` and runner owner tests. |
| R1 | Verified | Recovery rule matrix: semantic owners | The bounded frontend-family inventory below covers direct/structural Error recovery, independent same-expression/function errors and resolved invalid controls. Owner and driver matrices plus aggregate, receiver and nested-required-type snapshots cover the discovered gaps. Const/static duplicate attribution remains explicitly owned by R2. |
| R2 | Verified | Const/static diagnostic precedence: const checker, body checker, query reports | Same-span generic const-call and static-data consequences are coalesced while E0501, specialized E0401 roots and independent initializer errors remain. Owner and driver tests cover generic constraints, embedded-file failures, range errors, and cold/edit/cache equivalence. |
| O1 | Verified | Operational report matrix: CLI, build, linker/backend | CLI text/JSON matrices and owner fault-path tests retain process stages, both available bounded tails, target roles, suite context and cleanup causes. Both capture workers are retired on failure. The plan/process inventory below maps normal CLI failures and explicit executor/OS-level exclusions. |
| C1 | Verified | Final acceptance and delivery | The maintained explicit-target Windows workspace gate passes 3,835 tests including doctests, with 27 ignored; strict Clippy, formatting and maintenance pass. Accepted changes are organized into dependency-ordered local commits with intermediate build checks. Linux terminal evidence remains separate; durable contracts live with their owners. |

### Current Delivery

- [x] O1 final owner checks: `nia-build` passes 141 tests (26 platform/helper
  tests ignored); CLI `build_cases` passes all 14 tests, including 28 invalid-plan
  command/format combinations, and `diagnostic_failures` passes all eight tests.
  Strict workspace/all-target Clippy, maintenance and formatting pass.
  Native checks use `--target x86_64-pc-windows-msvc`, the official MSVC developer
  environment, and no custom `RUSTFLAGS`, `CFLAGS` or `CXXFLAGS`.
- [x] Final Windows workspace test on 2026-09-28, after the O1 repairs:
  `cargo test --locked --workspace --no-default-features --target x86_64-pc-windows-msvc -- --test-threads=1`
  completed with exit status 0: 3,835 passed, zero failed, 27 ignored, including
  documentation tests. This uses the maintained explicit-target entry point
  and the developer environment without custom Rust/C/C++ flags.
- [x] Separate ordinary-shell evidence: all 104 `nia-llvm` tests pass with the
  explicit target and no `LIB`, `INCLUDE`, `RUSTFLAGS`, `CFLAGS`, or `CXXFLAGS`.
  This covers native object emission and LTO without hardcoded SDK paths.
  Workflow contracts pass all five tests; the compiler's PE imports contain
  no dynamic CRT DLL. These local results are not a hosted CI run or new Linux
  terminal evidence.
- [x] C1 delivery on 2026-09-28: `68d4be598` records Windows toolchain/CRT
  setup, `984790287` the syntax/parser/loader migration, `a7b19f681` semantic
  recovery and source ownership, and `c397d2b5e` build/CLI failure reports.
  Architecture and roadmap closeout accompany the series. Parser and semantic
  intermediate trees pass locked workspace/all-target checks; the Windows
  intermediate tree passes all five workflow contracts. The final implementation
  matches the accepted source snapshot. These are local commits, not a push or
  a new hosted CI run.
- [x] Record G1/G2 implementation and verification. Their accepted conservative
  reparse boundaries are explicit below.
- [x] S1: reproduce skipped independent value-argument errors on failed callable
  selection, fix the owning checker paths, and add owner tests plus snapshots.
  The new source fixture preserves 16 diagnostics across eight failed calls;
  three nested argument errors were previously suppressed by full-call lookup
  labels. Trait ambiguity no longer adds a spurious field-access error.
- [x] Record S1 coverage and P1's producer/consumer inventory below. Current
  workspace evidence is centralized here rather than repeating historical
  test counts throughout the roadmap.

Full-source lexing and conservative declaration fallback for unsafe block edits
are accepted G2 boundaries. Incremental lexing and additional reparse islands
are performance follow-ups, not hidden requirements for closing this roadmap.

### Evidence Matrix

The source fixture root is `crates/nia-driver/tests/cases/check`. Each failure
snapshot includes structured fields and rendered text. Existing evidence is
reused; an uncovered combination remains open even when its error code already
has another fixture.

| Contract | Existing evidence | Next acceptance boundary |
| --- | --- | --- |
| Call arguments and recovery | `fail/call_diagnostics`, `fail/ambiguous_extension_method`, `fail/failed_callable_arguments`, `fail/trait_candidate_locations`, body-check call/method matrices | S1/S3 verified, including deterministic candidate limits and successful narrowing controls. |
| Qualified names and scope | `qualified_callable_visibility_matrix`, `fail/qualified_callable`, `fail/qualified_associated_callable_visibility`, `fail/restricted_qualified_visibility`, `fail/restricted_associated_values` | S2 verified across lookup forms, visibility scopes and cross-module declaration ownership. |
| Import cause chains | `fail/using_type_provenance`, `fail/private_using`, diagnostic bundle/report tests | P1 verified for aliases, value/type consumers, repeated edits and persistence; the producer/consumer inventory follows below. |
| Structural recovery | `fail/structural_recovery_diagnostics`, `fail/operator_recovery_diagnostics`, builtin/propagation fixtures, `fail/nested_required_type_recovery` | R1 verified against the family inventory; R2 owns overlapping const/static causes. |
| Generated and operational failures | Build-runner ownership tests, CLI captured-report and linker/backend tests | P2/O1 verified against the ownership, command/output and final fault-path inventories below. |

Current gate results belong to Current Delivery above. Owner evidence below
defines each accepted boundary rather than relying on a historical workspace
test count. Invalid type recovery exposed legacy `extend !` test inputs; these
semantic tests now use the specified `never` spelling and retain their original
semantic assertions. Final acceptance and Git delivery are recorded under C1 above.

### P1 Cause Evidence Audit

The verified module-import ownership chain is recorded below. Block-local
imports, unavailable hosts and edit/cache behavior have the owner and driver
evidence listed after the inventory.

| Step | Current owner and entry point | Boundary to verify |
| --- | --- | --- |
| Emit the import root | `nia-public-surface`: `using_failure_diagnostic`, `compute_using_scopes_from_surfaces_with_symbols` | The referenced path/code/span belongs to the root actually emitted; public re-export and missing-module handling must not invent a duplicate root. |
| Retain lookup evidence | `nia-defs::UnresolvedUsing`; `nia-public-surface::record_unresolved_using_names` | An alias changes the exposed name and selected-name span, while its cause remains the original diagnostic identity. Group selectors retain per-name evidence. |
| Consume types | `nia-type-resolve::unresolved_using_type_diagnostic` | Parameter, result and nested type uses retain the same cause and declaration-owned related path. |
| Consume values and namespaces | `nia-value-resolve::unresolved_using_path_diagnostic`, `node_unresolved_usings`; `nia-body-check::unresolved_using_diagnostic` | Qualified namespaces and deferred body values retain evidence without publishing a second unrelated unknown-name root. |
| Persist diagnostic evidence | `nia-diagnostic`: stable bundle encode/decode | Cause and related paths survive roundtrip; edited or relocated sources do not reuse stale locations. |
| Suppress and order reports | Compiler `diagnostic_has_root_evidence`; diagnostic `build_diagnostic_report_with_downstream` | Exact cause links remain valid; span fallback cannot hide independent errors. Missing, ambiguous, self-referential and cyclic causes remain visible instead of being dropped. |

P1 exercises a failed aliased/group import used as both a value and a type,
plus an independent same-function error, then edits the import from failing to
valid and back. Clean, incrementally updated and cached products agree.
Bundle/report tests cover serialization and missing/cyclic cause handling in
isolation; the combined source-driven path below verifies their composition.
Extend the existing typed evidence only when a failing source case demonstrates
information loss at an identified owner; do not introduce a parallel origin store.

The driver test `aliased_group_import_causes_survive_edits_and_cache` now traces
two failed grouped aliases through repeated type and value uses in a child
module, preserving four cause links and one independent body error. It compares
diagnostics and rendered reports after failure, repair, and failure at moved
source offsets against clean, cached and cache-verified drivers. Emitted
diagnostics also roundtrip through the stable bundle. This boundary passes.

P1 is verified. Block-local imports share resolved lexical environments
across type/value resolution, deferred const expressions, const lowering and
executable reachability. The five `block_imports` driver tests cover nested
and module-definition shadowing, sibling/outside rejection, type/value namespace
separation, wildcard and enum-alias imports, duplicate import locations, const
array lengths and codegen. Failed qualified namespaces retain the import root.
Failure, repair, moving an import into a child block and shifted failure are
compared against clean, cached and cache-verified checks, including rendered
reports. `private_import_and_reexport_causes_identify_the_emitted_root` checks
private module/block imports and public re-exports with repeated consumers and
dependency-owned declarations. `unavailable_import_module_keeps_the_loader_root`
checks both consumers against the actual loader root without adding an import
root. Query composition attaches that cause only when the failed module's
declaration matches exactly one published loader diagnostic. The focused import
audit passes 15 tests; workspace acceptance is recorded in Current Delivery.
Module aliases also take precedence over same-named declared child modules in
their block, including subsequent import hosts and qualified type/value paths.
A failed same-named alias cannot fall back to the original child module.

Block-import implementation must share lexical lookup results across type and
value resolution, including deferred const expressions and executable
reachability. Module-only query keys cannot distinguish nested aliases. Tests
must include a block and its children, sibling/outside rejection, alias shadowing,
duplicate names in one namespace/scope, imported enum variants, and failure
causes after edits. Ordinary `check` success alone does not establish the
executable-query path.

The lexical owner test also checks restoration of an outer import after an
inner failed alias, absence in sibling functions and preservation of an imported
enum variant's parent identity. Modules with no block imports reuse their module
scope; signature-const queries use their restricted item tree rather than adding
ordinary function bodies to signature dependencies.

### P2 Ownership Audit

The runner filter now removes only diagnostics owned by the generated runner
when a source error exists. It preserves independent dependency errors and
warnings, including when the build script itself also fails. Generated context
is attached as a related location only when an explicit path/code/span cause
identifies exactly one source root. Without source errors, runner errors remain
visible. `runner_focus_preserves_dependency_roots_and_attaches_only_proven_context`
and `build_script_errors_hide_generated_runner_cascade` cover this boundary.
The build renderer uses the retained runner text for generated related locations;
`runner_related_locations_use_generated_source_instead_of_caller_text` pins its
source ownership in text and JSON.

P2 source snapshots are implemented. Compiler query products retain the source
text for primary and related diagnostic paths, including explicit unavailability,
and the driver carries this data through preparation failures and emitted
artifacts. The renderer uses retained text before caller overrides or disk.
`diagnostic_reports_retain_dependency_overlays_across_edits_and_cache` checks
dependency overlays against unchanged disk files, cold/cached/cache-verified
checks, codegen and LLVM/object preparation failures, and retained reports after
repair and dependency file removal. `unavailable_report_sources_do_not_read_later_files`
ensures later files cannot change reports produced with missing dependencies.

### R1 Aggregate Recovery Audit

`aggregate_recovery_keeps_independent_initializer_errors` covers direct Error,
generic/pointer/array/tuple structural Error, wrong nominal context, absent
expected context and nested aggregate literals. Rejected aggregate targets still
check field expressions with a recovery expectation, retaining independent call
argument failures without adding a dependent nominal-shape error. Required type
arguments use the existing type-or-const validation path, so `Box[Missing]`
cannot silently become Error. `required_type_arguments_report_unresolved_candidates`
checks its root span and a valid const-generic control.
Array and tuple expressions now retain an Error expectation for nested
constructors instead of discarding it during element inference. Rejected array
contexts still check element expressions. The 14-case aggregate matrix checks
these boundaries, and `fail/aggregate_recovery` pins structured and rendered
reports for arrays, tuples, generic unions, and tuple/named enum payloads: five
source/type roots and five independent argument errors, without constructor
shape consequences.
The family matrix below records R1's accepted boundaries and controls.

The driver `recovery_families_keep_independent_expression_errors` now covers
67 direct/structural recovery combinations across calls/inference, unary and
binary operators, projections, bindings, statements, iteration, propagation,
SIMD/memory/atomic builtins, inline assembly, closures and nine match-pattern
forms. Each case requires exactly one unresolved-value root and one independent
argument error, including its precise source span. Thirteen further cases in
`recovery_owner_boundaries_keep_independent_body_errors` check parameter/result
types, returns, closure annotations and error-union payloads.

The audit exposed structural receiver method lookup publishing unknown-method
and field-shape cascades. Failed lookup now recognizes recursive Error types,
checks independent value and ambiguous bracket arguments, and avoids provider
demands for recovery receivers. Existing candidates still use normal signature
checking. Eight owner cases cover ordinary/explicit-generic calls on direct,
pointer, tuple and array recovery receivers; `fail/recovered_method_receivers`
pins the structured and rendered reports. Static-data lowering also refuses
initializers already rejected by expression checking, without publishing a
recovery initializer or another representation error.

The audit is bounded by these frontend families. Direct Error, structural Error,
same-function independent errors and resolved invalid controls are accounted for
by the evidence below. Const/static duplicate attribution is separate from
retention and is verified separately by R2 below.

| Family | Existing evidence | Accepted boundary and controls |
| --- | --- | --- |
| Callable selection and generic inference | S1/S3 matrices; structural inference owner tests; 67-case driver matrix and recovered-receiver snapshot | `known_method_on_recovered_nominal_receiver_keeps_fixed_argument_error` retains the fixed-parameter failure on pointer/tuple generic arguments containing Error. |
| Aggregate construction and required type arguments | `aggregate_recovery_keeps_independent_initializer_errors`; `required_type_arguments_report_unresolved_candidates`; `fail/aggregate_recovery` | Covered for direct/structural targets, union and enum payloads, nested constructors and independent initializer errors. |
| Operators and trait obligations | Shift/operator fixtures, trait/unary owner tests, driver direct/structural pairs with independent operands | `rejects_unary_operators_without_builtin_trait_impls`, `rejects_boolean_vector_arithmetic`, `checks_vector_shift_operand_shapes` and `suppresses_recovery_trait_obligation_diagnostics` retain resolved invalid controls. |
| Places, projections and references | Structural place owner tests; driver direct/structural pairs and independent index operands | Covered for field/index/deref/reference recovery. |
| Bindings, patterns, statements and iteration | Structural fixtures and owner tests; nine pattern forms with independent arm errors in the driver matrix | Covered for the enumerated pattern, binding, defer, condition and iteration forms. |
| Optional/error-union construction and propagation | Propagation fixtures; driver direct/structural construction and propagation pairs | Covered for the enumerated operand, payload and return boundaries. |
| Layout, SIMD, atomic and memory builtins | Dedicated owner suites; driver independent SIMD lane/atomic value argument checks | `recovery_builtins_keep_independent_configuration_errors` covers independent invalid ordering/RMW controls; nine nested-type forms retain leaf spans. `generic_layout_builtins_require_sized_bound`, `checks_atomic_builtin_ordering_rules` and SIMD/offset rejection tests retain resolved controls. |
| Inline assembly | Structural/unresolved-field owner tests; driver independent operand error alongside a failed code/input expression | The configuration matrix retains an unknown option beside direct/structural recovery. `checks_inline_asm_configuration` retains invalid field, output, clobber, option and aggregate-operand controls. |
| Closures, returns, const/static and signature constraints | Thirteen driver owner-boundary pairs; six global initializer retention cases | The 67-case matrix retains fixed-argument errors while suppressing trait obligations on direct/structural Error substitutions; the owner trait-obligation test retains a resolved violation. Const/static duplicate attribution remains R2. |

The final R1 audit found required builtin type arguments losing unresolved leaves
under pointers and tuples. `lower_required_bracket_type` now follows the canonical
type walker through lowered type sites, excludes unused const interpretations and
reports each unresolved leaf at its own span. Error-union candidate resolution
visits both payload types. `fail/nested_required_type_recovery` pins two type roots
and one independent argument error without a layout consequence.

A valid const-generic control also exposed nested alternate readings being lost
when an outer speculative grammar production was rewound. Grammar snapshots now
retain their nested alternatives, and nested bracket expressions reuse the
resolver's candidate-aware traversal. Syntax/parser owner tests assert the retained
const interpretation; incremental syntax/AST/origin parity and
`nested_const_type_arguments_survive_edits_and_cache` verify valid, nested,
recovered and restored inputs against clean, cached and verified-cache reports.

### R2 Const/Static Diagnostic Precedence

`global_recovery_keeps_independent_initializer_errors` verifies six const/static
cases with an unknown type/value and an independent call-argument mismatch,
including exact root and argument spans. It establishes retention, not absence
of duplicate diagnostics. With `const fn need(value: i32) i32 { value }`,
`const VALUE: Missing = need(true);` previously published both the generic
const-call argument failure and the precise body argument mismatch. The query
report now suppresses the generic E0401 only when the same span has the body
E0301 call-argument failure. The static form retains its independent E0501
const-execution restriction, while the same-span static-data representation
consequence is suppressed. Driver controls cover malformed and valid arguments.

`specialized_const_roots_survive_report_precedence` retains E0401 for a failed
generic trait bound, a missing embedded file and an out-of-range shift count.
`const_static_precedence_survives_edits_and_cache` compares diagnostics and
rendered reports across repair, regression, cold checks, normal caches and
verified caches. The 12-test driver recovery matrix and 290-test const-eval
selection passed. The existing const diagnostic order remains: placing all body
diagnostics first would hide specialized const explanations. R2 is verified at
this boundary; R1's expression pairs alone did not establish precedence.

### O1 Operational Matrix

Every applicable failure cell requires text and JSON, nonzero exit status,
stdout/stderr routing, structured identity, summary, and captured child output
when a child ran. The matrix below names evidence and remaining gaps explicitly.

Windows native LLVM evidence has a required host setup. The repository's LLVM
23.1.2 static libraries are built with the MSVC static CRT (`/MT`), so the
matching Rust target, C/C++ bridge and linker environment must use the static
CRT and the installed MSVC/Windows SDK library roots. A default dynamic-CRT
Cargo session is not valid backend evidence: it can link mixed CRT objects and
terminate in LLVM object emission with `0xC0000005` before a report exists.
The environment requirement is now a named O1 precondition rather than an
implementation failure.

| Failure class | `check` | `emit` | `build` / `test` |
| --- | --- | --- | --- |
| Source syntax and semantic diagnostics | Captured reports parse JSON and assert source ownership, rule code, root kind, primary spans, summary and stdout routing | `emit_checked_preserves_text_json_diagnostic_contract`; dependency errors cover checked/LLVM/object requests in both formats | `build_preserves_text_json_diagnostic_contract` and `dependency_errors_preserve_identity_across_command_formats` cover both commands with source-owned JSON and text |
| Source/toolchain input unavailable | `diagnostic_failures` checks both formats, E0102/E0104, operational identity and empty stdout | Same matrix covers checked/LLVM/object input failures; object output path failures retain E0702 | Missing script E0704 and missing toolchain E0104 covered for both workflows |
| Invalid target/runtime/artifact/link configuration | No link/backend phase; exclude linker/backend failures | Both formats cover unsupported linker flavor E0701 and executable runtime E0103 | `build_cases` covers invalid artifact/output declarations in build/test, both formats. Public executor tests cover host/artifact mismatches before build/test action dispatch. The CLI does not accept a stale plan or separate runtime/linker flags for these workflows; the explicit boundary is detailed below. |
| Backend/linker/archive process failure | Not applicable: checking starts no backend/linker | Direct `emit --llvm` and `emit --obj` invalid-backend cases preserve I0300, source path, primary label, text/JSON identity and empty stdout; `nia-llvm` native object emission, CLI `emit --obj`, and the invalid atomic text/JSON pair pass under the static-CRT precondition; Linux CLI mock linker and Windows `.cmd` linker verify E0701 and both captured streams | Linux CLI mock archive and Windows `.cmd` archive verify build/test E0701, both captured streams and empty stdout across text/JSON under the static-CRT precondition |
| Build-plan/runner/external-command failure | Not applicable: no build plan | Not applicable to direct emission | `build_cases` covers duplicate definitions, invalid names/output and dependency cycles across both workflows/formats, plus build-only missing-default and unknown-step errors. Runner E0703 and external command E0705 retain bounded output; the fault-path inventory below covers spawn, wait, capture, timeout/cancellation, handoff and cleanup. |
| Test suite failure | Not applicable | Not applicable | `test_command` verifies aggregate E0705, exit status, suite-prefixed child stderr and exit notes in JSON, plus text/fail-fast behavior. Owner test preserves each timed-out suite's streams and reason. `build` does not execute test suites. |

The table records deliberate phase exclusions; an uncovered applicable boundary
must be reopened rather than inferred from another fixture's diagnostic code.
The final O1 audit repaired capture output loss, early return before joining the
sibling worker, missing host/artifact role, and missing handoff operation notes.
The former Windows `0xC0000005` observation came from running against the
static LLVM archive with the default dynamic Rust/bridge CRT. Re-running the
minimal LLVM emission, `emit --obj`, invalid atomic text/JSON, and atomic
executable checks with the static-CRT MSVC/SDK environment passed, so that
observation is retained as an environment diagnostic rather than counted as a
compiler failure. The Windows build/test invalid-backend workflow has its own
text/JSON evidence under the same host precondition.

The command tests now parse complete JSON instead of checking its opening text.
The dependency matrix runs `check`, `emit --checked/--llvm/--obj`, `build` and
`test` in both formats, checking exit status, empty stdout, uncolored stderr,
dependency path, root identity, primary source span and error summary counts.
JSON build/test requests disable live child-stream forwarding and retain bounded
failure output in structured notes. Text requests preserve live output. Windows
process cleanup no longer leaks taskkill messages into either stream. Linker and
archive failures retain both stdout and stderr and share the same diagnostic
construction for text and JSON. The accepted cells are listed in the table;
direct LLVM/object emission and the host-qualified build/test backend failures
are covered under the static-CRT setup.

#### Plan And Process Fault-Path Inventory

| Boundary | Evidence and CLI applicability |
| --- | --- |
| Build-script definition/graph rejection | Existing `build_cases` fixtures cover duplicate module/target, invalid artifact/output names, selected/unselected dependency cycles and missing/default step selection. Each failure parses complete JSON, checks E0703, status 1, empty stdout, captured stderr, absent publication and transient cleanup. Build and test share definition/graph validation; named/default build-step errors are excluded from test selection by `std::build::validatePlan`. |
| Frozen-plan target and reference invariants | `invocation_target_mismatch_is_rejected_before_actions` freezes real plans, invokes the public executor for host/artifact mismatches in build/test modes, and verifies E0704 text/JSON and absence of output setup. Expected means invocation target; found means plan target. The CLI constructs and freezes each plan from the current invocation and does not execute `build-plan.bin`. Malformed references, third targets and invalid codec tags are rejected by `nia-build-plan` validation/codec tests before dispatch; injecting stale/corrupt executor state is outside normal CLI input. |
| Plan read/publication and configuration I/O | `plan_handoff_failures_preserve_stage_and_paths_in_reports` exercises real missing/corrupt draft reads and a failed atomic publication, verifies E0704 in both formats and checks temporary-file retirement. `runner_configuration_publication_is_exclusive_and_preserves_collisions` verifies configuration-write identity and preserved collision contents. Prepare/cleanup variants retain their exact operation. Low-level create/write/sync/read failures remain typed `PlanHandoffError` variants rendered with stage/path by the same handoff adapter; exhausting disk, altering ACLs or racing private files is not required CLI evidence. Windows directory sync is explicitly a no-op in the owner. |
| Process creation and configured pipes | `runner_spawn_failure_reports_stage_and_cleans_handoff` and `external_spawn_failure_reports_operation_and_program` exercise missing executables and complete text/JSON reports. Configured missing pipes are defensive owner invariants: production always requests piped output before spawning. `unavailable_capture_workers_report_stage_stream_and_available_output` covers these variants and worker-start failure reports for both streams without OS thread exhaustion. |
| Capture workers, stream I/O and process wait | Shared capture tests use real workers and a reader that fails after emitting bounded output. Both workers are joined before failure selection; a panic retains the completed sibling's tail. `startup_cleanup_keeps_partial_output_from_an_existing_worker` covers failed sibling startup. External and runner completion tests inject wait/read/panic failures at the production completion boundary and parse full report JSON. Direct action, suite and staged-cleanup contexts retain the cause and all available output. These faults cannot be reliably induced through ordinary CLI input without OS/race manipulation. |
| Exit, timeout and cancellation | CLI external/runner failures and `test_command` cover real nonzero exits. `completion_preserves_success_timeout_and_cancellation_precedence` verifies reason priority and retained output; existing POSIX owner tests exercise process-tree timeout/cancellation and background pipe cleanup. Windows CLI exercises process exit/stream routing; the POSIX process-group tests remain explicitly platform-qualified. |
| Output transaction/cleanup failure | Coordinator publication, lock and recovery suites cover failed staging, partial commit rollback, absent outputs and interrupted transactions. The final external completion matrix wraps each capture/wait failure in a real `StagedOutput` report and verifies that original reason and output survive alongside the cleanup failure. Such concurrent/OS fault paths belong to owner tests rather than a CLI production fault switch. |

### Linux Terminal Evidence

Ubuntu 24.04 under WSL with LLVM 23.1.2 passed
`cargo test --locked -p nia-cli --test commands interactive_terminal_reports_color_text_without_coloring_json`.
The test exercises real PTY stderr, colored text, `NO_COLOR`, and JSON without
ANSI escapes. Evidence is separate from Windows workspace results; this closes
the Linux terminal evidence requirement. C1's local commit delivery and Windows
workspace acceptance are recorded above.
The complete Linux `nia-cli --test commands` suite also passed all 22 tests.
Linux `diagnostic_failures` passed eight tests, including linker/archive process
failures; `test_command` passed with structured child output. Windows
`diagnostic_failures` passed all eight tests, including the static-CRT linker
and archive process failures; the test-suite report check also passed. Current
Windows gate results are recorded in Current Delivery; Linux results remain separate.
The Linux build/driver `report::tests` suites passed 10 and 9 tests respectively, including
Unix-only runner stream, linker and archive-tool text/JSON ownership checks.

## Invariants

- A user-facing error has a source-owned primary label whenever the compiler
  has a source location. Generated code and fallback locations are never shown
  as the primary location when the originating source location is known.
- A diagnostic has one root cause. Later phases may retain context internally,
  but they must not publish errors that only describe an invalid recovery value
  created by an earlier phase.
- Independent modules may continue checking after one module fails. Within one
  module, downstream checks suppress only diagnostics whose codes describe
  recovery products poisoned by an earlier root; independent resolution and
  constraint errors remain visible.
- Diagnostics are structured until the CLI boundary. Formatting, suppression,
  color, and terminal layout do not belong in semantic or query providers.
- Every diagnostic shown to a user has a stable code, a precise primary span,
  and either a useful label, note, or help action. Internal diagnostics are
  clearly separated from source errors.

## Grammar Migration Track

The diagnostic roadmap includes the parser rewrite because recovery ownership
determines diagnostic quality. The target is one grammar-aware lossless green
tree owned by `nia-syntax`; `nia-parser` only lowers that tree to AST. Grammar
recognition now lives inside `nia-syntax`, and the separate `nia-grammar` crate,
old AST-building token-cursor parser, and `Unparsed` migration markers have been
removed. No permanent dual parser or compatibility layer is accepted before 1.0.

The track advances through these gates:

1. **Source and declaration grammar:** attributes, modules, functions,
   aggregate headers and bodies, imports, aliases, and bindings. This gate is
   complete in production.
2. **Member grammar:** trait and extension associated types, associated values,
   methods, parameters, return types, bodies, and recovery boundaries. This
   gate is complete in production.
3. **Type grammar:** generic parameters and arguments, paths, pointers,
   arrays, slices, tuples, callable types, projections, and type recovery.
   Nodes now preserve source ownership and nested delimiter structure in the
   production grammar.
4. **Expression, statement, and pattern grammar:** blocks, control flow,
   calls, operators, literals, bindings, match arms, and nested recovery.
   Independent malformed regions must remain parseable and must not stall the
   parser. These productions now build the production grammar tree.
5. **Lowering cutover:** AST lowering consumes grammar nodes exclusively;
   obsolete token-cursor production code and migration-only adapters are
   deleted. The loader consumes `Parse` and `LoweredModule` through this path;
   existing parser and driver diagnostic fixtures remain the parity oracle.
6. **Incremental reparse:** `nia-syntax` owns revision-aware subtree edits,
   its grammar reparses the smallest safe region, unchanged green subtrees
   are shared, and red identities carry the current revision. Clean and incremental
   parses must produce equivalent diagnostics, spans, and AST origins. This
   implementation now reparses safe function-body blocks and falls back to
   declaration regions using token-read boundaries. Position-independent green
   storage shares unchanged subtrees, including siblings moved by an edit;
   red identities and diagnostic origins are rebuilt for the current revision.
   Loader edits reuse cached syntax inside query-owned revision retirement and
   retain a source-text dependency. Tests cover UTF-8 and stale edit rejection,
   trivia changes, malformed delimiters, repeated edits, ambiguous generic/index
   arguments, AST-origin equivalence, snapshot retention, and bounded query
   storage. Lexing still covers the full source; malformed or speculatively
   dependent blocks conservatively use declaration-region reparsing.

Each gate requires owner-level recovery tests, parser/driver parity coverage,
and strict formatting and Clippy checks. Reviewable commits and final delivery
are recorded under C1. The 0.3.0 multi-platform matrix is separate from
this grammar migration and is not required to close these implementation gates.

## Implemented Coverage and Contracts

The following sections describe existing behavior and evidence. The acceptance
ledger above, rather than the length of this record, determines what remains.

### 1. Cascade control

The query facade now gates per-module diagnostic collection by root diagnostic
code. Unresolved names and invalid signatures suppress only downstream type,
const, layout, and code-generation products while independent resolution and
constraint errors remain visible. The gate scopes function-local roots to their
enclosing function or method and requires one downstream primary span to contain
the other (or carry an exact source/code/span cause identity). This preserves
independent type errors elsewhere in the same function as well as in a different
function in the same module. An end-to-end call fixture confirms an unresolved
first argument does not hide an independent type mismatch in a later argument.
Build-runner compilation performs a source check first and publishes `build.nia` diagnostics instead of
generated-wrapper recovery errors when both are present.

Regression coverage includes incremental supertrait constraints, generated
runner ownership, and unresolved build-script values.

### 2. Source ownership and related locations

Introduce an explicit diagnostic origin/relationship model. A generated call
site may be retained as a secondary location, while the declaration or source
expression that caused it becomes primary. This is required for build runners,
macro-like generated code, imported declarations, and cross-module trait
resolution.

The first source-evidence chain is now implemented for failed `using` lookups:
the using scope retains the exposed name, selected-name span, directive span,
and a classified cause (`unknown`, private, not-public, or invisible namespace).
Value and type resolution consume that evidence at the actual use site, while
the using directive remains available as related context. The evidence is
indexed by local name so repeated lookups do not scan the complete import list.

Text reports load source text for explicit related paths, including dependencies
with no diagnostics of their own. Related locations use their owning file's
line and column; unavailable external sources retain byte spans instead of
borrowing the primary file's line numbers.

Trait implementation validation now resolves related declaration locations
through the implemented trait's owning module. The trait signature index also
retains declaration names by global identity, so cross-module supertrait errors
name the source trait instead of exposing an internal definition id.

### 3. Semantic diagnostic contracts

Replace generic summaries such as `name is unresolved` and broad `type-check`
messages with rule-specific constructors. Each constructor owns:

- the diagnostic code;
- the primary and secondary labels;
- the expected/actual type rendering;
- optional candidate lists;
- a concrete help action.

The first targets are name lookup, qualified values, imports, error-union
propagation, callable resolution, and visibility.

Name lookup, qualified namespace lookup, and failed using directives now use
rule-specific summaries with primary labels, related source locations, and
actionable help. Ordinary unresolved body values now identify the missing name
and use the name-resolution code. Method generic parameters that shadow an
enclosing extension or trait parameter now point at the repeated name and tell
the user to rename it or use the enclosing parameter. Type and module namespaces
used as values also have distinct contracts with declaration evidence where
available.
Assignment/place checking reuses the same import evidence and avoids publishing
secondary place errors after an unresolved expression, preventing syntax-
specific fallbacks such as `module value is unresolved` from hiding the original
import failure.
Unknown receiver-method calls on non-aggregate types now identify the missing
method and receiver type instead of falling through to an unrelated aggregate
field error. Callable fields on structs and unions still use the ordinary
field and callable checks; range-bound methods report when the requested bound
is absent. Ordinary and explicit-generic call fixtures now snapshot arity,
argument type, and non-callable errors together, while an unresolved argument
keeps an independent type mismatch in a later argument visible after generic
substitution as well. A call with both an unresolved argument and too few
arguments retains both independent diagnostics.

The S1/S2/S3 and R1 inventories above record the completed callable, visibility,
candidate and recovery-family audits, including cross-module scope boundaries
and resolved invalid controls. Ambiguous extension-method resolution keeps its specific
candidate diagnostic without falling through to a misleading missing-field
error. Ambiguous extension associated-function references also stop before
ordinary value checking can publish a secondary error type or initializer
mismatch.
Propagation now short-circuits an operand already typed as the error recovery
sentinel, so an unresolved value under `.?` retains only its name-resolution
diagnostic instead of also being described as a non-propagatable type.
Ambiguous bracket arguments now retain unresolved type-candidate evidence until
a semantic signature establishes that the argument must be a type. That context
reports an unknown type, while a name resolved as a const value reports the
type/value mismatch; const-generic arguments keep their value interpretation.
Atomic builtins now stop type-dependent checks on that recovery sentinel while
retaining independent pointer-shape and operation-code errors. Invalid RMW
operation codes have a dedicated diagnostic code so an earlier type-resolution
failure does not hide them in per-module phase gating.
Atomic ordering and RMW operation arguments now also stop const-integer
validation when name resolution has already produced an Error recovery value;
an unresolved control argument remains a single name-resolution diagnostic.
The unaligned-load builtin also ignores an Error recovery pointer instead of
reporting a secondary byte-pointer mismatch.
The slice-length builtin likewise suppresses its slice-pointer shape error
when name resolution has already produced an Error recovery value, while an
independent non-pointer argument still receives the shape diagnostic.
For-in checking now treats an unresolved iterable as a root error: iterable,
iterator, and irrefutable-pattern shape checks do not publish recovery-derived
diagnostics, and bindings introduced by the recovered pattern still receive an
Error type so later uses do not report a second unknown local type.
Defer statements apply the same rule: an unresolved deferred expression does
not produce a second unit-type diagnostic, while independent later errors remain
visible. Ordinary expression statements apply the same rule: an unresolved
expression does not produce a second discarded-result diagnostic, while a
separate non-unit expression statement remains visible.
The same recovery check now traverses structural types, so an unresolved value
wrapped in a tuple, array, pointer, optional, or error union cannot trigger a
discarded-result, `defer`, or `for-in` shape diagnostic from its wrapper.

### 4. Report organization

Make the report explicitly hierarchical: root errors first, related context
under each root, then independent errors. Suppression must say whether entries
were duplicates, downstream consequences, or a display limit. The report must
never present a generated wrapper before the source error that invalidated it.

The report layer now keeps deterministic duplicate, downstream-consequence, and
display-limit counts, counts retained errors and warnings, and ranks source-owned
primary spans ahead of generated wrappers. Downstream suppression counts survive
checked/codegen products and frontend check-certificate cache reuse. The full
root/related/independent hierarchy is now ordered explicitly when diagnostics
carry an unambiguous source-owned cause identity. That cause identity is retained
in stable diagnostic bundles. Failed `using` lookups now carry their emitted
root identity through value, type, and body resolution, so later value and type
use-site diagnostics are grouped under the import failure. A source fixture now
covers repeated use of the failed import in a function parameter and return
type, and snapshots the shared root identity. P1/P2 and R1/R2 above account for
source provenance and the bounded recovery-family inventory. The for-in path
suppresses its recovery products at the semantic checker boundary. Trait-bound,
projection, and builtin-operator checks now suppress obligations whose receiver
or trait arguments contain an Error recovery type, including structural tuples
and recovered generic arguments. Independent bounds on resolved values remain
visible, with body-check and driver snapshot coverage for both cases.

### 5. CLI interaction contract

Add stable text and machine-readable output modes, explicit summary counts, and
consistent exit status behavior for check, emit, build, and test. Terminal
rendering should provide color and compact context when interactive, while
non-interactive output remains deterministic and snapshot-friendly.

Text and JSON reports now expose the same retained error/warning summary and
duplicate/display-limit counts. Interactive text diagnostics are colorized at
the CLI boundary, while captured and JSON output remains deterministic and
free of ANSI control sequences. CLI cases now assert exact success and failure
statuses for check and emit, with test/build workflow status coverage.
Build and test failures now also share an explicit text/JSON captured-output
contract, including empty stdout, stable exit status, and the diagnostic JSON
envelope.
A Linux pseudo-terminal integration test verifies color on interactive text output and ANSI-free JSON output on the
same terminal; interactive text also honors `NO_COLOR`. Other terminal
platforms are deferred to the 0.3.0 multi-platform port. Command/output acceptance
is defined by O1's explicit matrix above.

### 6. Regression and quality gates

Every new diagnostic rule requires a source fixture and a snapshot containing:

- code and severity;
- primary path, line, and column;
- source excerpt and label;
- notes/help/related locations;
- suppression summary when applicable.

The failed-using provenance chain now has a source fixture and snapshot,
including the cause identity of repeated signature type diagnostics. Check-case
snapshots now include the rendered text report as well as structured fields,
covering severity, line/column, source excerpts, related locations, hierarchy,
and summary counts. This also covers reports after an incremental body edit.
A parser recovery fixture now exercises a missing statement terminator followed
by an incomplete expression, so grammar diagnostics and recovery order are
asserted through the same end-to-end structured/text snapshot path. It also
covers a missing function-parameter delimiter: parameter type recovery stops at
the function body, item recovery skips that invalid body, and diagnostics in
later functions remain visible. It also snapshots missing parameter types,
binding patterns, closing delimiters, and missing names, including their
rule-specific help. A lexical-error token no longer produces a secondary
missing-semicolon diagnostic at the same recovery point. Invalid expressions at
a statement boundary now recover locally and keep later statements and
top-level declarations, with parser AST and driver report coverage. Top-level
item recovery also tracks nested parentheses and brackets, so a malformed
declaration cannot stop at a nested semicolon before the next valid item; parser
and driver coverage pin the retained declaration. Statement recovery now tracks
nested parentheses, brackets, and braces, so a failed
control-flow expression cannot stop at a semicolon inside its discarded body;
the enclosing block and later declarations remain parseable with parser AST and
driver report coverage. Statement-keyword recovery also consumes a terminator
following a discarded nested block, so malformed `return` expressions do not
emit a second empty-expression error. Unresolved expression statements now
suppress their discarded-result recovery diagnostic at the body-check boundary,
with a driver snapshot proving the root name error and an independent discarded
literal remain distinct. Structural recovery wrappers receive the same
end-to-end coverage for discarded expressions, deferred pointers, and iterable
shapes. Malformed
struct fields
and named enum payload fields now recover at their comma boundaries; the driver
snapshot keeps both diagnostics and later functions visible, while a parser unit
test confirms a valid field after the malformed member survives in the AST.
Tuple struct fields and tuple enum payloads now skip a missing type at a comma,
retain later tuple elements and enum variants, and preserve following top-level
items; parser and driver snapshots cover both forms. Tuple type expressions
also retain valid elements and their enclosing function after missing elements
at comma boundaries. Tuple expressions and irrefutable tuple binding patterns
likewise retain valid elements and later statements after missing elements at
comma boundaries, with separate end-to-end snapshots. They also diagnose a
missing separator between valid tuple elements and retain the rest of the tuple
and following statements, with parser and driver coverage. The other accepted
parser recovery boundaries are recorded below. Associated
type/value member
recovery now has equivalent parser and driver coverage: malformed trait and
extension associated declarations retain later methods and following top-level
items while preserving the local grammar diagnostics. Call argument lists now also
skip missing expressions at comma boundaries while retaining later arguments and
following statements; a parser unit test covers the AST boundary and a driver
snapshot covers the rendered diagnostics. They also diagnose a missing separator
between valid arguments and retain later arguments and statements, with parser
and driver coverage. Array and struct literal lists now
also skip missing expressions at comma
boundaries while retaining later elements and fields; a parser unit test and
driver snapshot cover that boundary. They also diagnose a missing separator
between otherwise valid array elements or named fields and continue parsing the
remaining literal and following statements, with parser and driver coverage.
Struct literal recovery also tracks nested parentheses, brackets, and braces,
so an invalid field expression cannot treat a comma inside its discarded
nested expression as the next field boundary; later fields and statements
remain parseable with parser and driver coverage.
Array literal recovery uses the same nested delimiter synchronization for
malformed elements, preserving later elements and following statements with
parser and driver coverage.
Tuple expression recovery now applies that synchronization to malformed first
and later elements, retaining the remaining tuple and following statements
with parser and driver coverage.
Function pointer and callable interface type parameter lists likewise retain
later parameters and the enclosing function after missing types at commas, with
parser and driver coverage.
Generic parameter lists also skip a missing parameter name at a comma, retain
later parameters and the enclosing function, and have parser and driver
coverage. They also synchronize missing commas between clearly identifiable
parameters and before the closing bracket, retaining later parameters while
keeping the enclosing function parseable.
Const generic parameters now diagnose a missing bound type at a local comma or
closing-bracket boundary, retain later parameters, and keep the enclosing
function parseable with parser and driver coverage.
`where` predicate lists use the same local recovery for a missing predicate
type, retaining later predicates and following functions with parser and driver
coverage. They also recognize a clearly identifiable next predicate after a
missing comma, preserving its bounds and the enclosing function with parser and
driver coverage.
Associated type binding arguments also recover a missing binding value at a
comma, retain later arguments and the enclosing function, and have parser and
driver coverage; the recovery path suppresses the parser's generic expected
type duplicate when the more specific binding diagnostic is emitted.
Explicit type argument lists also synchronize a missing comma before `]` and
retain clearly identifiable later type or const arguments, the enclosing
function, and following declarations.
Malformed explicit type arguments now recover through nested parentheses,
brackets, and braces, retaining a later top-level argument and avoiding a
second delimiter diagnostic for the same missing argument.
Unterminated explicit type arguments now stop at an enclosing parameter or
statement delimiter, retaining the current and following top-level items
instead of consuming the rest of the module, with parser and driver coverage.
Unterminated struct, union, and enum bodies now stop at a following top-level
item instead of treating a later declaration as a member and consuming its
body, with parser and driver coverage.
Function parameter lists likewise synchronize a missing comma before `)` while
preserving the current function and later top-level declarations.
When the next parameter has a clear `name:` or receiver shape, recovery now
retains that parameter in the AST and continues checking its use; parser
coverage and the function-parameter driver snapshot pin this behavior.
Closure parameter lists use the same local recovery and retain a clearly
identifiable next parameter after a missing comma before `->`, while preserving
the enclosing closure and function.
Trait and extension member parameter lists reuse the same local recovery and
now have parser and driver coverage proving later methods and top-level items
survive.
Supertrait lists now apply the same local synchronization when a clearly
identifiable next type is missing `+`, retaining both supertraits and the
following declarations with parser and driver coverage.
They also treat comma-separated malformed entries as local supertrait
boundaries, report the missing `+` or type, and preserve later traits and
following declarations with parser and driver coverage.
They also skip a missing parameter name at a comma and retain later parameters
and the closure body with parser and driver coverage.
Function pointer, callable interface, and tuple type element lists now report
and recover a missing comma before `)` as one local type-parameter boundary.
Tuple struct and tuple enum payload lists apply the same boundary recovery,
retaining later payload types and following declarations with parser and driver
coverage.
Using selector groups now synchronize an invalid member at the next comma or
closing brace, retaining later selectors and the following top-level item.
They also diagnose a missing separator between valid selectors and retain the
remaining selectors and following items, with parser and driver coverage.
Using selector recovery tracks nested parentheses, brackets, and braces, so an
invalid nested selector cannot consume a later top-level selector; parser and
driver coverage pin that boundary.
Unterminated using selector groups now stop at the next top-level item while
retaining the missing-closing-delimiter diagnostic and the following item, with
parser and driver coverage.
Attribute argument lists now synchronize a missing expression at the next
comma or closing parenthesis, retaining later arguments and the attributed
item. They also diagnose a missing separator between valid arguments and retain
the remaining arguments and attributed item, with parser and driver coverage.
Attribute argument recovery likewise tracks nested parentheses, brackets, and
braces, retaining later arguments and the attributed item after a malformed
nested expression.
Malformed attribute headers now recover through their closing bracket or the
next top-level item/attribute boundary, keeping the attributed item and
following declarations parseable without a duplicate top-level item error,
and statement attributes also stop at the next statement boundary, with parser
and driver coverage.
Closure capture lists now synchronize a missing capture name at the next comma
or closing bracket, retaining later captures and the enclosing closure.
They also diagnose a missing comma before a clearly identifiable next capture
and continue parsing that capture.
Bracket argument lists now skip a missing argument at a comma and retain later
arguments, the surrounding type or expression, and following top-level items.
They also diagnose a missing separator between valid arguments and retain the
remaining arguments and following items, with parser and driver coverage.
Bracket argument recovery also tracks nested parentheses, brackets, and
braces, so a malformed nested argument cannot consume later top-level
arguments; parser and driver coverage pin the retained argument and statement.
Match arm pattern lists now skip a missing pattern at a comma and retain the
later patterns, arm body, and following statements.
They also diagnose a missing separator between valid patterns and retain the
remaining patterns and arms, with parser and driver coverage.
Tuple and nominal tuple match patterns now skip a missing field at a comma and
retain later fields and arms; parser and driver snapshots cover both recovered
shapes.
Named nominal match patterns likewise synchronize a missing field pattern at a
comma or closing brace, retaining later fields and arms with parser and driver
coverage. Tuple, nominal tuple, and named nominal patterns also diagnose a
missing separator between valid fields and retain the remaining fields and
match arms, with parser and driver coverage.
Tuple match and irrefutable binding patterns also synchronize a malformed first
field across nested delimiters, retaining later fields, arms, and statements;
parser and driver coverage pin the recovered boundary.
Tuple type parsing applies the same nested synchronization when its first type
fails, retaining later tuple elements and the enclosing declarations with
parser and driver coverage.
Function pointer and callable interface parameter lists now apply the same
nested synchronization for malformed first parameters, retaining later
parameters and declarations with parser and driver coverage.
Tuple pattern/type lists and ordinary call arguments also synchronize malformed
later entries across nested delimiters, retaining the remaining entries and
following statements with parser and driver coverage.
Named struct, union, and enum payload fields likewise diagnose a missing
separator between valid fields, retaining later fields and following items
with parser and driver coverage.
Enum variant lists now recover a missing variant name at a comma or closing
brace, retaining later variants and following top-level declarations with
parser and driver coverage. They also report a missing separator between valid
variant names while retaining the later variants and declarations with parser
and driver coverage.
Match arm bodies now synchronize an invalid arm at the next top-level comma or
closing brace, retaining later arms and statements after the match.
These recovery cases were first audited in the token-cursor parser. Grammar
recognition and recovery now belong entirely to `nia-syntax`; `nia-parser` lowers
the finished tree into AST. Declarations, members, types, expressions, statements,
and patterns use this path, and existing diagnostic fixtures continue to verify
their recovery behavior. The old token-cursor AST parser and `Unparsed` migration
nodes are gone. Incremental block/declaration reparsing, green structural sharing,
and loader edit retirement now use the contracts and conservative boundaries above.
Field names and types, enum variant names, payload delimiters, and initializer
expressions now have nested grammar nodes with explicit missing-node recovery.
Trait and extension members now retain nested associated type, associated value,
method, parameter, return type, and method body nodes with member-boundary
recovery.
Generic declaration parameter lists now retain delimiter, parameter, and bound
type nodes with recovery at malformed separators and following top-level items.
Top-level `using` declarations now also have grammar nodes with nested selector
delimiters and outer-item recovery.
Type aliases and const/static bindings now use the same declaration boundary and
terminator recovery in the grammar tree.
Error recovery targets in optional, error-union, tuple, pointer, and null
patterns now suppress target-shape and match-coverage consequences while
independent errors in the arm bodies remain checkable; an end-to-end snapshot
pins this behavior. Range patterns also continue resolving both bounds against
an error target, while skipping compile-time constant and coverage checks that
would only describe the recovery type. If one bound itself has an Error type,
only that bound's constant check is suppressed; an independent non-constant
error on the other bound remains visible.
A private-using fixture verifies related locations in a clean dependency whose
declaration is on a different line from the primary use site. Renderer tests
cover available and unavailable related sources and explicit same-file paths.
Unknown values, type-as-value misuse, and module-as-value misuse now each have
dedicated fixtures and complete structured/text snapshots.
Invalid method generic shadowing in both extension and trait declarations now
has a complete snapshot for the item-signature code, repeated-name label,
actionable rename help, and retention of an independent unresolved value in
another function.
Ordinary call diagnostics have a combined snapshot for argument arity, argument
type, non-callable callees, and an unresolved argument alongside an independent
later argument mismatch. The same unresolved-first-argument boundary is covered
for an explicit generic call after type substitution. A missing argument count
and an unresolved supplied argument on the same call are also both retained.
Optional and error-union propagation boundaries, invalid propagation operands,
and missing `IntoError` conversions now share an end-to-end case with complete
structured/text snapshots, including the enclosing function return boundary.
A missing value used under `.?` now has a snapshot proving the recovery type
does not produce a second propagation diagnostic.
A structurally recovered optional operand such as `(?missing).?` now has the
same root-only behavior, with body-check and driver coverage.
A genuinely ambiguous pair of visible `IntoError` implementations now has its
own snapshot for the ambiguity rule, propagated operand, return boundary, and
corrective help.
A rejected two-step `IntoError` chain also has a complete snapshot showing the
propagated operand, return boundary, one-step conversion rule, and direct
conversion help.
A type generic argument whose expression is already an Error recovery now
retains the expression's name-resolution root and suppresses the generic
category fallback; a driver snapshot covers the three downstream consequences.
An unknown qualified callable member now has an end-to-end snapshot for the
name-resolution code, typo suggestion, edit range, and suppressed downstream
consequence. A cross-module associated-call snapshot now distinguishes a
visible public function, a private method with its dependency declaration, and
an unknown associated function without falling back to a generic qualified
value error.
An unknown receiver method now has a complete snapshot for its method name,
receiver type, and corrective help. Regression tests preserve field-call
fallback; a separate snapshot covers ranges missing the requested start or end
bound. Ambiguous extension-method calls and associated-function references now
have a complete driver snapshot for candidate reporting without missing-field
or value-context fallbacks. SIMD builtins now
have snapshots proving error recovery does not add secondary shape diagnostics,
and that an unresolved type candidate or a const value passed where a type is
required receives the correct root diagnostic.
Builtin type argument lists also preserve an unresolved expression as the
root diagnostic instead of publishing a second "generic arguments must be
types" error for its recovery type.
Structural recovery provenance now also covers builtin arguments, generic
type arguments, place/index/deref operations, and match targets. Composite
recovery values such as pointers, arrays, tuples, and optionals no longer
trigger secondary builtin shape, trait-bound, pattern-shape, or exhaustiveness
diagnostics. Body-check tests and a driver snapshot cover both matching and
intentionally mismatched pattern constructors against those recovered shapes.
Generic inference applies the same root-only rule to structural substitutions:
an inferred tuple, pointer, or other generic shape containing an Error recovery
type does not publish a conflicting-substitution diagnostic, and later
arguments are not rechecked against that poisoned shape to manufacture tuple
element mismatches. The original unresolved expression remains the only
diagnostic for that inference path, with body-check and driver snapshot
coverage.
Irrefutable binding patterns apply the same recursive recovery rule, so a
destructuring pattern against a tuple or other composite containing an
unresolved value does not publish a secondary binding-shape diagnostic; the
body-check and driver suites cover this boundary.
Binary and shift operator recovery is covered end to end: unresolved operands
retain their name-resolution roots while operator trait and operand-shape
consequences are suppressed. Shift-count shape checking now applies the same
root-only rule to an unresolved right operand while retaining an independent
non-integer shift count diagnostic.
Unary arithmetic, bitwise-not, and error-union success recovery now follow the
same root-only contract when their operand is unresolved, with body-check and
driver snapshot coverage.
The same root-only contract is covered across field access, indexing, deref,
address-of, casts, optional propagation, and calls applied to an unresolved
value.
Atomic builtin recovery has a snapshot proving invalid RMW operation codes stay
visible alongside unknown type arguments while type-dependent atomic errors are
suppressed. An unaligned-load recovery snapshot proves an unresolved pointer
keeps only its name-resolution root. A slice-length recovery snapshot proves an
unresolved value keeps only its name-resolution root.
Layout builtins now suppress their `Sized` obligation when the explicit type
argument is an Error recovery, preserving only the unknown-type root with body
check and driver snapshot coverage.
Inline assembly operand validation now suppresses aggregate-shape diagnostics
when an operand's structural type contains an Error recovery, preserving the
underlying unresolved value as the sole root while still reporting shape errors
for resolved aggregate operands.
Inline assembly literal fields also check their expressions before enforcing
byte-string shape, so unresolved code, clobber, and option values retain their
name-resolution roots instead of being replaced by literal-only diagnostics.
The same contract applies to unresolved asm configuration, input, output,
clobber, and option containers; independent missing-required-field diagnostics
remain visible when the surrounding literal itself is otherwise valid.
The `offset` builtin now preserves an unresolved field-name expression as its
name-resolution root instead of replacing it with a string-literal shape error.
Qualified associated function-pointer references now reuse the restricted
extension-method visibility contract used by calls, preserving the declaration
location for private and restricted methods instead of silently returning a
recovery type. Cross-module free generic function pointers now retain the
declaration's generic parameter identity. Missing explicit arguments produce the
dedicated function-pointer diagnostic while explicit type arguments instantiate
normally.
Directly qualified private values and types now retain the owning
dependency source path for their declaration locations instead of rendering those
spans against the use-site file. The module graph exposes each module's source
path
to semantic consumers so related locations retain ownership across module
boundaries. Directly qualified `pub(super)` values and types now report their
actual visibility scope and declaration locations instead of mislabeling them
as private. Restricted module namespaces now report their actual scope and
point to the module declaration for both value and type paths. Directly
qualified `pub(pkg)` values and types now have dependency-backed snapshots for
package-scope errors and related declaration locations. A package-restricted
receiver-style or associated extension function now reports its visibility and
dependency declaration instead of falling through to an unrelated field or
unknown-function error.
Private, `pub(super)`, and `pub(pkg)` associated values in visible extensions
now retain their declaration visibility through lookup and report source-owned
diagnostics with dependency declaration locations instead of falling through to
a generic qualified-expression error. Package-internal access remains valid.
The same package visibility contract is covered for qualified associated
function-pointer references, including dependency-owned declaration locations;
call and pointer forms now share the source-owned diagnostic behavior.
Package-restricted module namespaces now retain parent declaration visibility
even when the dependency graph has not materialized a child module node; value
and type paths report the actual package scope and source-owned declaration.
Compiler check fixtures now exercise the registered frontend rule codes through
E0501: parse recovery has source snapshots, name/type/signature resolution uses
E0201-E0203, body/local/object-safety/type-check rules use
E0301-E0304, and const/layout failures use E0401 and E0501. Recursive type
normalization and object-safety each have dedicated source contracts rather
than relying only on broad type-check assertions. Operational codes remain
covered by their owning suites: target/toolchain and CLI usage by CLI tests,
build-plan/runner/action and artifact I/O by build/report tests, and linker or
LLVM failures by backend and linker tests. O1 defines operational cross-product
coverage and explicit phase exclusions; P1/P2 and R1/R2 define source ownership
and recovery provenance. The code registry itself is not evidence that every
combination of phase, source ownership, and suppression has been exercised.

The broad workspace test, clippy, formatting, and CLI case suites remain
mandatory for each stage.
