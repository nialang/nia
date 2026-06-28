# Nia ABI Reference

Status: ABI reference

This document defines the ABI model for Nia. It is a reference for language
design, compiler maintenance, tests, and backend work.

The Nia ABI is not a stable public binary ABI yet. It is, however, the canonical
description of how Nia-owned values, aggregates, functions, symbols, and C ABI
boundaries behave inside one compiler version.

## 1. ABI Domains

Nia has two ABI domains.

The Nia ABI is used for normal Nia definitions:

```nia
struct Pair {
    a: u8,
    b: i64,
}

fn sum(pair: Pair) i64 {
    pair::b
}
```

The C ABI is used for explicit external boundaries:

```nia
extern fn foreign_log(message: &u8) void;

extern struct CPoint {
    x: i32,
    y: i32,
}
```

The two domains are intentionally separate:

- normal Nia structs do not promise C layout;
- `extern struct` definitions do not receive Nia field reordering;
- normal Nia functions use Nia internal symbol naming;
- `extern` functions and globals use external C symbol names;
- crossing between Nia-owned values and C ABI values must be explicit and
  checked.

## 2. Stability

The Nia ABI is internal and unstable until the project explicitly declares a
stable ABI version.

Within one compiler version, all modules in a checked program must use one
consistent ABI model. Across compiler versions, object-file compatibility is not
guaranteed.

The C ABI boundary is the only cross-language ABI surface.

Nia-mangled symbols do not encode ABI version numbers, layout hashes, crate
hashes, or stable binary compatibility identity. They assume the current
compiler's Nia ABI rules.

## 3. Terminology

ABI means application binary interface. In this document, ABI includes:

- type size and alignment;
- field offsets and padding;
- aggregate layout policy;
- function parameter and return representation;
- symbol naming and linkage;
- C ABI boundary validation;
- static data representation;
- target data model assumptions;
- backend lowering contracts;
- ABI-visible optimization rules.

Aggregate means a value made from other values. Nia aggregates include structs,
unions, arrays, and slices.

Scalar means a single direct machine-level value, such as an integer, float,
pointer, function pointer, or enum backing value.

ZST means zero-sized type. `void` and empty structs are ZSTs. A ZST has a type
and may have values, but it has no runtime storage.

Padding means bytes inserted by the layout algorithm to satisfy alignment.

Field reordering means choosing a physical field order different from source
field order. Source order remains the declaration order used by field names,
field initialization, diagnostics, and documentation.

## 4. ABI Ownership

The ABI is owned by the checked program representation, not by backend-specific
code.

Layout computation produces the canonical size, alignment, field offsets, and
aggregate classification for every runtime type in a target.

Lowering and code generation must consume canonical ABI metadata. They must not
recompute struct layout from source field order, choose target-dependent
aggregate signatures independently, or silently change C ABI signatures.

Name mangling is deterministic source-level identity. It does not carry layout
metadata. All Nia-mangled symbols in one linked program are interpreted under
the same ABI model.

ABI checks are validation boundaries. A program that asks to expose a Nia-only
representation through the C ABI must be rejected instead of receiving an
implicit adapter representation.

ABI-visible optimizations are part of the ABI. If an optimization changes type
size, alignment, field offsets, parameter passing, return passing, static data,
or symbol identity, that behavior belongs in this document.

## 5. Target Data Model

ABI layout must be computed from an explicit target data model. Compiler phases
must not silently use host assumptions.

The current default model is LP64:

```text
pointer size   8
pointer align  8
usize/isize    8
```

The target data model controls pointer-sized types, aggregate alignment, static
data layout, and backend lowering.

## 6. Primitive Representation

For the current LP64 model:

```text
i8/u8/bool      size 1,  align 1
i16/u16         size 2,  align 2
i32/u32/f32     size 4,  align 4
char            size 4,  align 4
i64/u64/f64     size 8,  align 8
i128/u128       size 16, align 16
isize/usize     size 8,  align 8
void            size 0,  align 1
never           size 0,  align 1
```

`void` is a first-class zero-sized value type.

`never` is the never type. It has no values. Its zero-sized layout exists only so
compiler phases can reason about diverging expressions consistently.

## 7. Pointer Representation

Pointers are scalar pointer-sized values:

```text
&T              pointer-sized
&mut T          pointer-sized
^T              pointer-sized
^mut T          pointer-sized
&void           pointer-sized
&mut void       pointer-sized
```

For LP64:

```text
size 8, align 8
```

`&void` is an opaque pointer target. It means that the pointee type has been
erased. It does not mean that a `void` object can be read or written.

Dereferencing `&void` is invalid:

```nia
let mut p: &void = &value as &void;
p.* // invalid
```

Pointer erasure to `&void` is explicit:

```nia
let mut p: &void = &value as &void;
let mut q: &mut void = &mut value as &mut void;
```

Nia does not perform implicit `&T -> &void` coercions.

Volatile pointers `^T` and `^mut T` have the same ABI representation as ordinary
thin object pointers. Volatility is an access property: loads and stores through
these pointer types must be emitted as volatile memory operations. It does not
change pointer size, alignment, parameter classification, or C ABI pointer
representation.

## 8. Function Pointer Representation

Function pointers are scalar pointer-sized values.

For LP64:

```text
&fn(...)  size 8, align 8
```

Function pointer type identity includes:

- parameter types;
- return type;
- variadic marker;
- let function pointer marker.

Variadic function pointers are not accepted at C ABI boundaries.

## 9. Array Representation

Arrays are contiguous repeated element storage:

```text
[N]T size  = N * size(T)
[N]T align = align(T)
```

Array length must be concrete by layout time.

Arrays are Nia aggregates. Passing arrays by value through C ABI boundaries is
not defined.

Arrays of ZST elements have size `0` and alignment equal to the element
alignment.

## 10. Slice Representation

A slice is a Nia descriptor:

```text
slice = { ptr, len }
```

For LP64:

```text
&[T]        size 16, align 8
&mut [T]    size 16, align 8
```

The pointer component points at the first element. The length component is a
`usize` element count.

Slices are Nia ABI values. They are not C slices and must not be passed directly
through C ABI boundaries.

## 11. Optional And Error Union Representation

Optional and error union values are Nia tagged unions. The current representation
is a tag byte followed by storage large and aligned enough for the largest
payload:

```text
?T    = { tag: u8, payload: T }
E!T   = { tag: u8, payload: max(E, T) }
```

For `?T`, tag `0` is `null` and tag `1` is the present value. For `E!T`, tag `0`
is success and tag `1` is error. This representation is owned by the Nia ABI and
is not a C ABI contract.

Optional and error union values are aggregates for parameter and return
classification. At C ABI boundaries they are rejected by value.

## 12. Struct Representation

Nia has two struct layout policies:

- Nia struct layout for normal structs;
- C struct layout for `extern struct`.

### 11.1 Nia Struct Layout

Normal structs use Nia-owned layout:

```nia
struct A {
    a: u8,
    b: i64,
    c: u8,
}
```

Source field order is declaration order. Physical field order is ABI-owned and
may be optimized.

The Nia field layout algorithm is deterministic:

```text
sort fields by:
  1. descending alignment
  2. descending size
  3. ascending source declaration index
```

After sorting, fields are placed in physical order with normal alignment
padding. Struct size is rounded up to the final struct alignment.

For example, the source order:

```text
a: u8, b: i64, c: u8
```

may physically become:

```text
b: i64, a: u8, c: u8
```

The compiler must use field identity and layout tables for field access. It must
not assume that source field index equals physical field index.

### 11.2 C Struct Layout

`extern struct` uses C ABI layout:

```nia
extern struct CPoint {
    x: i32,
    y: i32,
}
```

Fields remain in declaration order. Padding and final size follow the target's
C-compatible layout model.

`extern struct` is not eligible for Nia field reordering, ZST compression beyond
what the C ABI model permits, or other Nia-only aggregate optimizations.

### 11.3 Public Nia Structs

`pub struct` does not imply C layout.

Public Nia structs still use Nia layout. Cross-module use is safe because the
compiler shares layout information through the checked program representation.

If a type must be consumed by C or another external ABI user, it must use a type
form whose representation is defined for that ABI boundary, such as
`extern struct`.

### 11.4 Empty Structs

Empty structs are ZSTs:

```nia
struct Empty {}
```

Their layout is:

```text
size 0, align 1
```

Empty struct values have no runtime storage.

### 11.5 ZST Fields

ZST fields contribute no storage.

A normal Nia struct may contain ZST fields. Those fields are semantically
present and addressable rules may be defined by the language, but they do not
consume bytes in the physical Nia layout.

If every field is zero-sized, the struct is also zero-sized with alignment `1`.

## 12. Union Representation

Nia unions are C-style untagged unions:

```nia
union Bits {
    i: i32,
    f: f32,
}
```

Union layout is:

```text
size  = max(size(field))
align = max(align(field))
all field offsets = 0
```

Empty unions are not part of Nia.

Normal unions use Nia-owned layout.

`extern union` uses a C-compatible union layout for its target. All fields have
offset `0`, size is rounded according to the target C ABI rule, and alignment is
the maximum field alignment.

C ABI union by-value passing is not defined by this ABI. Passing a union across
the C ABI must use a pointer to an `extern union` or another explicitly defined
C-compatible representation.

## 13. Enum Representation

C-style Nia enums use an integer backing type:

```nia
enum Color: u8 {
    Red,
    Green,
}
```

The enum representation is the representation of its backing type.

Enum values are not passed directly through C ABI boundaries. C ABI code should
use the backing integer type explicitly.

Nia does not use niche or payload-carrying enum representations. Plain C-style
enum layout is backing-type based.

## 14. Function ABI

Normal Nia functions use the Nia function ABI.

`extern` functions use the C function ABI.

Executable startup is owned by the selected standard-library runtime. The
default Linux x86_64 runtime exports `_start` as an `extern fn` symbol and calls
the Nia-level root entry contract from standard-library code.

### 14.1 Nia Function Parameters

The Nia function ABI classifies parameters as follows:

```text
scalar parameters      direct
pointer parameters     direct
function pointers      direct
slice parameters       direct descriptor
ZST parameters         omitted
aggregate parameters   indirect readonly address
```

Direct means the value is passed as an LLVM-level scalar or scalar aggregate
matching the Nia representation.

Omitted means the parameter exists semantically but has no runtime parameter.

Indirect readonly address means the caller passes an address of a value whose
contents are observed by the callee. The callee must treat this address as the
by-value parameter's storage and must not mutate it.

### 14.2 Nia Function Returns

The Nia function ABI classifies returns as follows:

```text
scalar returns         direct
pointer returns        direct
function pointers      direct
slice returns          direct descriptor
void returns           no runtime return value
ZST returns            no runtime return value
aggregate returns      hidden out pointer
never returns          no normal return
```

Hidden out pointer means the caller provides result storage and the callee writes
the aggregate result into it.

### 14.3 Methods

Methods are functions with an explicit receiver convention.

Receiver lowering follows the receiver type:

```text
&self        pointer parameter
&mut self    mutable pointer parameter
self        classified as a normal value parameter
```

Method receiver ABI must follow the same parameter classification rules as other
Nia function parameters.

### 14.4 Generic Functions

Generic functions are monomorphized.

Each concrete instance uses the Nia ABI after substituting concrete type
arguments. Layout and function ABI classification are performed on the concrete
types, not on generic parameters.

## 15. C ABI Function Boundaries

`extern fn` declarations and definitions are C ABI boundaries.

The compiler must reject Nia-only types at C ABI boundaries unless this document
defines a C-compatible representation for them.

C ABI policy:

```text
generic extern fn               rejected
extern variadic fn with body    rejected
bool by value                   rejected
char by value                   rejected
void parameter                  rejected
void return                     allowed
never return                    rejected
Nia slice by value              rejected
array by value                  rejected
normal Nia struct by value      rejected unless represented as extern struct
empty struct by value           rejected
union by value                  rejected
Nia enum by value               rejected; use the backing integer type
variadic function pointer       rejected
pointer values                  allowed when the pointee boundary is meaningful
volatile pointer values         allowed with ordinary pointer representation
```

`extern struct` is the C-layout aggregate form. Ordinary Nia structs are not
C-layout types.

## 16. Static Data ABI

Top-level `let` and `let mut` storage uses the ABI representation of its type.

Static aggregate initializers must be emitted in physical layout order, not
source field order.

For Nia structs with reordered fields:

- semantic field names are matched from source initializers;
- backend static data is emitted using physical field order;
- padding is target-defined and not directly addressable.

ZST globals have semantic identity in the compiler but require no runtime
storage.

Extern globals use external symbol names and C ABI-compatible types.

## 17. Modules And Cross-Module ABI

All modules in one checked program use one ABI model.

Cross-module public Nia items still use the Nia ABI:

```nia
// geom.nia
pub struct Point {
    x: i32,
    y: i32,
}

// main.nia
using root::geom;
```

`pub` controls source-level visibility. It does not imply C ABI visibility,
C layout, or unmangled symbols.

Cross-module layout must be communicated through compiler metadata, not
recomputed from source text in later phases.

## 18. Symbol Naming And Linkage

Normal Nia symbols use Nia mangling.

Extern symbols use external names.

Executable runtime entry points are `extern fn` definitions exported with their
source names, such as `_start`.

Symbol policy:

```text
normal function              Nia-mangled
generic function instance    Nia-mangled with type arguments
method                       Nia-mangled
generic method instance      Nia-mangled with type arguments
normal global                Nia-mangled
extern function              external source name
extern global                external source name
runtime entry extern fn      external source name
```

Nia mangling must be deterministic and must distinguish generic instances.

The Nia ABI does not encode ABI versions or layout hashes in symbols. Stable
binary compatibility requires a separate explicit compatibility model.

## 19. Optimization Rules

ABI optimizations must preserve source semantics.

Allowed Nia ABI optimizations include:

- field reordering for normal structs;
- padding reduction;
- ZST field elision;
- omitted ZST function parameters;
- no runtime value for ZST returns;
- aggregate return via hidden out pointer;
- aggregate parameter passing by address;
- descriptor-style slice passing.

Disallowed optimizations include:

- changing `extern struct` field order;
- changing C ABI function signatures silently;
- treating `pub struct` as C layout;
- making source field index equal physical field index by assumption;
- using backend-specific layout decisions that are not reflected in layout
  metadata;
- changing symbol names nondeterministically.

Every optimization that affects binary representation is part of the Nia ABI and
must be reflected in layout, lowering, codegen, tests, and documentation.

Nia optimization levels are `-O0`, `-O1`, `-O2`, `-O3`, `-Os`, and `-Oz`, with
`-O` meaning `-O2`. These levels select an internal optimization policy rather
than directly exposing LLVM's codegen levels. Backend-only instruction selection
optimizations may vary by target backend, but ABI-visible optimizations must
still satisfy this section regardless of optimization level.

Exact-key monomorphized instance deduplication is required at every
optimization level for deterministic symbol identity. The
`dedup_monomorphized_instances` policy field exposes this boundary for current
reports and future size-oriented cross-instance deduplication; it must not make
required exact-key deduplication optional or merge instances that need distinct
ABI-visible symbols.

Optimization levels may change backend IR shape, generated instruction
sequences, temporary storage, and whether unreachable or redundant runtime work
is emitted. They must not change:

- type size or alignment for a target;
- Nia field identity or physical field offsets;
- C ABI field order or function signatures;
- Nia function parameter and return classification;
- hidden out-pointer use for aggregate returns;
- omitted runtime representation for ZST parameters and returns;
- static data layout and relocation meaning;
- normal, generic, method, extern, global, and runtime entry symbol identity;
- diagnostics and semantic checks required by earlier phases.

The current backend cleanup passes are backend-visible only. Removing
unreachable Function IR blocks, merging empty jump blocks, folding constant
boolean branches, removing same-type casts, removing no-op local stores,
discarding pure value-only expression statements, removing zero-sized local
runtime binding/store operations while preserving initializer effects, removing
unused compiler-generated temporary bindings, propagating local copies, removing
overwritten local stores, removing never-read local stores, and removing unused
user local bindings must preserve the ABI metadata chosen before backend
lowering. These passes may make generated code smaller or clearer, but they
must not become a second layout, name-mangling, or calling convention authority.

Size-oriented levels (`Os` and `Oz`) have an additional constraint: reducing code
size must not create a new ABI variant. They may prefer deduplication, smaller
inlining thresholds, less specialization, wrapper/thunk merging, repeated
constant merging, static initializer simplification, or vtable data
deduplication only when the resulting program still exposes the same ABI
surface defined by this document. Trivial single-parameter forwarding wrapper
inlining is ABI-neutral because it removes only the local call wrapper and keeps
the argument expression's evaluation as the resulting value.

O3 cross-function constant propagation and direct trait-call devirtualization
are ABI-neutral. Cross-function constant propagation may replace a call to a
no-argument leaf function or function instance with the backend constant that
the callee returns. It does not change the callee symbol, function ABI, or the
representation of the constant value. Direct trait-call devirtualization may
replace a backend IR dynamic trait method call with a direct concrete method
call only when the receiver is a local trait-object coercion and the selected
implementation is known before codegen. It does not change the trait object
representation, vtable layout, object-safe method ABI, or any externally
visible symbol; calls whose receiver may carry runtime metadata still use the
normal vtable dispatch ABI.

The LLVM codegen optimization level and size policy reported by
`nia check <file.nia> --opt-report` and the
`emit ... <file.nia> --opt-report` commands are also backend-visible only.
They may affect
instruction selection, scheduling, register allocation, and equivalent
object-code details after Nia lowering, but they must not reinterpret Nia layout
metadata, symbol identity, function ABI classification, static data meaning, or
field offsets. `Os` and `Oz` still rely on the Nia optimization policy for
size-aware monomorphization, inlining, specialization, and deduplication before
LLVM sees the lowered program. The current native LLVM target-machine setup
uses the mapped LLVM codegen optimization level; the reported size policy
records the Nia/codegen size boundary and is not an ABI-visible target-machine
setting.
The optimization report is an observability tool, not an ABI contract. It may
list enabled passes and changed backend IR contexts, but ABI compatibility is
defined by the representation and calling-convention rules in this document.

Static initializer simplification is representation-preserving. For example, a
repeated zero array may be emitted as an equivalent backend `zeroinitializer`,
and repeated nonzero array, byte-string, or char-string data may be canonicalized
to an equivalent repeated initializer. The global's type, layout, addressability,
and element values remain the same as the explicit initializer. LLVM codegen may
also emit repeated byte-array initializers as equivalent constant strings; this
is a backend-visible encoding choice for the same `[N]u8` object, not a new
static data representation.

Aggregate codegen cleanup is also representation-preserving. An aggregate
literal may be materialized directly into local storage, an indirect readonly
argument copy, or the function's hidden aggregate return storage instead of
first building a separate temporary aggregate value and copying it. Likewise, an
aggregate-returning call may reuse the destination local, indirect argument
copy, or hidden return pointer as its out pointer when doing so preserves defer
and initializer evaluation order. These choices may remove temporary storage and
copies, but they do not change aggregate layout, field identity, parameter and
return classification, or externally visible ABI.

## 20. Inline Assembly Boundary

Inline assembly observes backend-level values and machine registers.

Values passed to inline assembly must use their ABI representation. Nia-only
aggregates should not be passed to inline assembly unless the compiler has a
defined lowering for that operand class.

Inline assembly outputs cannot have `void`, `never`, or other non-material
runtime types.

## 21. Backend Lowering Contract

Backend lowering must preserve the ABI representation chosen before codegen.

LLVM or another backend may choose instruction-level optimizations, but it must
receive ABI-compatible types, parameters, returns, globals, field offsets, and
symbol names from Nia lowering.

The backend representation of a Nia aggregate may be different from a source
aggregate shape. Field access, aggregate literals, static initializers, copies,
loads, and stores must use ABI field identity and physical layout metadata.

For zero-sized values, lowering may omit storage and runtime operands. Omission
must not remove source-level checks, field identity, type identity, or evaluation
order that the language requires. If a zero-sized aggregate literal contains
field or element initializers with side effects, those effects must still be
emitted even though the aggregate itself has no runtime representation.

## 22. ABI Invariants

The following invariants must hold:

- every type used at runtime has exactly one ABI layout for a target;
- field access uses field identity and layout metadata;
- static initialization uses physical layout order;
- C ABI boundaries reject unsupported Nia-only representations;
- normal Nia functions and generic instances use Nia ABI classification;
- ZST values do not require runtime storage;
- `extern` symbols are not Nia-mangled;
- Nia-mangled symbols are deterministic;
- all modules in one checked program agree on target layout and ABI rules.
