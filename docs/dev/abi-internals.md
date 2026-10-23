# ABI Implementation Notes

Compiler-side notes for the ABI described in
[`docs/reference/abi.md`](../reference/abi.md). The reference owns the
user-visible rules; this file records how the compiler implements them and
which invariants each stage must keep.

## Ownership

The checked program owns the ABI; backends consume it.

- `nia-layout` computes the one canonical size, alignment, field offsets, and
  aggregate classification of every runtime type for a target, from an explicit
  `TargetDataLayout` (`pointer_size`, `pointer_align`, `wide_scalar_align`).
  No phase may fall back to host assumptions.
- `nia-abi-check` validates C boundaries and produces the cacheable
  `AbiSignature` product for every function in both domains: ABI domain, target
  data model, parameter modes, return mode, and an ordered hidden-parameter
  list (prefix sret pointer, suffix caller-location pointer). It consumes
  demand-driven type and layout queries. C legality is checked separately at
  the declaration boundary.
- `nia-abi-check::c_abi` classifies C signatures per target convention from
  byte layouts only (no LLVM or type-store dependency). Codegen turns the
  result into LLVM types and attributes.
- `nia-mangle` encodes linkage names.

Lowering and codegen must not recompute struct layout from source order,
choose aggregate signatures independently, or silently change a C signature.
LLVM is a consumer of `AbiSignature`; it never derives a second calling
convention. A program that exposes a Nia-only representation through the C ABI
is rejected; the compiler never synthesizes an adapter representation.

Any optimization that would change size, alignment, field offsets, parameter or
return passing, static data, or symbol identity is an ABI change and belongs in
the reference, not in a pass.

## Layout Checks

`extern` and Nia-owned aggregates use the same primitive alignments
(`wide_scalar_align` is 4 only on i386). Codegen checks every LLVM aggregate
body against its Nia layout; a disagreement is an internal compiler error, never
a silent difference between `std::builtin::size[T]()` and the storage LLVM
uses.

Field access, aggregate literals, static initializers, copies, loads, and
stores go through field identity (`DefId`) and the physical field table in
`StructLayout`. Never assume that the source field index equals the physical
index. Tuple structs, tuples, and closure states use `sequential_layout`
(declaration order); named Nia structs use the sorted order.

Layout arithmetic is checked: overflow becomes a diagnostic, not a wrapped
layout.

## Symbol Identity

A `StableSymbolKey` holds the canonical package identity, normalized module
identity, stable definition identity, item kind, source name, and canonical
generic arguments. The package identity is mandatory, so two packages with the
same module and definition spellings never share a linker namespace. The
encoder writes a length-delimited binary record rendered in the linker-safe
`_N` namespace; `demangle_stable_symbol` validates and decodes it. Names are
self-terminating and cannot collide through delimiter escaping or Unicode
sanitization.

The record contains no release version, layout hash, calling convention, or
target. Those belong to the program's ABI metadata and the protocol epoch
identity.

Backend IR linkage is a closed contract:

```text
Nia                         canonical Nia mangling
ExternImport { symbol }     C declaration resolved from `symbol`
ExternExport { symbol }     C definition published as `symbol`
```

Only bodyless `extern` declarations take `linkName`; only `extern` definitions
take `exportName`. There is no `noMangle`. Compiler-generated symbols without
a source package (vtables, source-metadata records) use the reserved
compiler-generated package identity and remain ordinary `_N` symbols.
Exact-key monomorphized instance deduplication is required at every
optimization level; the `dedup_monomorphized_instances` report field must not
make it optional or merge instances that need distinct symbols.

Only `ExternImport`/`ExternExport` symbols get default visibility; everything
else is hidden, so references need no GOT indirection.

## Nia Function Classification

Parameters:

```text
scalar, pointer, function pointer   direct
slice                               direct descriptor
ZST                                 omitted
aggregate                           indirect readonly address
```

Returns:

```text
scalar, pointer, function pointer   direct
slice                               direct descriptor
() and other ZSTs                   no runtime value
aggregate                           hidden out pointer (sret)
never                               no normal return
```

An indirect readonly address is the by-value parameter's storage; the callee
must not mutate it. Method receivers follow the receiver type (`&self` and
`&mut self` are pointers, `self` is an ordinary value parameter). Generic
functions are classified after monomorphization, on concrete types only.

### Tracked Caller Metadata

`@[trackCaller]` appends one compiler-owned pointer after all source
parameters:

```text
hidden result pointer -> source parameters -> caller-location pointer
```

It addresses immutable `std::SourceLocation` storage
(`{ file: &[u8], line: u32, column: u32 }`). An untracked caller passes a
link-time deduplicable constant for the call site; a tracked caller forwards
its incoming pointer. Trait-object entries and receiver adapters use the same
final position. The pointer is not part of semantic function types, local
numbering, reflection, or arity. Tracked functions cannot use the C ABI or be
represented by function pointers or callable views; monomorphization,
inlining, and vtable lowering must keep the tracked/untracked distinction.

## Callables

A closure entry takes the state as a hidden readonly first parameter:
`entry(state, Args...) -> Return`. Entries are never variadic. Entry symbols
include the concrete source or generic-instance owner and the stable closure
ordinal, so an instance never collides with its template or another instance.

Callable entry symbols and ABI records are produced by backend lowering,
reachability, partitioning, and incremental fingerprinting. LLVM declares and
defines each entry in the codegen partition of its source function or generic
instance. Direct closure calls pass the state pointer; view calls load the
state and entry words and call indirectly with the same ABI.

Function-pointer-to-view conversion uses a private signature-specific adapter
whose state word carries the thin function pointer, so every view dispatches
through one `entry(state, args...)` shape with no null-state sentinel or
runtime function-versus-closure branch. A no-capture closure converted to
`&fun` uses a separate thin adapter, because the thin ABI has no state
parameter.

`std::mem::Allocated[T]` and `CallableAllocation[V]` are ordinary library
aggregates, not ABI primitives: the compiler supplies only closure state
layout, entry metadata, and casts, with no allocator hook, implicit heap move,
or drop glue. The library round-trips release pointers through integers so the
caller-managed lifetime is explicit to the closure provenance analysis.

## Enum Constructor Helpers

`&Enum::Variant` for a payload variant materializes a compiler-generated
internal helper with the thin Nia function-pointer ABI. Its parameters are the
payload fields in declaration order and its result is the enum value. It is not
a source function or C symbol. Unit variants have no helper.

## Static Data Emission

Static aggregate initializers are matched by field name and emitted in
physical order. A readonly promoted ZST allocation whose address is observable
may use one byte of backend identity storage; that byte is not part of the
ZST's layout and does not change its size, alignment, passing, or dereference
semantics.

Static initializer simplification is representation-preserving: repeated
arrays may become `zeroinitializer` or a repeated initializer, and byte arrays
may be emitted as constant strings, without changing type, layout, or
addressability.

## Startup, Runtime Entry, And Linking

Executable startup is owned by the selected toolchain runtime, a private
resource under the internal runtime package identity, not public std API.

- Linux x86_64 and i686 runtimes export `_start` as an `extern fun`
  (`ExternExport`) and call the Nia root entry contract. The i686 runtime uses
  the `int 0x80` syscall ABI and is experimental.
- `_start` and its platform startup function (`niaStartStack` on Linux,
  `niaWindowsStart` on Windows) are the runtime's external codegen roots. The
  naked `_start` names its startup function in assembly, so the runtime roots
  it by module and definition. All other startup code is ordinary Nia whose
  reachability follows calls.
- Native helpers referenced implicitly by codegen, such as Windows `__chkstk`,
  are rooted by runtime source module and definition.
- On Darwin the source entry is `main`, spelled `_main` for Mach-O; `dyld`
  enters through `LC_MAIN` and it forwards to `niaDarwinStart`.
- Linux executables are static-pie by default. `_start` passes `__ehdr_start`
  and `_DYNAMIC` to `niaStartStack`, whose first statement applies the image's
  RELATIVE relocations (REL, RELA, or RELR) before any relocated address is
  read. The relocator uses only locals, reads only the image, and calls only
  hidden PC-relative functions. For `static` (no dynamic section) and
  `dynamic` (relocated by the interpreter) it returns without writing. The form
  is a link option (`--executable static-pie|static|dynamic`); objects are
  identical for all three.
- LLVM object emission uses the artifact's explicit triple, baseline CPU,
  features, PIC relocation model, and default code model. All are part of the
  native object and LTO cache identity.
- Vtables and other address-holding data go in `.data.rel.ro`.

## C ABI Classification

`c_abi::classify_c_function` follows clang per convention (System V AMD64
eightbyte classification with register counting, i386 expansion of small
padding-free structs, Win64 register-sized aggregates as integers, AAPCS64
homogeneous float aggregates). Variadic extras are promoted with
`CType::promoted_variadic` before classification. Declarations are compared
with clang's output for each maintained target, and value exchange with C runs
on the Linux targets (`crates/nia-cli/tests/c_abi.rs`).

## Optimization Invariants

Optimization levels may change backend IR shape, instruction sequences,
temporaries, and removed redundant work. They must not change:

- type size or alignment for a target;
- field identity or physical field offsets;
- C field order or function signatures;
- Nia parameter and return classification, including sret use and ZST
  omission;
- static data layout and relocation meaning;
- any symbol identity;
- diagnostics required by earlier phases.

`-Os`/`-Oz` may deduplicate, inline less, merge wrappers, or simplify
initializers only when the ABI surface is unchanged. Backend cleanup passes and
the LLVM codegen level must not become a second layout, mangling, or
calling-convention authority. The `--opt-report` output is an observability
tool, not an ABI contract.

## Backend Lowering Contract

Backends receive ABI-compatible types, parameters, returns, globals, field
offsets, and symbol names from Nia lowering and may only make
instruction-level choices. For zero-sized values lowering may omit storage and
operands, but must keep source checks, field and type identity, and evaluation
order: side effects inside a zero-sized aggregate literal are still emitted.
Inline assembly operands use ABI representations; outputs cannot be `()`,
`never`, or other non-material types.
