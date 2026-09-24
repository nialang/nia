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
type, and snapshots the shared root identity. Other recovery-derived diagnostics
still need explicit provenance before this stage is complete; the for-in path now
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
statuses for check and emit, with test/build workflow status coverage. A Linux pseudo-terminal integration
Build and test failures now also share an explicit text/JSON captured-output
contract, including empty stdout, stable exit status, and the diagnostic JSON
envelope.
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
and following statements, with parser and driver coverage. Other parser
recovery boundaries still need equivalent source coverage. Associated
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
Function pointer and callable interface type parameter lists likewise retain
later parameters and the enclosing function after missing types at commas, with
parser and driver coverage.
Generic parameter lists also skip a missing parameter name at a comma, retain
later parameters and the enclosing function, and have parser and driver
coverage. They also synchronize missing commas between clearly identifiable
parameters and before the closing bracket, retaining later parameters while
keeping the enclosing function parseable.
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
Attribute argument lists now synchronize a missing expression at the next
comma or closing parenthesis, retaining later arguments and the attributed
item. They also diagnose a missing separator between valid arguments and retain
the remaining arguments and attributed item, with parser and driver coverage.
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
LLVM failures by backend and linker tests. The remaining fixture work is
cross-product coverage for those operational paths plus broader source-origin
and recovery provenance; the code registry itself is not evidence that every
combination of phase, source ownership, and suppression has been exercised.

The broad workspace test, clippy, formatting, and CLI case suites remain
mandatory for each stage.
