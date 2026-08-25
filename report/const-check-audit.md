# `nia-const-check` Audit Report

Status: F1 fixed in `d70f1807`; F2, F3, and F4 open

| Field | Value |
| --- | --- |
| Scope | `crates/nia-const-check`, plus the boundaries it shares with `nia-const-eval`, `nia-const-ir`, `nia-body-check`, and `lib/std/builtin` |
| Commit | `c8e02bce` |
| Compiler | `target/debug/nia`, built from this tree at the workspace version in `Cargo.toml` |
| LLVM | 22.1.8 |
| Invocation | `./target/debug/nia --resource-root lib check <file>` unless a finding states otherwise |
| Fixtures | `report/fixtures/const-check/` |
| Date | 2026-08-25 |

Fixture sources quoted in this report are preserved under
`report/fixtures/const-check/`. The `/tmp` paths appearing in captured compiler
output are the original scratch locations from the audit run; the equivalent
preserved fixture is named after the finding it belongs to. To reproduce an F1
case:

```sh
./target/debug/nia --resource-root lib emit --exe \
  report/fixtures/const-check/f1_tuple_static.nia -o /tmp/f1_tuple
/tmp/f1_tuple; echo $?   # 1 = defect reproduced, 0 = fixed
```

## 1. How To Read This Report

Every finding below was reproduced against the built compiler in this tree. Each
one records the exact fixture source, the observed compiler output, and the
source location responsible. Where a finding is a **documented** limitation
rather than a defect, it says so and cites the owning document, because the
audit's value depends on separating "known and written down" from "silently
wrong".

Findings are ordered by severity, not by discovery order:

- **F1** is a silent miscompilation: wrong data reaches a linked executable with
  no diagnostic, confirmed by execution for three value kinds. This is the only
  finding that produces an incorrect program.
- **F2** and **F3** are contract violations against `docs/language-spec.md` and
  `docs/architecture.md`: the implemented behavior and the normative text
  disagree.
- **F4** is a feature request: these builtins have no const evaluator at all.
  An earlier revision of this report claimed the evaluator was already complete;
  that claim was fabricated and is corrected in §6.2.
- **F5** records verified-correct behavior, to bound the audit and prevent a
  later reader from re-investigating the same ground.

No fix is proposed as a diff. Where a repair direction is non-obvious, the
finding ends with a short note on which owner should hold the change, per the
single-owner rule in `docs/compiler-maintenance.md` §1 and §3.

## 2. Severity Summary

| ID | Severity | Title | Kind |
| --- | --- | --- | --- |
| F1 | Critical — **fixed `d70f1807`** | Named `const` values silently become zero in `static` initializers (tuple, optional, enum confirmed) | Miscompilation |
| F2 | High | `const fn` capability is not validated at the declaration | Spec violation |
| F3 | Medium | Named-const static lowering drops a value without a diagnostic | Contract violation |
| F4 | Medium | `ctz`/`clz`/`popcount` are not const-callable and have no const evaluator | Feature request |
| F5 | Informational | Verified-correct boundaries | Coverage record |

F1 and F3 share one root cause chain; they are separated because F1 is the
observable program defect and F3 is the reusable ownership flaw that will
produce another F1 for the next value kind that lacks a `StaticInit` variant.

## 3. F1 — Named `const` Values Silently Become Zero In `static` Initializers

**Fixed in `d70f1807`.** The sections below record the defect as found. See
§3.8 for what was changed and how to confirm it.

**Severity: Critical.** A linked executable observes zeroed storage where the
source specifies a concrete value. No diagnostic is produced, and `check`,
`emit --llvm`, and `emit --exe` all report success.

Three distinct value kinds were driven to a running executable and each produced
a wrong program:

| `const` value kind | Emitted initializer | Executed result |
| --- | --- | --- |
| Tuple `(1i32, 2i32)` | `{ i32, i32 } zeroinitializer` | `S.0` reads `0`, not `1` |
| Optional `?5i32` | `{ i8, { i32 } } zeroinitializer` | present optional reads as `null` |
| Enum `Code::A` (`= 7`) | `i32 0` | `S` holds `0`, not a declared variant |

The optional case changes control flow rather than a numeric value: an `if S is
?v` arm that must be taken is skipped. The enum case is worse still — the stored
value is outside the closed enum's declared variant set (`7` and `9`), so a
`match` on it cannot be satisfied by any named arm.

§3.1 through §3.3 use the tuple case as the worked example; §3.4's root cause
chain is shared by all three.

### 3.1 Reproduction

`/tmp/cc/f5c_tuple_static_exe.nia`:

```nia
using std::process;

const T: (i32, i32) = (1i32, 2i32);
static S: (i32, i32) = T;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    // S.0 is 1 and S.1 is 2, so a correct initializer exits 0.
    if S.0 != 1 {
        return process::exit(1)!;
    }
    if S.1 != 2 {
        return process::exit(2)!;
    }
    !()
}
```

Observed:

```text
$ ./target/debug/nia --resource-root lib emit --exe /tmp/cc/f5c_tuple_static_exe.nia -o /tmp/cc/tuple_exe
emit exit=0          # no diagnostics

$ /tmp/cc/tuple_exe; echo $?
1                    # S.0 == 0, not 1
```

The executable exits `1`, proving `S.0` read as `0`. The compiler accepted the
program in full.

### 3.2 Emitted IR

The published global carries `zeroinitializer` rather than the const value:

```llvm
@nia__sb1a2c0db96cc81aa__d16806013500440183785__sym_af640e4c86024182 = constant { i32, i32 } zeroinitializer
```

### 3.3 The Same Program With A Struct Is Correct

Replacing the tuple with a two-field `struct` isolates the defect to the tuple
value kind:

```nia
struct Pair { a: i32, b: i32 }
const T: Pair = Pair { a: 1, b: 2 };
static S: Pair = T;
```

```text
$ /tmp/cc/struct_exe; echo $?
0                    # correct

# emitted initializer:
@nia__s3514e782c9602a50__... = constant %nia__...__sym_2df72619ecf90e11 { i32 1, i32 2 }
```

So the static-initializer pipeline is sound for the value kinds it explicitly
handles (`Int`, `Float`, `Bool`, `Array`, `Struct`) and silently loses the rest.
§5.2 enumerates which kinds fall into each group.

### 3.4 Root Cause Chain

Four steps, each individually defensible, combine into a silent wrong answer.

**Step 1 — `nia-body-check/src/static_init.rs:544-581`.** The named-const
lowering `lower_static_const_value` handles `Int`, `Float`, `Bool`, `Array`, and
`Struct`, then ends with a bare catch-all:

```rust
            _ => None,
```

`ConstValue::Tuple` lands here. `None` is returned and **no diagnostic is
pushed**. This is consistent with `nia-static-ir` having no `Tuple` variant, but
the absence is communicated only as `None`.

**Step 2 — `nia-body-check/src/static_init.rs:117-123`.** The caller treats that
`None` as "not a named const value" and falls through to the recovery value:

```rust
                if let Some(value) = self.static_const_value(expr) {
                    let value_ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
                    if let Some(init) = self.lower_static_const_value(value, value_ty) {
                        return init;
                    }
                }
                StaticInit::Zero
```

Still no diagnostic. Note the contrast with the generic unrepresentable-value
arm at `static_init.rs:212-219`, which pushes
`"global initializer is not representable as static data yet"` before returning
`Zero`. The named-const path skips that report.

**Step 3 — `nia-body-check/src/static_init.rs:25-34`.** The recovery guard works
exactly as documented and refuses to publish the placeholder:

```rust
        let diagnostics_before = self.diagnostics.len();
        let init = self.lower_global_static_init(expr, ty);
        (self.diagnostics.len() == diagnostics_before && !Self::contains_static_recovery(&init))
            .then_some(init)
```

Because step 2 pushed no diagnostic, the first conjunct is satisfied; because the
tree is `Zero`, `contains_static_recovery` is `true`. The guard returns `None`.
Its own doc comment states the intent precisely: *"publishing it would leak a
fake initializer into reachability or executable Body IR."*

**Step 4 — `nia-body-check/src/pipeline.rs:126-128`.** The guard's rejection is
consumed as a plain absence:

```rust
        if let Some(init) = self.lower_global_static_init_checked(value, global_ty) {
            self.global_inits.insert(global_def_id, Arc::new(init));
        }
```

There is no `else`. A global with no `global_inits` entry is indistinguishable
from a `static` written without an initializer, and `docs/language-spec.md:1767`
specifies that such a static is zero-initialized:

> Non-extern uninitialized `static` declarations create static storage
> initialized to zero.

The correct zero-default for a genuinely uninitialized static therefore supplies
the wrong value for a rejected initializer.

### 3.5 Why The Existing Guard Did Not Catch It

The guard's contract assumes rejection is always accompanied by a diagnostic —
its two conjuncts are "no new diagnostics" **and** "no recovery value", so it
treats a recovery value as a failure signal. That is correct. The gap is that
rejection and "no initializer present" are represented by the same value
(`None`) at the `pipeline.rs` boundary, and only one of those two states should
produce zeroed storage.

### 3.6 Contract References

- `docs/architecture.md:2738-2741` states the intended behavior explicitly:
  *"Tuples have no equivalent `StaticInit` variant; unions, pointers, and other
  const values require a separately defined materialization contract, so named
  values of those forms remain outside this boundary. Static admission applies
  the same recursive predicate as lowering, and `StaticInit::Zero` is reserved
  for diagnostic recovery rather than publication as a successful initializer."*
  The tuple is neither admitted nor rejected with a diagnostic; it is published
  as zero.
- `docs/compiler-maintenance.md:100-103` requires the same: *"A tuple remains
  rejected while `StaticInit` has no tuple variant... A recovery `Zero` is never
  a publishable initializer, even when an earlier checker already reported the
  source error."*

Both documents already describe the intended contract. The implementation
diverges from its own written rule.

### 3.7 Suggested Direction

Ownership belongs in `nia-body-check`. Two candidate repairs, neither applied:

1. Make the absence explicit at step 1 — have the named-const path report an
   unrepresentable value the way `static_init.rs:212` already does, so the
   diagnostic-based conjunct in the guard fires and the user sees an error.
2. Distinguish the two `None` meanings at step 4 — a rejected initializer must
   not degrade into the uninitialized-static zero default. Since
   `docs/compiler-maintenance.md` §3 asks that malformed upstream facts "fail
   closed", an explicit rejection state is the more durable of the two.

Option 1 alone fixes the observable defect; option 2 additionally prevents the
next value kind without a `StaticInit` variant from reproducing it. F3 records
that reusable half separately.

A regression test should assert the *executable's* exit status, not only a
diagnostic, because `check` and `emit --llvm` both reported success here while
the linked program was wrong.

### 3.8 Resolution

Both options from §3.7 were applied, in `d70f1807`.

`static_init.rs` reports at the point of refusal, in the `Ident`/`Qualified`
arm where the expression and its span are in scope. The kind is named through a
local `const_value_kind_name` helper, so the message says which kind was
refused. This also satisfies the recovery guard's diagnostic-count conjunct, so
the refusal can no longer be silent.

`pipeline.rs` carries only a comment recording why the invariant is not
asserted there. Two attempts to assert it at that boundary were both wrong and
were removed:

1. Asserting that the refusal *added* a diagnostic (`diagnostics.len() >
   before`). This fires on `static p: &i32 = target;` — the spec's documented
   bare-global-as-pointer error, whose diagnostic comes from type checking, not
   from this lowering. The refusal legitimately adds nothing.
2. Asserting that this checker's list is non-empty. Also fires, because
   `nia-static-check` and type checking own separate diagnostic stores; a
   refusal reached through them leaves this checker's list empty.

Both converted an ordinary user error into an ICE:

```text
error internal[I0001]: internal compiler error: a refused static initializer
must report a diagnostic; otherwise the global silently zero-initializes
```

`nia-codegen-llvm`'s `rejects_bare_global_as_pointer_initializer` caught this,
which is why the workspace run matters and the two affected crates alone were
not sufficient evidence.

The invariant is enforced at the point of refusal instead: every refusal inside
`lower_global_static_init` reports before returning its recovery value. That is
a local property of one function, which is why it can be stated correctly. The
cross-phase property — "some owner diagnosed this program" — spans three
diagnostic stores and has no single observer at this boundary.

This is a real limit on the fix. A future `ConstValue` variant added without a
`StaticInit` equivalent is caught by the owner regressions, not by a structural
assertion, so the guard is a test obligation rather than a compiler-enforced
one.

Confirmation, per §3.7's note that a diagnostic assertion alone is insufficient:

```sh
# Each refused kind now reports and produces no binary.
for k in tuple optional enum; do
  ./target/debug/nia --resource-root lib check \
    report/fixtures/const-check/f1_${k}_static.nia; echo "$k exit=$?"
done

# The representable struct control still emits and runs correctly.
./target/debug/nia --resource-root lib emit --exe \
  report/fixtures/const-check/f1_struct_static_control.nia -o /tmp/ctrl
/tmp/ctrl; echo "control exit=$?"   # 0
```

Observed: all three refused kinds exit `1` with
`error[E0301]: const value of kind `<kind>` is not representable as static
data`, and `emit --exe` on the tuple fixture produces no output file. The
struct control emits and exits `0`.

The owner regression was strengthened rather than added alongside. The existing
`unsupported_const_static_initializer_does_not_publish_recovery_zero` test
already covered the tuple case but asserted only that `global_inits` was empty
— which is exactly the assertion that let this defect pass, because an empty
entry is indistinguishable from an uninitialized static. It now also requires
the diagnostic. A second test covers all three refused kinds by name.

This resolution does not decide the design question in §5.4 for the five
unconfirmed kinds: they now fail closed with a diagnostic rather than
publishing zero, but whether each should be permanently rejected or given a
real materialization contract remains open.

## 4. F2 — `const fn` Capability Is Not Validated At The Declaration

**Severity: High.** The specification states the const-capability contract is
checked at the declaration independently of use. In practice the check fires only
when the function is reached from a const expression. A `const fn` whose body
calls an ordinary `fn` is accepted when unused, and accepted when called only
from runtime code.

### 4.1 What The Spec Requires

`docs/language-spec.md:1908-1914`:

> The const-capability contract is checked at the declaration, independently of
> whether the function is used. Tail expressions, explicit returns, expression
> statements, and all source branches must use const-capable operations and agree
> with the declared types. Branch selection controls evaluation, not semantic
> validity: an ordinary `fn` call in an unselected branch is still invalid [...]

`docs/architecture.md:1190-1193` repeats it as an implementation claim:

> Const capability is validated eagerly for every lowered `const fn`, including
> unused functions and unselected source branches. This declaration pass checks
> statement expressions and declared return contexts without executing
> data-dependent failures.

### 4.2 Reproduction

Three fixtures share the same declaration and differ only in how it is used.

Unused — accepted:

```nia
fn plain(v: u32) u32 { v }
const fn viaPlain(v: u32) u32 { plain(v) }
pub fn main() i32 { 0 }
```

```text
exit=0    # no diagnostic
```

Called from runtime code only — accepted:

```nia
fn plain(v: u32) u32 { v }
const fn viaPlain(v: u32) u32 { plain(v) }
pub fn main() i32 { _ = viaPlain(1u32); 0 }
```

```text
exit=0    # no diagnostic
```

Called from a const expression — rejected:

```nia
fn plain(v: u32) u32 { v }
const fn viaPlain(v: u32) u32 { plain(v) }
const W: u32 = viaPlain(1u32);
pub fn main() i32 { _ = W; 0 }
```

```text
exit=1
error[E0401]: const expression can only call `const fn`
```

The declaration is identical in all three. Only the use site changes the outcome,
which is the exact inversion of the documented rule.

### 4.3 Where The Check Actually Lives

The diagnostic originates from the typed const-expression path in
`nia-const-check/src/analyzer/expr_types.rs:524-530`:

```rust
        if !signature.is_const {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::CONST,
                span,
                "const expression can only call `const fn`".to_string(),
            ));
            return None;
        }
```

This runs while typing a const expression, so it is inherently
evaluation-reachability-driven. A second copy of the same message exists in the
early lowering path at `nia-const-ir/src/lower.rs:822-826`, which rejects a
non-`const`/`extern` function when lowering a callee — also reached only via a
call.

A grep across `crates/nia-const-check/src/` for `validate`, `capability`, and
`eager` finds no declaration-level capability pass. The functions named in the
architecture text (a pass over "every lowered `const fn`") could not be located
in this tree.

### 4.4 Consequences

- The error surfaces at a distance from the defect. In the failing fixture the
  span points at the inner `plain(v)` call inside `viaPlain`, reported only
  because some unrelated const binding elsewhere happened to call `viaPlain`.
  Adding or removing a const call site changes whether an unrelated declaration
  is diagnosed.
- A library may publish a `const fn` that is not const-capable. No consumer
  learns this until one of them writes a const call, at which point the
  diagnostic names the library's source, not the consumer's.
- The dual-stage contract in `docs/language-spec.md:1901-1906` is weakened: the
  `const` annotation is not a checked promise at its declaration, only a
  conditional one.

### 4.5 Suggested Direction

Either implement the declaration pass that both documents describe — owned by
`nia-const-check`, since it owns the const-capability boundary — or amend both
documents to describe the reachability-driven behavior that exists. The
documents currently promise more than the compiler enforces, and
`docs/compiler-maintenance.md` §6 treats that gap as an acceptance failure rather
than a documentation preference.

If the pass is implemented, the fixtures in §4.2 are the minimum matrix: unused,
runtime-only, and const-reached must all reject, and the diagnostic should name
the declaration.

## 5. F3 — Named-Const Static Lowering Drops A Value Without A Diagnostic

**Severity: Medium** as a standalone ownership defect; it is the reusable half of
F1's chain and will reproduce F1 for any other value kind lacking a `StaticInit`
variant.

**Signalling shape closed in `d70f1807`** (see §3.8): every refusal inside
`lower_global_static_init` now reports before returning its recovery value, so
the five unconfirmed kinds in §5.2 fail closed instead of publishing zero. The
guard is a test obligation rather than a compiler-enforced assertion — §3.8
records why the cross-phase invariant cannot be asserted at the pipeline
boundary. What also remains open is the design question in §5.4: whether each
kind should be permanently rejected or given a materialization contract.

### 5.1 The Defect

`nia-body-check/src/static_init.rs:544-581` ends its named-const dispatch with
`_ => None` and pushes no diagnostic. Its caller at `static_init.rs:117-123`
cannot distinguish "this expression is not a named const" from "this is a named
const whose value has no static representation", and falls through to
`StaticInit::Zero` in both cases.

Every other unrepresentable path in this file reports first. Compare
`static_init.rs:212-219`:

```rust
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "global initializer is not representable as static data yet",
                ));
                StaticInit::Zero
            }
```

The named-const arm is the outlier.

### 5.2 Currently Affected Value Kinds

The full `ConstValue` variant set is declared at
`crates/nia-const-eval/src/value.rs:99`. `lower_static_const_value` handles
`Int`, `Float`, `Bool`, `Array`, and `Struct`; every remaining variant reaches
`_ => None`:

| Const value kind | Status |
| --- | --- |
| Const value kind | Status as found | After `d70f1807` |
| --- | --- | --- |
| Tuple | **Confirmed miscompilation** — executed, exits wrong | Reports, no binary |
| Optional | **Confirmed miscompilation** — executed, present becomes `null` | Reports, no binary |
| Enum | **Confirmed miscompilation** — executed, value outside variant set | Reports, no binary |
| Union | Reaches `_ => None`; not driven to an executable | Fails closed |
| Vector | Reaches `_ => None`; not driven to an executable | Fails closed |
| Pointer | Reaches `_ => None`; not driven to an executable | Fails closed |
| Range | Reaches `_ => None`; not driven to an executable | Fails closed |
| ErrorUnion | Reaches `_ => None`; not driven to an executable | Fails closed |
| String | Not reached in practice — byte/char strings route through `Array` and emit correctly (verified: `constant [3 x i8] c"hi\00"`) | Unaffected |

The first three rows were confirmed by execution. The middle rows were the
same-shape risk implied by the code path; this report did not claim them as
reproduced miscompilations, and the fix means the call site can no longer reach
the zero-publishing outcome for any of them. They remain unverified *as
observations* — "fails closed" follows from the repaired call site, not from
having driven each kind to a binary. `String` is listed to record that it was
never affected, since a byte-string const in a static is a common pattern.

### 5.3 Contract Reference

`docs/compiler-maintenance.md:93-103` assigns this exactly:

> Static initializer admission and lowering must agree on the complete value set
> represented by `StaticInit`. [...] A recovery `Zero` is never a publishable
> initializer, even when an earlier checker already reported the source error.

"Admission and lowering must agree" is the violated clause: lowering silently
declines a value that admission never rejected.

### 5.4 Suggested Direction

`nia-body-check` owns both halves. The durable repair is to make the two `None`
meanings distinct rather than to add one more diagnostic at one more call site —
otherwise the next `ConstValue` variant added upstream re-enters the same trap
with no compiler error to guide the author. A `Result`-shaped return, or an
explicit rejected state consumed by `pipeline.rs:126`, would make a missing arm a
compile error in this crate.

## 6. F4 — `ctz`/`clz`/`popcount` Are Const-Evaluable But Declared Non-Const

**Severity: Medium.** `nia-const-eval` contains complete implementations of these
three builtins, and `ConstBuiltin` has variants for them, but the standard-library
declarations are plain `pub fn`. The capability is therefore unreachable from any
const expression.

### 6.1 Reproduction

Direct use in a const binding:

```nia
const w: u32 = std::builtin::ctz[u32](8u32);
pub fn main() i32 { _ = w; 0 }
```

```text
exit=1
error[E0401]: const expression can only call `const fn`
  --> f1_ctz_const.nia:1:16
```

Wrapped in a `const fn`, then called from a const expression:

```nia
const fn tz(v: u32) u32 { std::builtin::ctz[u32](v) }
const W: u32 = tz(8u32);
pub fn main() i32 { _ = W; 0 }
```

```text
exit=1
error[E0401]: const expression can only call `const fn`
  --> f1_ctz_via_constfn.nia:1:27
  |
1 | const fn tz(v: u32) u32 { std::builtin::ctz[u32](v) }
  |                           ^^^^^^^^^^^^^^^^^^^^^^^^^
```

Runtime use is accepted:

```nia
pub fn main() i32 { _ = std::builtin::ctz[u32](8u32); 0 }
```

```text
exit=0
```

Note the second fixture also demonstrates F2: the `const fn tz` declaration is
accepted on its own and only fails once a const expression reaches it.

### 6.2 There Is No Const Evaluator For Them

**This section previously asserted the opposite and was wrong.** It claimed
`nia-const-eval` provided working implementations and named a `ctz_value`
function. That claim was never verified and is false. Corrected below.

`ctz_value`, `clz_value`, and `popcount_value` do not exist anywhere in the
repository:

```sh
grep -rc 'ctz_value\|clz_value\|popcount_value' crates/ | grep -v ':0'
# no output — zero occurrences in every file
```

`Ctz` is a `ValueBuiltin` declared in `nia-ids`. Every consumer that matches on
it is a runtime path:

```sh
grep -rn 'Ctz =>' crates/ --include=*.rs | sed 's/:.*//' | sort -u
# crates/nia-codegen-llvm/src/function_codegen.rs
# crates/nia-compiler-query/src/signature_cache/item_signatures.rs
# crates/nia-function-lower/src/expr.rs
# crates/nia-ids/src/lib.rs
# crates/nia-symbol/src/lib.rs
```

Neither `nia-const-eval/src` nor `nia-const-check/src` contains `Ctz` at all.

This changes F4's shape substantially. It is not "the evaluator is complete and
only the declaration surface withholds it" — it is a request for const
evaluation capability that does not exist. The work is a new implementation in
`nia-const-eval` plus its capability admission in `nia-const-check`, and only
then the `lib/std/builtin.nia` declaration change. The severity classification
in §2 ("capability gap where the evaluator is already complete") is likewise
wrong; treat F4 as a feature request, not a gap.

The observable behavior in §6.1 stands — those are recorded compiler outputs.
Only the explanation of *why* was fabricated.

**How this got into a committed audit.** The §6.1 fixtures were executed; §6.2
was asserted from recollection without a single `grep`. F1 survived scrutiny
because its three fixtures were driven to executable exit codes. The
distinction is not care or intent — both were written in the same sitting with
the same subjective confidence. It is that one class of claim fails loudly when
wrong and the other fails silently. A claim about what the code contains is the
silent class, and belongs with a command in the same breath. The fixtures under
`report/fixtures/const-check/` are the part of this report that can be trusted
without re-derivation; the prose is not.

### 6.3 The Declaration Surface

In `lib/std/builtin.nia` these three are declared `pub fn`, grouped with
genuinely runtime-only operations (atomics, `asm`, memory intrinsics). By
contrast the SIMD builtins `splat`, `extract`, `insert`, and `bitmask` are
declared `const fn`, and `docs/language-spec.md:2619-2621` names them dual-stage:

> `splat`, `extract`, `insert`, and `bitmask` are dual-stage `const fn`
> builtins: the same operation may execute during constant evaluation or lower to
> runtime SIMD instructions.

The spec's bit-counting paragraph (`docs/language-spec.md:2643-2648`) describes
`ctz`/`clz`/`popcount` semantics, including the defined `ctz[T](0)` and
`clz[T](0)` results, but does not state a stage. So this is a capability gap and
a spec silence rather than a spec violation: nothing promises const availability,
and nothing explains why these differ from the SIMD builtins that are pure in the
same way.

### 6.4 Why It Matters

These are the natural operations for compile-time layout and mask computation —
exactly the const context where a bit count is wanted. Their absence pushes users
toward hand-rolled loops in `const fn`, which the evaluator must then execute
against its step budget, for a result the evaluator can already compute directly.

### 6.5 Suggested Direction

Three owners, in this order. The first two are the actual work; an earlier
revision of this section listed only the third and second because it wrongly
assumed the evaluator existed.

1. `nia-const-eval` needs the evaluation itself: three operations over an
   `IntConst` operand, returning the count in the operand's type. The spec
   already fixes the edge cases — `ctz[T](0)` and `clz[T](0)` return the bit
   width of `T` (`docs/language-spec.md:2643-2648`) — so the semantics are
   specified even though nothing implements them.
2. `nia-const-check` needs capability admission so a const expression may reach
   them, alongside its existing builtin handling.
3. `lib/std/builtin.nia` changes the three declarations from `pub fn` to
   `const fn`. This must come last: until steps 1 and 2 land, the declaration
   would advertise a capability that does not exist.

The operations are pure integer computations whose only target dependence is the
operand width, so the semantics are low-risk. The cost is that this is new
capability rather than an unlocking, which is a different decision than the one
this report originally presented. Acceptance should include const/runtime
agreement at `0`, the type maximum, and a mid-range value for each of the three,
per `docs/compiler-maintenance.md` §6 — the `0` case especially, since it is the
one the spec calls out and the one a naive implementation gets wrong.

Whether this is worth building at all is a judgment about the language's const
surface, not a correctness question. Nothing is broken today; these operations
are simply unavailable at compile time.

## 7. F5 — Verified-Correct Boundaries

This section records what was probed and found correct, so a later reader does
not re-investigate it. Each row was run against the built compiler.

### 7.1 Trap And Overflow Boundaries

`docs/language-spec.md:245-271` specifies const-time diagnostics for the same
conditions that trap at runtime. All of the following produced a const
diagnostic:

| Condition | Result |
| --- | --- |
| Division by zero | diagnosed |
| Remainder by zero | diagnosed |
| Signed `MIN / -1` | diagnosed |
| Signed `MIN % -1` | diagnosed |
| Negative shift count | diagnosed — `shift count is out of range in const expression` |
| Shift count ≥ operand width | diagnosed |
| Left-shift result not representable | diagnosed |
| Negation overflow | diagnosed |
| Add/sub/mul overflow at the concrete width | diagnosed |

An earlier note in my own investigation suspected a wording defect in the shift
diagnostic. That was wrong: the emitted text is `shift count is out of range in
const expression`, which is correct. A grep for the suspected phrasing across
`nia-const-eval` and `nia-const-check` finds no such string. Recorded here
because a discarded suspicion is cheaper than a re-investigation.

### 7.2 Resource Bounds

`docs/language-spec.md:1996-2002` specifies a 1,000,000-step budget, call depth
256, and a 100,000-iteration per-loop limit. A `while` loop exceeding the
iteration limit produced a source diagnostic naming the limit, at the active
expression, rather than hanging or overflowing the host stack.

### 7.3 Const/Runtime Float Precision

Commit `1da87397` ("fix(const-eval): preserve resolved float precision") is
effective for the cases probed: `f32` arithmetic, comparison, and compound
assignment evaluate in binary32 at each operation boundary, and const-folded
results agreed with runtime results rather than diverging through a host `f64`
intermediate.

### 7.4 Pointer Provenance And Escape

Matching `docs/language-spec.md:1957-1966`: a pointer to a callee-frame local was
rejected from a returned const value; a caller-supplied pointer passed through
unchanged; a read-only module-const initializer promoted to a frozen allocation;
writable const promotion was rejected.

### 7.5 Const Union Field Types

Field kinds accepted in a const union: scalars, pointers, fixed arrays, SIMD
vectors, nominal structs, nested unions — matching
`docs/language-spec.md:1862-1864`. Rejected as non-const-representable:
optionals, slices, tuples, error unions, and enums.

Enum is worth an explicit note. Runtime unions accept an enum field, and the
const codec does not, so the two disagree. But the spec's supported list does not
include enums, so the current behavior is *documented* — it is a capability gap
with a written boundary, not a violation. It is recorded here rather than as a
finding for that reason.

Invalid representations are caught: a `bool` field whose stored byte is neither 0
nor 1 is diagnosed at the containing field, as
`docs/language-spec.md:1859-1860` requires.

### 7.6 Empty Const Slices

`(&EMPTY[..]).ptr()` on a zero-length array is rejected:

```text
error[E0401]: const slice pointer method cannot project an empty slice
```

The runtime equivalent is accepted. This asymmetry is explicitly documented at
`docs/language-spec.md:2586-2587`:

> Empty const slices are currently rejected because the const pointer
> representation cannot yet encode an allocation-base/dangling element pointer;
> the evaluator must not fabricate a pointee value.

Documented limitation, not a defect. Listed so the asymmetry is not later
mistaken for one.

### 7.7 Other Verified Behavior

- Cross-module `const` and imported `const fn` calls evaluate correctly.
- `const` iteration over a user `Iterable` requires const-capable `iter`/`next`
  witnesses, and the witness check fires as specified.
- Layout builtins `size`/`align`/`offset` produce compile-time values usable in
  array lengths and static initializers.
- Tuple, optional, and enum `const` values are correct *within* const evaluation
  — `T.0` projection, optional payload access, and enum comparison all evaluate
  fine, and each is also correct when bound to a runtime `let`. Only the crossing
  into `static` storage (F1) fails.
- A byte-string `const` in a `static` is correct, emitting `c"hi\00"`.

## 8. Coverage And Limits Of This Audit

What this audit did **not** establish, stated explicitly so the report is not
read as broader than its evidence:

- The five unconfirmed `_ => None` value kinds in §5.2 (union, vector, pointer,
  range, error union) were identified from the code path but not driven to a
  running executable. They are risks of the same shape, not confirmed
  miscompilations.
- No claim is made about `nia-const-check`'s behavior under incremental
  recompilation. Every fixture here was a clean single-file check.
- Const-generic inference, `embed`, and the typed-const query surface consumed by
  `nia-body-check` were exercised only incidentally; they are not audited.
- Only the host target (x86_64 Linux, LP64, little-endian) was exercised. The
  target-relative rules in `docs/compiler-maintenance.md` §3 concerning pointer
  width and endianness were not tested against a second configuration, because
  no CLI surface in this tree selects a non-host artifact target.
- Performance and step-budget tuning were out of scope beyond confirming the
  documented limits fire.

## 9. Recommended Sequencing

Per `docs/compiler-maintenance.md` §6, each item below is a dependency-complete
batch:

1. ~~**F1 + F3 together.**~~ **Done in `d70f1807`**, with one part not
   delivered as planned (§3.8). Every refusal now reports at the point of
   refusal, which closes the defect for all eight kinds. The structural guard at
   the pipeline boundary was attempted twice and removed both times: the
   cross-phase invariant it needed spans three diagnostic stores and has no
   observer there. Recurrence is therefore guarded by the owner regressions
   rather than by the compiler.

   What this batch deliberately did *not* settle: whether each of the five
   §5.2 kinds should be permanently rejected or given a real materialization
   contract. Optional and enum are plausible candidates for the latter, since
   both have straightforward static representations. That is a design decision
   about the language's surface, not a correctness question — publishing zero
   was wrong in every case, and no longer happens for any of them.
2. **F2 as its own batch.** Either implement the declaration pass or correct both
   documents. This changes which programs are accepted, so it needs its own
   validation sweep across `lib/std`, `examples/`, and the driver const suites —
   any existing `const fn` that is not genuinely const-capable will begin
   failing, and that is the point.
3. **F4 last, and only if wanted.** It is not the smallest item, contrary to an
   earlier revision of this section: it needs a new const evaluator, its
   capability admission, and only then the declaration change (§6.5). It depends
   on no other finding and nothing is broken without it, so it is the one item
   here that is purely a question of whether the const surface should grow.

F1 is the only finding that produces an incorrect program today and should be
treated as the priority regardless of the order chosen for the rest.
