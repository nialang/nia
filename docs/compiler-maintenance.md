# Compiler Maintenance Contract

Status: normative compiler maintenance policy

This document records the engineering discipline that must survive individual
compiler roadmaps. It complements the implemented architecture described in
[architecture.md](architecture.md), the repository rules in
[project-conventions.md](project-conventions.md), and the measurement contract
in [performance.md](performance.md).

Temporary roadmaps may define sequencing and acceptance for a bounded project.
They are not permanent architecture documents. When a roadmap closes, durable
rules belong here or in the relevant reference document; progress logs remain
available through Git history.

## 1. Root-Cause Changes

Compiler maintenance must repair the owning abstraction and its complete data
flow, not only the first failing call site.

- A migration is complete only when the obsolete entry point, identity,
  fallback, duplicate source of truth, and old/new dual path are physically
  removed.
- A compatibility adapter may exist only at one explicit, advancing migration
  boundary. Its deletion belongs to the same bounded project; it must not become
  a permanent public API.
- A large call surface is evidence that a contract crosses many owners. It is
  not a reason to weaken the target architecture or preserve the old contract.
- Do not fix ownership problems with broad `Clone`, `Arc`, interior mutability,
  side stores, or service-locator callbacks unless that is the intended
  long-term ownership model.
- Do not add driver special cases, hidden environment switches, cache exceptions,
  or test-only production behavior to make an architectural migration appear
  complete.

The preferred sequence is to identify the owner, the consumers, the stable
identity, and the only source of truth; change that contract; follow compiler
errors through every affected consumer; then delete the superseded path.

## 2. Failure And Diagnostic Discipline

Invalid source, unsupported input, query cycles, I/O failures, cache corruption,
and other expected failures use explicit result and diagnostic channels.

- Query failures propagate through `QueryResult` or another typed result. Panic
  and unwind are not ordinary error transport.
- User-facing compiler failures are registered in the compiler or loader
  diagnostic store and are carried through stable bundle handles where query
  products need to refer to them.
- Semantic query values should not own repeated diagnostic vectors. Algorithms
  may use short-lived local buffers while constructing the canonical bundle.
- Panic is reserved for genuine internal invariants and ICEs. It must reach the
  designated ICE boundary rather than being caught and reinterpreted as a user
  error.
- Persistent diagnostic data must use stable source identity and validated
  spans. It must not serialize session-local source, module, revision, or bundle
  handles.
- Keep numeric-separator grammar with the lexer: `_` is valid only between two
  digits in the active radix, and recovery consumes the complete malformed
  literal as one error token. Literal decoders must validate this independently
  before deleting separators because cached or synthesized phase products can
  reach them without source tokenization.
- Keep numeric body/suffix splitting and decoder suffix validation in
  `nia-literals`. Semantic consumers map the shared suffix to their own type
  representation; they must not rescan integer, fraction, and exponent forms or
  let a decoder erase an unknown suffix.
- Decode a source integer token as an unsigned 128-bit magnitude; source signs
  remain unary expressions. Carry signedness plus complete bits in `IntConst`
  after lowering, and test source-literal representability against the target
  primitive and pointer width without narrowing through `i128`.
- Negate an unsigned source magnitude before interpreting signed bits so the
  `i128::MIN` endpoint remains representable. Keep this distinct from negating
  an already signed `i128::MIN`, which is an overflow. Normalize the endpoint
  builtin call before Function IR rather than weakening backend literal range
  validation for an otherwise invalid positive `i128`.
- Validate float-to-integer const casts with an exclusive power-of-two upper
  bound for the concrete target width, then construct the target-signed
  `IntConst` directly. Do not compare against integer maxima converted to float
  or route unsigned results through a saturating signed host cast.
- Resolved const numeric operations must apply the concrete primitive precision
  at every operation boundary. In particular, execute `f32` arithmetic in
  binary32 and narrow its operands and result before later casts, comparisons,
  bindings, or compound-assignment writeback; narrowing only the final value
  permits const/runtime divergence through host `f64` intermediates.
- Static initializer lowering must thread the checked destination type through
  every data initializer. Target propagation may normalize integer signedness
  while preserving the complete `IntConst` bit pattern; explicit source casts
  remain the only boundary that masks to a narrower integer width.
- Resolved compound assignments must query the concrete primitive at the target
  leaf, including field and index paths, and apply its integer width and
  signedness before writeback. A final assignment conversion is too late:
  `u8 += 1` must diagnose the intermediate overflow just as runtime code does.
- Static initializer admission and lowering must agree on the complete value
  set represented by `StaticInit`. Named const values of integer, float,
  boolean, and fixed SIMD-vector type (including local and imported bindings)
  lower to their actual payload; vector lanes and tuple positions use explicit
  initializer variants, while arrays, tuples, and nominal structs are admitted
  recursively only when every leaf has an equivalent static-data
  representation. Aggregate lowering uses the checked destination element,
  tuple position, and canonically substituted field types, and restores integer
  signedness at every leaf without truncating its bits. Unions, pointers, and
  other const values remain rejected until they have an explicit equivalent
  materialization contract. A recovery `Zero` is never a publishable
  initializer, even when an earlier checker already reported the source error.
- Treat LLVM-emitted wide arithmetic/conversion libcalls as reachable compiler
  builtins, not implicit host linker dependencies. Collect them from typed
  Function IR, include every requested symbol in the builtins fingerprint, and
  implement wide conversions from native-width operations so their definitions
  cannot lower back into the same unresolved helper.
- Keep signed and unsigned wide conversion symbols paired in the same owner;
  signed conversion should normalize magnitude first and apply two's-complement
  negation afterward, preserving the `i128::MIN` endpoint without a signed
  conversion helper recursion.

An error-path change is not accepted until tests cover both the ordinary
diagnostic result and the invariant/ICE boundary it intentionally leaves.

## 3. Identity, Ownership, And Products

Hot-path identities are typed, compact, and session-local. Persistent identity
is a separate canonical representation.

Cross-component release, toolchain, ABI, persisted-format, and cache-namespace
identities are owned by the dependency-free `nia-compat` registry. Product
owners retain their encoders, decoders, bounds, checksums, and corruption
policy, but must consume registry values directly rather than exporting aliases.
`lib/toolchain.meta` is generated registry data, and the workspace package
version is the only release-version input.

Fingerprint domains are owner-local `nia_query::FingerprintDomain` constants,
not raw builder strings or a central list detached from the hashed inputs. Each
distinct input contract has one domain declaration; deliberate reuse shares
that declaration. Changing the encoded inputs or their meaning requires a
domain-version increment, and the compatibility audit rejects malformed,
duplicated, or inline production domains.

A public-version reset, compatibility epoch, or development-schema renumbering
requires its own release proposal after audit and representative project
testing. The registry preserves current values until that proposal is accepted;
it does not make a reset implicit.

- Never serialize a session-local index or infer stable identity from allocation
  order, module number, query slot, pointer value, or debug formatting.
- Reclaiming stores use owner/index/generation or an equivalent stale-handle
  boundary. Append-only session stores never reuse an index for a different
  meaning.
- A query product has one clear owner and a storage policy chosen by the query
  declaration, not by a caller selecting between owned and shared APIs.
- Large immutable products stay cache-owned and are borrowed or referenced.
  Products that must be optimized or consumed uniquely move through an owned
  query boundary instead of being deeply cloned.
- Backend products contain backend facts and stable semantic handles, not
  snapshots of semantic stores. Compiler phases receive only the capabilities
  required for their work.
- Backend vtable ownership deduplicates complete `(self type, object type)`
  keys through structural type equivalence before selecting a deterministic
  source owner. Rebuilt nominal or const-generic handles must not produce
  duplicate LLVM tables merely because their interner identities differ.
- Backend aggregate-instance ownership applies the same semantic-key rule to
  struct and union instances. Equal nominal definitions from different module
  products are compared by type/const arguments and recursively checked field
  payloads before deterministic owner selection; raw instance-key hashing is
  not a semantic identity boundary.
- Backend-lower trait-object and projection comparisons treat associated
  bindings as unordered semantic sets. Candidate matching must be a complete
  bijection with backtracking; a greedy first equal key can consume a value
  needed by a later duplicate key and make equivalent types appear different.
- `TypeEquivalence::same_const_generic_args_for_equiv` is the shared fallback
  for const metadata and compares integer values by bits, independent of their
  signed/unsigned representation. Specialized adapters may add evaluator
  context, but must preserve this semantic rule when delegating to the common
  structural walker.
- Trait obligations remain complete across phase and consumer boundaries. The
  self type, trait identity, type and const arguments, and associated-type
  bindings are one semantic product; candidate filters and final validators
  must not silently reduce it to a bare trait goal. Consumers of impl-signature
  products check exact argument arity before pairwise matching so malformed or
  stale upstream facts fail closed instead of benefiting from truncated `zip`
  comparisons. Inference probes clone their substitution state and commit it
  only after the complete impl header and associated binding set matches; every
  recursive composite pattern is one transaction, so a failed nested field
  (including tuples, callable signatures, and nominal arguments) must not leak
  a partial generic inference. Associated
  binding vectors are unordered semantic sets: comparisons consume every
  candidate at most once and backtrack over compatible keys, since a greedy
  first match can reject a later valid permutation. This applies to fast shape
  filters and specialization ordering as well as final impl selection; a
  permissive prefilter is still unsound when it changes which candidate wins.
  Backend extension instantiation and backend type equivalence preserve this
  rule so code generation cannot select a weaker instance than body checking.
- Const-execution generic inference treats associated-type bindings as an
  unordered bijection. Each actual binding may satisfy at most one pattern
  binding, and candidate substitutions are committed only after the complete
  permutation succeeds; otherwise a repeated binding can hide an incompatible
  sibling obligation.
- Body-check generic inference uses the same unordered, backtracking bijection
  for associated-type bindings. A generic key may initially match an earlier
  actual binding but must yield it when a later concrete key needs that slot;
  publish the inferred type or const map only after a complete assignment.
- Supertrait declarations are persisted as complete trait obligations too. Any
  associated-type binding attached to a supertrait travels through collection,
  type-root discovery, cache encoding, body assumptions, impl validation, and
  trait-object object-safety/method/upcast consumers. A child object inherits
  declared supertrait equalities; callers need not repeat them in its object
  spelling, while unrelated or incompatible parent bindings remain rejected.
  Adding such a field requires a persisted-format version change so old entries
  fail closed instead of being read with a shifted positional layout.
- Supertrait assumptions are checked as a graph, not as a flat list. A repeated
  parent instance with the same associated-type value is an allowed diamond;
  the same parent instance with different values is a declaration error. Keep
  the full parent identity (trait, type arguments, const arguments, and
  associated name) in diagnostics and tests so later consumers cannot inherit
  an ambiguous projection.
- Trait-object reachability traversals must keep cycle guards path-local. Always
  remove a node before returning from a DFS branch, including error or missing
  signature paths; a global visited set can incorrectly hide a valid sibling
  supertrait.
- Executable-reachability extension matching is transactional at every type
  pattern boundary. A tuple, nominal, array, callable, or trait-object match
  may discover substitutions before a later field fails; the failed candidate
  must leave type, const, and array-length maps unchanged before another impl
  is tried.
- Generic trait-reachability DFS keeps its definition recursion guard
  path-local and checks it before inserting the complete instantiation key into
  `visited`. A recursively encountered sibling instance may be deferred while
  its definition is active, but must not be permanently suppressed by a stale
  visited mark from that rejected branch.
- Frontend vtable-instantiation facts and backend vtable-entry lowering must
  traverse the same source-supertrait graph. Substitute type and const
  arguments at each edge and key the active path guard by the complete trait
  instance; keep a separate per-coercion expanded set so diamond paths do not
  duplicate semantic dependency facts. Otherwise inherited default methods can
  be omitted from semantic facts even when backend lowering emits their slots,
  or the same method instance can be recorded repeatedly.
- Trait-object upcasts must use the same unordered bijection semantics as other
  associated-binding consumers. Candidate source bindings are speculative until
  all target bindings match, so a failed later binding cannot leave an earlier
  greedy choice committed.
- Trait and impl witness filtering must call the canonical module-graph
  visibility predicate. Keep `Public`, `PublicPkg`, `PublicSuper`, and private
  behavior aligned with ordinary item lookup; do not duplicate a public-only
  shortcut in visibility consumers.
- Expand substituted impl where predicates from the bound trait's declared
  methods and supertraits, using the same closure as direct generic predicates.
  Never reuse the outer extension method name as a builtin witness fallback;
  doing so can filter out the concrete operator method while an instantiated
  backend body still references it, leaving LLVM declaration ownership absent.
- Public standard-library owned iterator state types must expose one canonical
  constructor that establishes private state invariants. Keep range-literal
  conversion helpers private when they only adapt builtin range values to that
  constructor; keep borrowed or raw-backed iterator construction with its
  source container rather than exposing raw parts.
- Validate every bounded-range `Step` or `StepBack` result before committing
  iterator state. The candidate must move strictly in the requested direction
  without crossing the live endpoint; otherwise exhaust the iterator without
  yielding the invalid candidate.
- Keep builtin range-step implementations exhaustive over the language's
  signed and unsigned integer primitive sets. Endpoint tests must cover both
  zero and the type maximum so adding a width cannot leave a one-sided or
  overflow-prone iterator implementation.
- Trait-object object-safety checks must reject a source trait whose supertrait
  graph requires builtin `Sized`: the erased object has no statically known
  layout, so accepting that relationship would make the object contract
  impossible. Builtin supertraits with methods or associated items must also
  be rejected until their object-level vtable contract is explicitly defined;
  marker-only builtin bounds remain eligible for their existing semantics.
- Source trait associated values/consts require an explicit object-level
  contract. Until vtable metadata and lookup define one, reject such traits at
  object construction rather than silently erasing the item while retaining a
  source-level promise.
- Object-safety type traversal must inspect every type-bearing part of a method
  signature, including builtin array-length metadata and the type of each
  nominal, trait-object, projection, or associated-binding const argument.
  `Self` in any of these positions changes the erased ABI or object identity and
  must be diagnosed just like `Self` in an ordinary nested type. Erased-type
  reconstruction must recursively normalize the same metadata so accepted
  signatures retain their complete layout and identity.
- Associated-type projection normalization during object-safety checks must
  match a binding by the complete trait instance: trait identity, type
  arguments, and const arguments. The root trait's const arguments must remain
  available for unqualified bindings, while inherited qualified bindings use
  their own const arguments; matching only the trait and type arguments can
  resolve a projection against a sibling const instance and hide `Self` from
  object-safety validation.
- Monomorphization depth limits must inspect type structure carried by const
  arguments as well as ordinary type arguments. Check `ConstGenericArg::ty` at
  both recursive type traversal and concrete instance admission, or deeply
  nested const metadata can bypass convergence protection.
- Monomorphization depth limits must inspect the type operand of
  `ArrayLenTy::Builtin` as well as an array's element. A deeply nested type can
  be carried only by `size[T]` or `align[T]`; skipping that operand allows an
  otherwise unbounded concrete instance to evade the same convergence guard.
- Const-function generic inference must treat the type operand of
  `ArrayLenTy::Builtin` as part of the expected type. When another argument
  provides the same type parameter, `size[T]`/`align[T]` metadata must not cause
  the expected array to be classified as permanently concrete and emit a
  premature inference failure.
- Const-call generic inference must recurse through equivalent layout builtin
  operands before validating the substituted target. A `size[T]` operand can
  be the only type evidence for `T`; collect it during inference, then let the
  final substituted-type check reject genuinely incompatible layouts.
- Body-check generic call shape checks must recurse through the type operand of
  `ArrayLenTy::Builtin`. A generic parameter hidden only in `size[T]`/`align[T]`
  makes the expected array incomplete; checking an array literal against that
  expected type would otherwise emit a misleading layout-computation error
  before reporting the actual unresolved generic.
- Backend aggregate-instance collection and module type registration must walk
  `ArrayLenTy::Builtin.ty` alongside the array element. A nominal struct or
  union used only as the operand of `size[...]`/`align[...]` still owns layout
  and concrete instance products required by lowering.
- Program-signature type substitution must recursively substitute
  `ArrayLenTy::Builtin.ty`. Array-length metadata is a full type-bearing
  component; leaving its generic operand untouched produces stale generic
  identities in lowered trait and extension signatures.
- Body-check generic inference must use the same structural probe for array
  length builtin operands as for array elements. When `size[T]` is the only
  evidence for `T`, matching equivalent layout builtins must recurse into their
  operand before deciding that the call has an unresolved generic.
- Backend extension-target matching must stage substitutions while comparing
  array layout builtin operands. Matching `size[T]` structurally can bind a
  type parameter, but a later element mismatch must roll that binding back.
- Backend semantic type matching must recurse through equal
  `ArrayLenTy::Builtin` kinds before comparing array elements. Treating the
  complete length as an opaque value makes equivalent layout operands with
  distinct handles compare unequal; other length forms retain exact identity
  semantics until a canonical value is available.
- Body-check projection-obligation equivalence must apply the same recursive
  comparison to equal `ArrayLenTy::Builtin` kinds. Projection-cycle and
  obligation deduplication can otherwise split equivalent arrays solely because
  their layout operand handles were rebuilt independently.
- Body-check projection and trait-object structural equivalence must resolve
  evaluated `ArrayLenTy::ConstExpr` values through `array_len_const_expr_value`.
  Expression handles may match a `ConstValue` or another expression only when
  the owner publishes an evaluated fact; unresolved expressions remain
  identity-only so unrelated lengths are never collapsed.
- Extension-pattern generic presence and bound checks must recurse through
  nominal const-argument type metadata as well as ordinary type arguments.
  Const arguments carry a type identity that can contain a still-unbound type
  parameter even when their value itself is concrete.
- Method-pattern matching must treat `ArrayLenTy::Builtin` as structural when
  both sides use the same layout builtin. Match its operand through the same
  staged type/const substitutions as the array element; otherwise an extension
  target such as `[u8; size[T]()]` cannot infer `T` from a matching receiver.
- Method specificity ordering must use that same structural layout-operand
  recursion. A concrete `[u8; size[i32]()]` extension must subsume the generic
  `[u8; size[T]()]` candidate, or otherwise-valid calls become ambiguous.
- Trait-solver impl-pattern matching follows the same rule: when both array
  lengths use the same builtin, recursively match their type operands in the
  candidate substitution transaction. Treating the complete length as opaque
  loses type-generic impl candidates that are selected through layout metadata.
- Executable-reachability extension matching must preserve the two-store type
  identity boundary while recursively matching layout builtin operands. The
  operand is type evidence for generic recovery, but it must be compared using
  the `TypedTyRef` stores rather than raw interned handles.
- Backend vtable payload equivalence must compare `ArrayLenTy::Builtin` operand
  types structurally through the active `TypeEquivalence` owner. Equivalent
  nominal const representations can use distinct handles while describing the
  same vtable ABI payload.
- Backend aggregate and vtable owner deduplication must resolve evaluated
  `ArrayLenTy::ConstExpr` facts across all lowered modules. Distinct expression
  handles with the same evaluated length describe the same backend shape; raw
  expression identity is only a fallback when evaluation facts are unavailable.
- Backend-lower owner equivalence must apply the same facts to nominal const
  arguments nested in aggregate and vtable keys. An expression-valued const
  argument may match an integer spelling or foreign expression only when its
  array-length fact resolves; unrelated expressions remain distinct.
- Monomorphization projection-guard equivalence must apply the same recursive
  layout-operand comparison. Rebuilt projection keys can carry distinct but
  semantically equal nominal const types inside `size[...]` metadata.
- Monomorphization projection-cycle guards must resolve evaluated
  `ArrayLenTy::ConstExpr` values from every participating module. Distinct
  expression handles with the same evaluated length can describe the same
  recursive projection; raw expression identity alone can let equivalent
  cycles expand independently.
- Monomorphization projection-key equivalence must apply the same per-module
  const facts to nominal const arguments. Evaluated expression values may match
  integer spellings or foreign expressions; unresolved expressions must stay
  distinct to avoid collapsing unrelated instances.
- Trait-solver structural type equivalence must use its configured
  `const_expr_value` evaluator when comparing array lengths. This keeps
  projection and trait-goal matching consistent with const-argument
  equivalence while preserving identity-only behavior when evaluation is not
  available.
- Trait-solver layout-backed type lookup must apply the configured
  `const_expr_value` evaluator to array lengths and nominal const arguments
  when comparing against a `Layouts` product. Rebuilt expression handles from
  the layout interner are equivalent only when both values resolve; without
  evaluator facts, raw expression identity remains the conservative fallback.
- Trait-solver shared structural equivalence must apply the configured
  `const_expr_value` evaluator to nominal const arguments as well as array
  lengths. Projection and trait-goal guards must not split rebuilt nominal
  handles whose expressions resolve to the same value, while unresolved
  expressions remain identity-only.
- Program-signature equivalence may use only the `ConstExprSummary` facts
  supplied by the active lowering product. Use those summaries for literal
  array-length const arguments in nominal and trait identities; store-only
  comparisons without a lowering product must remain identity-only.
- Backend aggregate layout-product validation must match materialized instance
  declarations through the validator's structural type and const-argument
  equivalence. Raw vector equality can skip validation when signed and unsigned
  representations carry the same const bits, allowing a malformed product to
  pass without checking its declared fields.
- Backend static function-address validation must use structural type-argument
  matching when locating a materialized function instance. The fallback lookup
  must not skip ABI checks merely because equivalent nominal argument handles
  were rebuilt in a different order or compilation path; const arguments still
  use the validator's canonical const comparison.
- Backend semantic type matching must treat `TraitObjectPointee` as a complete
  structural type, not an opaque leaf. Compare the trait identity, type and
  const arguments, and associated-type bindings through the same recursive
  equivalence used for the public trait-object pointer; rebuilt pointee handles
  must not split extension, instance, or ABI matching.
- Trait-solver structural equivalence must recurse through the operand type of
  equal `ArrayLenTy::Builtin` kinds in both its shared type-equivalence adapter
  and projection-aware array comparison. Selection-only pattern matching is
  insufficient because projection cycle guards consume the general relation.
- Body-check projection-obligation equivalence must cover both trait-object
  views (`TraitObject` and `TraitObjectPointee`) as complete structural values.
  Their readonly mode, trait identity, generic arguments, and unordered
  associated-type bindings all participate in the cycle guard; falling through
  to the default mismatch path can duplicate obligations or miss a valid
  inherited projection.
- Layout-root collection must enqueue the type of every const argument, not only
  ordinary type arguments. This applies to nominal values, trait objects,
  associated bindings, projections, and standalone generic instantiation facts;
  const metadata can reference aggregates with independent layout ownership.
- Object-safety DFS must substitute both type and const arguments for every
  inherited source trait instance. Key its active cycle guard by the complete
  `(trait, args, const_args)` identity and keep a separate expanded set so
  diamond siblings do not repeat diagnostics while a recursive path still
  terminates correctly.
- When object-safety normalizes nested types, preserve nominal const arguments
  and recursively normalize each argument's type. Dropping those fields makes
  erased signatures disagree with trait-object identity and backend vtable
  instantiation even when the source declaration is otherwise valid.
- Where-bound candidate substitution has the same identity rule: nominal
  arguments must retain both type and const vectors, with const argument types
  recursively substituted. A type-only reconstruction can accept or reject a
  bound against the wrong const instance.
- Backend static trait-method fallback must carry trait const arguments into
  default-method self selection, concrete-implementation diagnostics, and the
  solver `TraitGoal`, then retain them in the fallback `FunctionCallee` payload.
  Clearing them at either boundary can select or materialize the wrong
  implementation context even when dynamic vtable dispatch is correct.
- Backend recursive type filters must inspect `ConstGenericArg::ty` anywhere a
  type can carry const metadata. This includes function-instance admission and
  the generic, projection, error, and depth walkers; aggregate struct/union
  instance collectors and top-level backend type registration must likewise
  enqueue those types from nominal, trait-object, associated-binding, and
  projection positions.
- Backend recursive type filters must also inspect the operand type of
  `ArrayLenTy::Builtin`. Arrays can carry generic parameters, unresolved
  projections, or error types only through `size[T]`/`align[T]` metadata; depth,
  generic-parameter, unresolved-projection, and error filters must visit that
  operand with the same recursion budget as the array element.
- Target-relative primitive widths must not be encoded as host constants in
  builtin validation. In particular, atomic `isize`/`usize` values use the
  configured target pointer width before the native-width admission check;
  focused tests must include a non-host pointer width.
- Allocator `resize`/`remap` implementations may turn a non-empty allocation
  into an empty block only if the resize itself retires all ownership state.
  Allocators with separately fallible backing ownership must return `false` so
  default `realloc` creates an empty block and frees the old owner; otherwise
  empty-block `free` semantics strand metadata and report a false leak.
- Default `Allocator::realloc` must return a typed rollback error when the old
  block release fails and cleanup of the replacement also fails. That error must
  retain the replacement `Block` and both error identities; never reduce a
  two-owner failure to a bare allocation error or silently drop the replacement.
- A close-on-exec spawn error pipe is a protocol, not an ordinary best-effort
  write. Retry interrupted fixed-record reads/writes, distinguish EOF only
  after zero record bytes, and reap every failed child with an EINTR-safe wait.
  The parent must close the consumed handshake read descriptor exactly once
  after producing the primary handshake result: a successful EOF exposes a
  close failure as setup error, while a protocol/read failure remains primary.
  Public child cleanup must attempt all owned pipe closes before returning the
  first error, especially after exited status has been cached.
- LLVM declaration readiness must retain const-expression ownership as well as
  const-argument type ownership. A `ConstGenericValue::ConstExpr` embedded in a
  nominal, trait-object, associated binding, or projection contributes its
  expression module to the pending dependency closure before codegen starts.
- LLVM DIBuilder subroutine metadata has a distinct return slot at index zero:
  it must be `null` for `void`, followed by the parameter type metadata. Keep
  return and parameter types separate in the typed API, and check the combined
  count before converting it to LLVM's `u32` field.
- LLVM DIBuilder array subranges represent element counts, so reject negative
  lengths before calling the signed C API. A typed debug wrapper must not let a
  negative source count become a malformed DWARF range.
- LLVM instruction inspection APIs with opcode-specific preconditions must
  check the opcode in the typed wrapper before entering the C API. In
  particular, allocated-type queries accept only `alloca`; an arbitrary
  `InstructionValue` is not proof that LLVM can inspect it as an allocation.
- Every non-owning LLVM handle allocated in a context arena must carry that
  context lifetime in its Rust type. This includes attributes: a non-null raw
  handle does not make it valid after the originating `Context` is dropped.
- LLVM sub-owners must also borrow their immediate owner when disposal or
  finalization touches it. A `DebugInfoBuilder` belongs to one `Module`, not
  merely its context, and must be dropped before that module is disposed.
- Backend IR validation recursively validates const-argument types in every
  type constructor, so malformed or session-mismatched metadata cannot bypass
  the validator merely by appearing in a const generic argument.
- Executable reachability's type-only owner projection must retain both
  `ConstGenericValue::ConstExpr` modules and `ArrayLenTy::ConstExpr` modules.
  They can own layout/signature facts without owning a runtime body, so they
  belong in `type_modules` even when no ordinary type argument points there.
- Semantic-input const-expression pruning must use a complete recursive type
  walk. Trait-object and pointee const args, associated-binding type/const args,
  and projection const args are roots just like nominal const args; omitting
  them discards AST/value-resolution input that later semantic facts retain.
- Extension-trait signature type-module discovery must retain the same owner
  closure. Collect source trait/nominal owners, including associated-binding
  trait identities, `ConstGenericValue::ConstExpr` modules from every const
  metadata position, and `ArrayLenTy::ConstExpr` modules before provider
  expansion; a type-only owner can be needed even when it contributes no
  runtime provider body.
- Aggregate whole-program products require evidence that a real consumer needs
  the aggregate. Prefer item-, body-, module-, or codegen-unit-owned products
  when they preserve the dependency boundary.

Ownership is judged by lifetime and mutation authority, not by whether a type is
wrapped in `Arc` or stored in a different crate.

## 4. Query And Incremental Correctness

The typed query/fact graph is the only dependency and invalidation truth source.

- Mutable input cannot bypass dependency recording.
- Driver orchestration must not reproduce a semantic fixed point already owned
  by compiler queries.
- A source update retires obsolete current-revision entries after quiescence.
  The current cache, dependency graph, slot tables, and locators must not retain
  revision history as an accidental compatibility feature.
- Incremental results must remain equivalent to clean recomputation. Randomized
  edit sequences and clean/incremental differential tests are preferred for
  cross-query invalidation contracts.
- Representative build acceptance must make its process and module boundaries
  machine-checkable. Each incremental and independent-clean state runs in a
  fresh compiler process, records that process identity, and compares at least
  one executable whose source graph contains more than one module.
- Concurrent slot, cycle, invalidation, and red-green state machines require
  deterministic tests; model or race-focused tests are required where ordinary
  examples cannot exercise the transition safely.
- Query wait-for edges are temporary state, not dependency history. Cycle
  detection must release every edge/frame on both normal wait completion and
  cycle failure, and retirement admission must be reopened by RAII after a
  callback panic. Build schedulers likewise keep cancellation tied to canonical
  action position, wait for the active wave, and never dispatch dependents after
  a failure.
- Cache keys include schema, compiler, target, options, stable input identity,
  and domain separation appropriate to the product. Entries repeat and validate
  their identity, reject truncation and trailing data, and retire corruption
  rather than attempting a compatibility decode.
- Persistent cache retirement and publication for one content-addressed path
  use the same mutation lock. A reader may remove a corrupt record only when the
  bounded bytes or oversized state it observed still occupy that path; it must
  preserve a valid replacement published before retirement acquires the lock.
  Immutable publishers revalidate an existing winner under that lock rather
  than silently overwriting it.
- Verification mode recomputes the product and replaces a well-formed but
  semantically stale artifact. A cache hit is not proof of correctness.

Do not persist a query merely because its value is serializable. A persistent
product is worthwhile only when it cuts a measured dependency chain without
smuggling revision-owned state across sessions.

## 5. Concurrency And Resources

Parallelism follows ownership and resource accounting.

- Query providers do not create unmanaged operating-system threads. They submit
  work to the session-owned persistent executor.
- All compiler batches share the process CPU budget, including Cargo or GNU Make
  jobserver capacity when inherited.
- LLVM work additionally obeys process-wide memory backpressure. Worker count is
  not a substitute for a memory budget.
- Parallel tasks own their mutable result and borrow an immutable `Send + Sync`
  context. Results merge in a deterministic order independent of completion
  order.
- Tests use the same public compiler and LLVM contracts as production. The
  integration harness may reserve an explicit compiler or build resource
  session, but unit tests must not change production API semantics.
- Test resources derive from effective CPU, memory, and cgroup limits. Hidden
  machine categories and undocumented limit variables are forbidden.

Wrapping an existing aggregate loop in a parallel iterator does not establish a
task model. Partition identity, readiness, ownership, cancellation, memory
limits, and deterministic merge behavior must be explicit first.

## 6. Evidence And Acceptance

Every non-trivial compiler project defines acceptance before broad migration.
Acceptance must describe observable architecture and behavior, not only that new
types or APIs exist.

- Completion requires the old model to be absent, relevant structural searches
  to be clean, focused tests to pass, and the affected end-to-end path to run.
- Validate from narrow to broad: owner tests, consumer tests, workspace check,
  strict all-target/all-feature Clippy, formatting, then relevant integration or
  performance gates.
- External infrastructure acceptance requires external evidence. A local config
  check cannot substitute for an actual hosted run, artifact download, cache
  reuse, linker execution, or cross-run comparison.
- Performance conclusions use the complete workload path, repeated samples, and
  compatible resource identity. Deterministic query, codegen-unit, cache, and
  allocation counters should be interpreted before noisy wall time.
- Never report progress from an isolated cache hit if another consumer
  immediately rebuilds the same raw dependency chain. Measure the end-to-end
  execution cut.
- Failed experiments are valuable evidence, but rejected schemas, readers,
  counters, adapters, and fallback paths are removed completely. Preserve the
  lesson in documentation or commit history, not dormant production code.

Changes should be grouped into meaningful dependency-complete batches rather
than one-symbol commits. Several coherent implementation waves may remain
together while that batch is still advancing; do not create commits merely to
snapshot partial movement. Once the delivery batch passes its relevant gates,
commit it with a descriptive `feat: ...` subject before reporting or handing off
the work, and do not carry it into an unrelated batch. Do not mix unrelated
cleanup into the commit merely because a broad validation command exposed it.
Temporary execution progress belongs in its bounded project roadmap; durable
ownership, validation, and maintenance lessons must be moved into the relevant
stable document as part of the batch that establishes them.

## 7. Boundary And Test Reviews

Source line count, file count, crate count, and test count are investigation
signals, not architecture goals.

- Split a file when it exposes stable algorithm or data ownership with a narrow
  collaboration surface. Do not move code only to reduce a line count.
- Merge a crate only after reviewing production consumers, dependency direction,
  stable public types, and cycle risk. A small shared leaf crate can be the
  correct boundary.
- Data-driven suites should express repeated compiler matrices, resource class,
  inputs, edits, expected diagnostics, and outputs. They should not hide complex
  runtime or standard-library behavior inside opaque metadata.
- Keep hand-written tests for dynamic repository/toolchain contracts and for
  process, filesystem, I/O, allocator, container, startup, runtime, and standard
  library semantics when those behaviors are the subject of the test.
- Lower bounded filesystem syscall paths into caller-stack storage. Do not add
  allocator-backed path convenience APIs whose internal fallible release can
  replace a primary filesystem error or discard an opened handle; paths beyond
  the platform bound must fail before the syscall with their path-domain cause.
- Treat public reader/writer byte counts as untrusted implementation output:
  validate `n <= requested.len()` before changing any cursor, buffered length, or
  limit, and retain pending bytes when reporting an invalid transfer. Generic
  adapters must forward the wrapped implementation's `invalidRead` or
  `invalidWrite` classification rather than replacing it with ordinary EOF or
  short-write identity.
- For multi-allocation collection cleanup, detach each owner only after its
  allocator release succeeds. Preserve failed slots for a cleanup retry and do
  not advertise partially deinitialized state as an empty reusable collection.
- Keep hash-table control bytes, keys, and values in one aligned storage block.
  During rehash, publish the new table only after transferring the old block to
  a map-owned retired state; a failed retired free remains attached, blocks
  later rehash until cleanup succeeds, and is retried by `deinit`.
- Apply the same transaction rule to single allocation owners. `Allocated` and
  `CallableAllocation` must retain the allocator-returned Block layout plus the
  complete release pointer and length, and clear their owner fields only after
  `free` returns success; a release error must leave the original block
  retryable. Never rebuild a release block from only the typed value pointer and
  `Layout::of[T]`: a zero-sized value may still have a non-empty Block. A
  successful deinit or owner transfer must clear the release metadata as well
  as the logical size, so repeated cleanup cannot double-free the old block.
- Allocator rollback paths must retain child blocks when a validation failure is
  followed by a failed release. Pending rollback owners belong to allocator
  state and are retried before later allocation or during `deinit`; they must
  contribute to `capacity`/emptiness so no failed cleanup becomes invisible.
- Do not implement generic error or process-exit conversions for errors that
  carry live owners. In particular, callers must match `ReallocError::Rollback`
  and release both the original Block and its replacement; converting the enum
  by value to an exit code would silently discard the replacement owner.
- Keep allocator page slot growth and live-count updates checked. Reject a
  doubling overflow or an impossible used-slot transition before mutating the
  page metadata.
- Keep public allocation-owner constructors on the `std::mem` facade. Do not
  publish a type while leaving its only construction extension in a
  package-private implementation module.
- For collections whose elements own allocations, retain the collection
  backing whenever any element cleanup fails. Releasing the element storage
  would discard the only metadata capable of retrying the residual owner.
- Fallible build-plan validation scratch is state, not defer-local cleanup.
  Keep indegree and ready-list owners on `Build`, attempt both releases after
  every pass, preserve a cycle or validation error over cleanup failures, and
  retry any failed release before the next pass or final deinitialization.
- Public allocator-backed construction must retain partial owners when rollback
  can fail. `Build::init` therefore returns a retryable `BuildInitAttempt`;
  direct `Error!Build` construction and fallible local rollback defers are not
  valid because neither can carry unreleased paths or target strings.
- Graph insertion must reserve the containing collection before constructing
  fallible owned fields. For package records, keep the partial record in the
  `Build` pending slot and retry its cleanup before the next insertion or from
  `deinit`; never return through a local rollback defer that loses the record.
- Apply the pending-record protocol independently to each artifact target kind.
  Object and static-archive name/output owners remain on `Build` until an
  infallible post-reserve append transfers them; failed cleanup blocks the next
  matching insertion until retry succeeds.
- A pending executable includes its static-archive handle-list backing, not just
  its strings. Cleanup must attempt the list, output name, and name even when an
  earlier release fails, retaining all failed owners for the next insertion or
  final graph cleanup.
- Nested graph records must be attached before their first allocation. Module
  imports are appended as empty owners to a reserved imports list, then
  initialized in place; this lets module cleanup retain the nested list whenever
  any import field release fails. Do not recreate a detached clone helper.
- Pending union records must establish the active kind before retaining payload
  owners. Generated-file and uncacheable steps use distinct empty constructors,
  reserve the steps list first, and initialize name/payload in place so cleanup
  always reads the active union member and can retry every failed owner.
- A committed record that must be rolled back cannot return to a local owner.
  `rollbackLastStep` moves it into `Build.pendingStep` before cleanup. Run/test
  dependency errors stay primary over rollback failures, while argument list,
  argument strings, and step name remain reachable for the next operation.
- Use the same pending-step and explicit dependency completion path for install
  actions. Executable and static-archive destination strings must not have a
  separate fallible defer or a cleanup error that replaces producer-edge
  failure.
- External-command argument and environment records must be appended in their
  active union/struct form before nested strings are retained. On any producer
  edge failure, remove the complete newly added dependency suffix and transfer
  the popped command step to pending ownership before recursive cleanup.
- Keep every build step insertion on the pending-step boundary, including
  non-owning aggregate/check/emit payloads, so later payload evolution cannot
  reintroduce detached name ownership. Multi-edge commits use explicit suffix
  rollback; `lib/std/build` must remain free of fallible cleanup defers.
- Treat plan-encoding bytes as a `Build`-owned publication buffer. Do not hide
  its release in a fallible defer; retain it through encoding and file
  publication, preserve the first writer/flush/sync/close error, and retry a
  failed backing release on the next draft or final deinitialization.
- Replacement-based collections must retain a temporary new backing if the
  old-owner release fails and cleanup of that new backing also fails. Keep both
  owners reachable for `deinit`; never assume a best-effort rollback freed the
  replacement.
- `ArrayList` keeps allocation ownership in `storageBlock`, `replacementBlock`,
  and `aliasBlock` only. The `items` view and logical capacity are borrowed
  metadata, not release state; a present `storageBlock` must be released even
  when the logical element size or capacity is zero, and growth must use it as
  the old replacement owner. Only an ownerless zero-capacity list may use a
  canonical empty Block. Do not add a fallback that rebuilds a `Block` from a
  raw slice pointer and length.
- Self-aliasing collection operations need a distinct temporary-copy owner;
  do not reuse the replacement slot when a grow/shrink failure can already
  retain another allocation.
- For consuming close APIs, clear the cleanup flag before the syscall. A failed
  close may still consume the descriptor, so a defer retry can only mask the
  original error with `BadFd`.
- Adapters derived from an owning file or directory must borrow the owner and
  resolve its live optional handle before each underlying descriptor access. Do
  not copy its raw descriptor into adapter state: after owner close and
  descriptor reuse, that snapshot can target an unrelated object. Document the
  caller's responsibility to keep borrowed owner storage alive and stable
  because Nia has no borrow checker. Keep exhausted iterators fused, and commit
  directory refill cursors only after the underlying read succeeds so a
  transient failure cannot replay an exhausted buffer.
- For an operation that owns temporary descriptors, perform the main operation
  first and then close every descriptor explicitly. Preserve the main operation
  error when it failed; otherwise return the first close error. Chained cleanup
  must still attempt later descriptors after an earlier close failure.
- Formatting helpers that stage temporary output must apply the same precedence:
  preserve the primary format/encoding error when staging cleanup also fails,
  and report the cleanup allocation error only when staging itself succeeded. If
  `free` fails before releasing the staging backing, transfer that owner to the
  destination and retry it before the next formatting operation and during
  destination cleanup; never leave the writer and destination as duplicate
  owners. A string-to-slice ownership transfer must retire that pending staging
  owner before transferring the text backing. If that pending release fails,
  leave both owners attached to the source for retry.
- Validate formatter radices at the public entry point before digit counting or
  writing signs, prefixes, and padding. The supported set is exactly 2, 8, 10,
  and 16; radix zero must not divide by zero, radix one must not loop forever,
  and every unsupported radix must leave the writer unchanged. Public spec
  constructors must have all parameter/result enums exported from the facade.
- Treat every open-enum field in a public formatting spec as untrusted input.
  Reject unnamed presentation and alignment discriminants before any writer
  mutation; wildcard matching must not turn an unknown alignment into a
  successful default behavior.
- Audit hash-table capacity APIs against logical `len`, tombstones, and physical
  empty-slot growth independently. Deletion must make assume-capacity insertion
  legal without allowing an exhausted growth counter to underflow.
- Treat fallible iterator address calculation as a transaction. Compute and
  validate the candidate element address before committing front/back index
  changes; failure must not silently consume an item.
- Check large-allocation base plus header offsets before alignment and metadata
  publication. A malformed child-allocator address must fail closed before any
  wrapped header pointer is written.
- Keep hash-table probe cursors closed over their allocated table shape. Validate
  the power-of-two/group-width precondition before deriving a mask, and perform
  probe-step modulo arithmetic without an overflowing host-width intermediate.
- Treat freestanding startup metadata as an untrusted raw ABI boundary. Check
  word-count, byte-offset, and stack-base arithmetic before turning the initial
  stack into `argv`/`envp` pointers; fail closed before publishing malformed
  process views.
- Keep process command staging behind an explicit attempt owner. Path, argument,
  argv, environment, envp, and cwd lowering must retain every allocation plus
  the pending child or primary spawn error until all fallible releases succeed.
  Retry every failed owner, attempt all later releases after an earlier failure,
  and do not reintroduce a `run` shortcut that discards this state.
- Treat successful process creation as an owner-carrying boundary. Linux spawn
  setup should retain every pipe pair in one resource transaction, transfer the
  child pid and public pipe ends before consuming the remainder, and close every
  untransferred end on every return path. Descriptor close errors must not
  replace a primary spawn/handshake error; after exec succeeds, do not return a
  plain cleanup error that cannot carry the still-live child owner.
- Keep failed-child reaping inside the spawn attempt. Retry `EINTR`; if another
  `wait4` error occurs, report the pid, primary spawn stage/cause, and reap cause
  without detaching any of them from the attempt. A later `finish` must retry
  that same pid, and the original spawn error becomes observable only after the
  owner is reaped. Treat `ECHILD` as terminal because no waitable owner remains.
- Materialize startup process views once. Long-lived `Init` values should store
  validated `Args`/`Env` records, not raw `argv`/`envp` pointers that force later
  rescans or recreate the ABI boundary on every accessor.
- At the startup pointer boundary, handle null vectors as empty and reject null
  element pointers before passing them to NUL-terminated string adapters.
- For repeated path validators, keep the terminal component outside the scan
  loop. A `<= len` loop that increments after its end sentinel can wrap a
  maximum-width index even when every component is valid.
- Before growing an owned path or string, identify an input slice borrowed from
  that same logical text by offset. Reconstruct it from the replacement storage
  after reserve succeeds; never carry the old pointer across allocator growth.
  Reserve the complete mutation first, then use assume-capacity operations so
  allocation failure cannot commit only a separator or prefix. Prefer offset
  rebasing over a temporary allocation when the source is within the same text.
- Treat caller-provided path encoding storage as transactional output. Validate
  every scalar, the checked total byte length, and trailing-NUL capacity before
  writing the first byte. A failed replacement encode must not partially
  corrupt another borrowed native view backed by the same caller buffer.
- UTF-8 iterators must validate decoded-width advancement against the borrowed
  slice before committing state. Treat a failed advancement or remaining-count
  decrement as a terminal invalidation, rather than exposing wrapped indices.
- Parse variable-length syscall records once and share the validated bounds
  between payload construction and cursor advancement. Required terminators
  belong to that boundary; record padding is not payload.
- Validate kernel timestamp subsecond fields before publishing domain metadata.
  A raw nanosecond value must be within one second; do not defer this invariant
  to formatting or later consumers.
- Validate pointer-returning syscall payloads before converting them to slices.
  For Linux `getcwd`, require a positive in-buffer count and the promised NUL
  terminator; malformed success data is an I/O failure, not a partially trusted
  path.
- For wait-like syscalls, validate the returned identity before publishing
  status. `wait4` must return the exact requested child pid for both blocking
  and non-blocking paths; any other successful identity is malformed `Io`.
- Treat raw syscall error returns as untrusted signed ABI data. Require a
  negative value within Linux's errno range before negating and narrowing it;
  reject sign, minimum-integer, and width-overflow cases as `Io`.
- Treat successful descriptor and process-id returns as untrusted too. Check
  non-negative/positive sign and the target `i32` width before constructing
  typed handles; validate kernel-filled pipe descriptors through the same
  boundary.
- For syscalls whose ABI promises zero on success, require exactly zero before
  publishing success. Identity-returning calls such as `dup2` must also match
  the requested destination rather than merely being non-negative. Validate
  output buffers such as `pipe2` descriptors only after that zero result check.
- For pointer-returning allocation syscalls, reject null success addresses
  before constructing references unless the call explicitly requested a null
  mapping. Keep the platform flags and pointer invariant documented together.
- Over-aligned Linux page mappings may expose an aligned interior pointer, but
  the complete mmap range remains the release owner. Keep that release range in
  the allocator block and unmap it exactly once; do not trim prefix/suffix
  pages through best-effort calls that can strand a residual mapping. If
  validation fails after `mmap` succeeds, unmap the complete range before
  returning the error; an unpublished mapping is still an owned resource.
- Typed slice owners must retain the allocator `Block`, not only a borrowed
  slice. Transfer APIs consume `SliceAllocation[T]`; cleanup failure leaves the
  owner and its release range attached for retry. Do not add raw-slice ownership
  constructors or reconstruct a release block from pointer plus logical length.
  `deinit` releases the Block even when the logical length is zero, because a
  custom allocator may attach a non-empty release range to an empty view.
- Treat successful allocator resize/remap as a logical-layout transition on the
  existing `Block`. Use the canonical layout-preserving helper so the release
  pointer and length survive default remap, concrete wrappers, and resized arena
  backing chunks; never replace the owner with `Block::init(block.ptr(), ...)`.
- Arena cleanup must detach and walk both used and free chunk lists, attempt all
  child releases, and link every failed chunk back into owned state before
  returning the first error. Retained capacity must expose those residual
  owners, and retry must not revisit chunks whose releases already succeeded.
- General-purpose allocator destruction must attempt small pages, large headers,
  and pending rollback blocks independently. Restore each failed owner to its
  matching list or slot, preserve the first release error, and keep capacity,
  used bytes, and emptiness derived from that residual state for the next retry.
- Treat build-plan counts as untrusted protocol fields: check aggregate and
  derived additions before narrowing to wire integers, and reject oversized
  payload lengths before writing their prefixes. Enforce the total byte budget
  before each encoder buffer growth, not only after serialization completes.
- Do not preallocate typed decoder collections directly from an untrusted count.
  Grow after each item consumes bounded input, so truncated prefixes cannot
  amplify into `count * size_of(T)` host memory.
- Tests and documentation describe the current contract. Historical behavior
  belongs in Git, not compatibility fixtures or stale debug switches.
- Projection-aware trait type equivalence must recurse through every semantic
  type constructor, including tuples and closure states. A projection nested
  inside a container must use the same cycle-aware relation as a top-level
  projection; adding a new `TyKind` variant requires updating this relation and
  its owner regression matrix.
- Layout lookup has the same cross-store identity constraint: layout keys may
  contain rebuilt tuples or closure states whose nested handles come from a
  different interner. The layout equivalence walker must recurse through those
  constructors and compare const-argument types semantically before deciding a
  product is absent.
- Body-check projection-obligation equivalence follows the same recursive
  contract as matching and layout lookup. Keep tuples, closure states, and
  identity-only variants in the relation so associated-type checks do not
  reject a valid composite merely because normalization rebuilt its handles.
- Backend instantiation matching must remain closed over the full `TyKind`
  shape as well. Tuple, closure-state, volatile-pointer, and slice-pointee
  values can appear after generic substitution; compare their payloads
  recursively instead of letting a rebuilt composite fall through to a false
  leaf result.
- ProgramIndex instance maps may use interned argument handles as an exact
  fast path, but lookups for structs, unions, functions, and globals must
  retain a definition-grouped semantic fallback. Compare rebuilt type payloads
  and integer const arguments through `TypeEquivalence`, preserve a function's
  optional receiver and argument-module identity, and keep a global's
  argument-module identity in the match; otherwise cross-module codegen can
  silently miss an already materialized owner.
- ProgramIndex vtable ownership queries must likewise retain a semantic fallback
  for rebuilt receiver and object-type handles. The value and object-type
  iteration APIs must use the same complete-key relation. Matching only one
  erased type or trusting raw key equality can select the wrong owner or report
  a missing dispatch table.
- ProgramIndex array-length equivalence must resolve evaluated `ConstExpr`
  handles through their owning backend modules. A const expression from a
  rebuilt or foreign module can be equivalent to a `ConstValue`, or to another
  expression with the same evaluated length; raw expression-id equality is only
  an exact fast path when no evaluation facts are available.
- ProgramIndex nominal const-argument equivalence must apply the same backend
  array-length facts. Expression-valued const arguments may match an integer
  spelling or a foreign expression only when the corresponding facts resolve;
  unrelated const expressions remain identity-only.
- LLVM module-codegen declaration, instance, and vtable-entry matching must
  apply published array-length facts to const arguments as well as type
  payloads. Keep the generic helper identity-only without an evaluator, while
  `ModuleCodegen` supplies the owning program/source facts for expression and
  integer spellings.
- Executable reachability's paired `TypedTyRef` relation is explicitly
  cross-store. Every composite constructor used in a reachable impl target
  must recurse through the left and right stores; a raw handle shortcut is
  valid only before structural comparison, never for nested tuple or closure
  payloads.

## 8. Build And Standard Library Work

Compiler architecture is an input to build-system and standard-library work,
but it is not a substitute for their design.

Before a substantial build or standard-library project begins, create a
separate bounded design and acceptance document covering its own ownership,
error model, cache and reproducibility rules, runtime/ABI interaction, public
surface, migration boundary, tests, and performance evidence. Do not append
build or standard-library feature work to an already closed compiler roadmap or
reuse compiler completion percentages to describe it.

Durable build ownership and API decisions live beside their owners in
[`crates/nia-build/README.md`](../crates/nia-build/README.md), the relevant Rust
module, and [`lib/std/build.nia`](../lib/std/build.nia). Other standard-library
decisions likewise live in [`lib/README.md`](../lib/README.md) and the relevant
`lib/std` facade or module. Current std APIs are not retained merely because
they made the bootstrap run, and full build sessions remain resource-accounted
integration work rather than ordinary unit tests.

`nia-test-support` is the shared owner of that resource accounting. Explicit
compiler/build sessions are thread-affine and cover nested compiler commands;
runtime commands remain independently charged, while their memory reservation
may reuse the enclosing compiler working set. Temporary directories are guard-
owned, and fixture manifests are consuming schemas: tests must read every
expected key and call `finish` so stale or misspelled fields cannot be ignored.
Fixture-relative paths reject absolute and parent components, and fixture copies
exclude generated `.nia-build` and `.nia-cache` state.

Any compiler changes required by that work still follow this maintenance
contract. Ordinary build or standard-library errors must continue through the
project's explicit diagnostic and result systems rather than introducing panic
paths or private control channels.

## 9. Roadmap Retirement

A roadmap can be deleted when all of the following are true:

1. Every stated acceptance item is closed with code, test, or external evidence.
2. Implemented architecture is documented in its stable reference document.
3. Durable maintenance rules and lessons have been migrated out of progress
   notes.
4. Follow-on projects with materially different scope are explicitly separated.
5. Git history retains the detailed sequence, rejected experiments, and
   intermediate measurements.

Deleting a completed roadmap prevents historical percentages and temporary
sequencing from becoming false current policy. It does not erase the work or
its evidence.
