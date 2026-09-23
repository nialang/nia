<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Diagnostics Roadmap

Nia's terminal diagnostics are a compiler product, not a last-mile string
formatter. A useful report must answer four questions in order:

1. What source construct is wrong?
2. Which compiler rule rejected it?
3. What other location or declaration explains the rule?
4. What can the user change to continue?

The implementation is being upgraded in bounded stages. Each stage must keep
the existing source spans and diagnostic codes stable unless the old contract
was actively misleading.

Status: in progress. The implemented paths below are incremental coverage, not
completion of the whole roadmap. Remaining work includes broader source-origin
and recovery provenance, rule-specific semantic diagnostics, and fixture/snapshot
coverage for the complete diagnostic rule set. Non-Linux terminal validation is
deferred to the 0.3.0 multi-platform port and is not a gate for this roadmap.

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

## Stages

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

This coverage is still incremental. Qualified callable lookup outside unknown
members and restricted extension functions, cross-module edge cases among
visibility scopes, and recovery-derived semantic rules outside the listed call,
propagation, and builtin cases still need rule-specific contract audits and
source fixtures. Ambiguous extension-method resolution now keeps its specific
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
type, and snapshots the shared root identity. Other recovery-derived diagnostics
still need explicit provenance before this stage is complete; the for-in path now
suppresses its recovery products at the semantic checker boundary.

### 5. CLI interaction contract

Add stable text and machine-readable output modes, explicit summary counts, and
consistent exit status behavior for check, emit, build, and test. Terminal
rendering should provide color and compact context when interactive, while
non-interactive output remains deterministic and snapshot-friendly.

Text and JSON reports now expose the same retained error/warning summary and
duplicate/display-limit counts. Interactive text diagnostics are colorized at
the CLI boundary, while captured and JSON output remains deterministic and
free of ANSI control sequences. CLI cases now assert exact success and failure
statuses for check, emit, build, and test. A Linux pseudo-terminal integration
test verifies color on interactive text output and ANSI-free JSON output on the
same terminal; interactive text also honors `NO_COLOR`. Other terminal
platforms are deferred to the 0.3.0 multi-platform port. Broader output snapshot
coverage remains to be verified.

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
top-level declarations, with parser AST and driver report coverage. Statement
recovery now tracks nested parentheses, brackets, and braces, so a failed
control-flow expression cannot stop at a semicolon inside its discarded body;
the enclosing block and later declarations remain parseable with parser AST and
driver report coverage. Statement-keyword recovery also consumes a terminator
following a discarded nested block, so malformed `return` expressions do not
emit a second empty-expression error. Unresolved expression statements now
suppress their discarded-result recovery diagnostic at the body-check boundary,
with a driver snapshot proving the root name error and an independent discarded
literal remain distinct. Malformed
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
comma boundaries, with separate end-to-end snapshots. Other parser recovery
boundaries still need equivalent source coverage. Call argument lists now also
skip missing expressions at comma boundaries while retaining later arguments and
following statements; a parser unit test covers the AST boundary and a driver
snapshot covers the rendered diagnostics. Array and struct literal lists now
also skip missing expressions at comma
boundaries while retaining later elements and fields; a parser unit test and
driver snapshot cover that boundary.
Function pointer and callable interface type parameter lists likewise retain
later parameters and the enclosing function after missing types at commas, with
parser and driver coverage.
Generic parameter lists also skip a missing parameter name at a comma, retain
later parameters and the enclosing function, and have parser and driver
coverage. They also synchronize a missing comma before the closing bracket so
the enclosing function remains parseable.
`where` predicate lists use the same local recovery for a missing predicate
type, retaining later predicates and following functions with parser and driver
coverage.
Associated type binding arguments also recover a missing binding value at a
comma, retain later arguments and the enclosing function, and have parser and
driver coverage; the recovery path suppresses the parser's generic expected
type duplicate when the more specific binding diagnostic is emitted.
Explicit type argument lists also synchronize a missing comma before `]` and
retain the enclosing function and following declarations.
Function parameter lists likewise synchronize a missing comma before `)` while
preserving the current function and later top-level declarations.
Closure parameter lists use the same next-parameter boundary detection and
recover a missing comma before `->` without losing the enclosing function.
They also skip a missing parameter name at a comma and retain later parameters
and the closure body with parser and driver coverage.
Function pointer, callable interface, and tuple type element lists now report
and recover a missing comma before `)` as one local type-parameter boundary.
Using selector groups now synchronize an invalid member at the next comma or
closing brace, retaining later selectors and the following top-level item.
Attribute argument lists now synchronize a missing expression at the next
comma or closing parenthesis, retaining later arguments and the attributed
item.
Closure capture lists now synchronize a missing capture name at the next comma
or closing bracket, retaining later captures and the enclosing closure.
They also diagnose a missing comma before a clearly identifiable next capture
and continue parsing that capture.
Bracket argument lists now skip a missing argument at a comma and retain later
arguments, the surrounding type or expression, and following top-level items.
Match arm pattern lists now skip a missing pattern at a comma and retain the
later patterns, arm body, and following statements.
Tuple and nominal tuple match patterns now skip a missing field at a comma and
retain later fields and arms; parser and driver snapshots cover both recovered
shapes.
Named nominal match patterns likewise synchronize a missing field pattern at a
comma or closing brace, retaining later fields and arms with parser and driver
coverage.
Enum variant lists now recover a missing variant name at a comma or closing
brace, retaining later variants and following top-level declarations with
parser and driver coverage.
Match arm bodies now synchronize an invalid arm at the next top-level comma or
closing brace, retaining later arms and statements after the match.
These are recovery gaps in the grammar parser that consumes the lossless token
view. `nia-syntax` currently
preserves source, trivia, malformed tokens, and delimiter groups in its green
tree, with borrowed red views and conservative single-token partial rewrites;
grammar productions and AST recovery remain in the separate token-cursor parser.
This diagnostics roadmap audits that parser's error and recovery behavior. A
grammar-shaped green tree or broader incremental grammar reparse would be a
separate architecture change and is not implied by the recovery fixes here.
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
Binary and shift operator recovery is covered end to end: unresolved operands
retain their name-resolution roots while operator trait and operand-shape
consequences are suppressed.
The same root-only contract is covered across field access, indexing, deref,
address-of, casts, optional propagation, and calls applied to an unresolved
value.
Atomic builtin recovery has a snapshot proving invalid RMW operation codes stay
visible alongside unknown type arguments while type-dependent atomic errors are
suppressed. An unaligned-load recovery snapshot proves an unresolved pointer
keeps only its name-resolution root. A slice-length recovery snapshot proves an
unresolved value keeps only its name-resolution root.
Directly qualified private values and types now retain the owning dependency
source path for their declaration locations instead of rendering those spans
against the use-site file. The module graph exposes each module's source path
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
LLVM failures by backend and linker tests. The remaining fixture work is
cross-product coverage for those operational paths plus broader source-origin
and recovery provenance; the code registry itself is not evidence that every
combination of phase, source ownership, and suppression has been exercised.

The broad workspace test, clippy, formatting, and CLI case suites remain
mandatory for each stage.
