# Closure Roadmap

Status: active bounded language project

This roadmap defines Nia's closure project. It is intentionally separate from
the completed tuple/unit work and from the reserved-language list in the
language specification. Nia has no ownership or borrow checker, so closure
state and pointer provenance must remain explicit at every stage.

## Design Contract

The canonical source form is:

```nia
[captures](parameters) ReturnType {
    body
}
```

Capture entries are named and explicit. A bare `name` captures the value of
that name; `name = expression` captures the result of an ordinary expression.
There is no implicit lexical capture and no second `fn[]` spelling.

The type taxonomy is deliberately split:

- A closure expression creates a unique anonymous, `Sized` concrete state
  value.
- `&fn(Args...) Return` remains the existing thin function pointer and carries
  no environment. Only a no-capture closure may coerce to it.
- `Fn(Args...) Return` is an unsized callable interface. `&Fn(...)` and
  `&mut Fn(...)` are non-owning fat-pointer views containing state and entry
  metadata.
- Dynamic views never own or free state. An allocator-backed owner is a later,
  explicit API, not an implicit escape conversion.

## Delivery Waves

### 1. Syntax and identity

- [x] Parse explicit capture closures as a dedicated AST node.
- [x] Preserve capture names, capture expressions, parameter types, return
  type, body, and stable node identity.
- [x] Reject variadic closure parameters.
- [x] Keep `[]` parsing transactional so failed closure recognition falls back
  cleanly to arrays and type-target expressions.

### 2. Concrete closure values

- [x] Introduce an anonymous closure-state semantic type and layout identity.
- [x] Resolve captures in the enclosing scope and parameters/body in a fresh
  closure scope.
- [x] Type-check closure bodies against their declared signature.
- [x] Lower direct calls to generated entry functions with an explicit state
  pointer.

### 3. Callable views and coercions

- [x] Add the unsized `Fn(Args...) Return` callable interface type.
- [x] Add explicit `&Fn`/`&mut Fn` fat-pointer construction and dynamic calls.
- [x] Permit no-capture decay to the existing `&fn` pointer only.
- [x] Reject capturing-to-thin-pointer coercions with a dedicated diagnostic.

### 4. Escape and allocation safety

- [x] Track closure-state provenance through aggregates, returns, stores, and
  call summaries.
- [x] Reject dynamic views that outlive stack-backed closure state.
- [x] Reject captures of local addresses when the resulting state can escape.
- [ ] Add an explicit allocator-backed owner with explicit destruction before
  enabling escaping dynamic closures.

### 5. Products and documentation

- [x] Generate stable symbols and backend ABI records for closure entries.
- [x] Materialize direct entries and non-owning callable views in LLVM, with
  codegen, runtime, diagnostics, and clean/incremental identity coverage.
- [x] Materialize no-capture closure decay to `&fn` through a zero-state
  adapter/thunk.
- [x] Document the current non-owning ABI and stack-backed memory model in the
  language and ABI specifications.
- [ ] Extend the ABI and memory documentation when allocator-backed ownership
  and explicit destruction are designed and implemented.
- [ ] Retire this roadmap only after every acceptance item has evidence and the
  durable rules have moved to stable documentation.

Wave 2 now lowers each concrete closure value to an ordered state aggregate and
keeps generated entry bodies under the owning source-body query. Direct calls
carry a dedicated closure-entry identity plus an explicit readonly state
pointer, while entry bodies project captured values through that pointer rather
than referring to parent-function locals. Backend input assembly publishes
stable symbols and ABI records without treating generated entries as source
`GlobalDefId` functions or thin `&fn` pointers.

Wave 3 callable construction is expected-type guided but remains explicit in
source: only `&closure` and `&mut closure` create views. Signatures match
structurally, writable state may be viewed readonly, and a readonly state
pointer cannot become writable. Body IR and Function IR use dedicated
construction and callee variants, and the generated entry remains owned by the
same `LoweredFunctionBody` product. LLVM materializes the entry and its callable
view directly from its stable backend ABI record. No-capture closures
additionally support expected-signature-guided readonly `&closure` conversion
to the existing thin `&fn` pointer. Body IR and Function IR preserve that
operation with dedicated closure-function-pointer nodes carrying `ClosureId`;
capturing closures are rejected with a diagnostic that directs the programmer
to `&Fn(...)`. LLVM materializes the compatible no-capture form as a thin
adapter that supplies a private zero-state token to the ordinary closure entry.

Wave 4 now runs as an independent `nia-closure-check` semantic stage after Body
IR construction. It computes a cross-function fixed point of parameter return
and escape summaries, then tracks callable-view provenance through locals,
patterns, aggregates, projections, stores, calls, branches, loops, and nested
closure bodies. Direct calls use the summaries; function pointers, dynamic
dispatch, and unknown calls conservatively treat arguments as
retaining-capable. Every stack-backed `&Fn`/`&mut Fn` view is rejected when it
is returned, stored through memory, passed to a potentially retaining call, or
carried across the lexical scope that created its closure state. The stage is
deliberately a bounded provenance analysis, not a general ownership or borrow
checker. Captured local addresses are tracked as a separate state provenance
category; an allocator-backed owner remains separate Wave 4 work.

Wave 5 identity work now publishes closure entries as backend products with a
stable `ClosureId` plus source-or-instance owner key. Entry symbols are derived
from the concrete owner symbol and closure ordinal, so generic instances cannot
collide with their source template or with one another. The ABI record makes the
readonly state pointer explicit as the hidden first parameter, followed by the
ordered user parameters and return type. Closure entry bodies participate in
backend reachability, dead-code analysis, aggregate roots, codegen partition
membership, and incremental fingerprints. The LLVM backend validator checks that
ABI record against the generated entry body before codegen starts. LLVM
declaration/body materialization emits every entry in the same partition as its
source function or concrete generic instance. Direct calls use that entry
directly, and callable views store its address for indirect calls. No-capture
thin-pointer decay uses a generated adapter because an entry's hidden state
parameter is not part of the `&fn` ABI.
