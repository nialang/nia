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
- [ ] Permit no-capture decay to the existing `&fn` pointer only.
- [ ] Reject capturing-to-thin-pointer coercions with a dedicated diagnostic.

### 4. Escape and allocation safety

- [ ] Track closure-state provenance through aggregates, returns, stores, and
  call summaries.
- [ ] Reject dynamic views that outlive stack-backed closure state.
- [ ] Reject captures of local addresses when the resulting state can escape.
- [ ] Add an explicit allocator-backed owner with explicit destruction before
  enabling escaping dynamic closures.

### 5. Products and documentation

- [ ] Generate stable symbols and backend ABI records for closure entries.
- [ ] Add codegen, runtime, diagnostics, and clean/incremental identity tests.
- [ ] Document the final ABI and memory model in the language and ABI specs.
- [ ] Retire this roadmap only after every acceptance item has evidence and the
  durable rules have moved to stable documentation.

Wave 2 now lowers each concrete closure value to an ordered state aggregate and
keeps generated entry bodies under the owning source-body query. Direct calls
carry a dedicated closure-entry identity plus an explicit readonly state
pointer, while entry bodies project captured values through that pointer rather
than referring to parent-function locals. Stable backend symbols and ABI
records remain gated as Wave 5 work; backend input assembly rejects generated
entries at that explicit materialization boundary instead of treating them as
source `GlobalDefId` functions or thin `&fn` pointers.

Wave 3 callable construction is expected-type guided but remains explicit in
source: only `&closure` and `&mut closure` create views. Signatures match
structurally, writable state may be viewed readonly, and a readonly state
pointer cannot become writable. Body IR and Function IR use dedicated
construction and callee variants, and the generated entry remains owned by the
same `LoweredFunctionBody` product. Executable materialization remains
intentionally blocked until Wave 5 assigns stable entry symbols and ABI
records.
