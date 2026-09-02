# Omitted Constructors

Status: proposal

This proposal introduces a leading `.` spelling for constructors and associated
functions whose nominal owner is known from context. It is intentionally an
elaboration feature: the parser records an omitted owner, and semantic checking
resolves it to the same constructor or associated function used by the explicit
spelling. No new runtime representation or pattern-analysis constructor is
introduced.

## Motivation

Nominal construction is deliberately explicit today:

```nia
let p: Point = Point { x: 10, y: 20 };
match color {
    Color::Red => 0,
    Color::Blue => 1,
}
```

When the surrounding expression already fixes the type, repeating the type or
enum owner adds noise. The proposed forms are:

```nia
let p: Point = .{ x: 10, y: 20 };
let c: Color = .Red;
let event: Event = .Data(42);
let list: std::ArrayList = .init();

match color {
    .Red => 0,
    .Blue => 1,
}
```

The feature is useful only where a unique expected type exists. It must not
turn an otherwise ambiguous expression into an arbitrary constructor choice.

## Surface syntax

The leading dot is a distinct omitted-owner marker, not field access. The
proposal supports:

```text
.{ field: expression, ... }       // inferred nominal struct
.Variant                          // inferred unit enum variant
.Variant(expression, ...)         // inferred tuple-payload variant
.Variant { field: expression, ... } // inferred named-payload variant
.name(arguments, ...)             // inferred associated function call

// The same enum forms are valid in patterns:
.Variant
.Variant(pattern, ...)
.Variant { field: pattern, ... }
```

The omitted form is selected from the expected type. For an enum expected type,
`.Variant`, `.Variant(...)`, and `.Variant { ... }` select an enum variant. For
a non-enum nominal expected type, `.name(...)` selects an associated function
of that type. `.{ ... }` selects a struct or union constructor.

An omitted aggregate is not an anonymous structural value. For example,
`let p = .{ x: 1 }` remains invalid because no expected type reaches the
constructor. The existing `{ ... }` block-expression grammar is unchanged.

## Expected-type rules

An omitted constructor requires an expected type supplied by one of these
contexts:

* an explicit local, parameter, return, or field type;
* an argument position whose parameter type is known;
* an arm/result position whose enclosing expression has an expected type;
* a match pattern, where the scrutinee type is the expected type;
* an associated-function call, where the expected result identifies the
  nominal owner.

Expected types propagate through parentheses, references, and ordinary coercion
sites before constructor resolution. They do not propagate backwards through
an unconstrained generic variable, an overloaded call, or a type inference
hole. If resolution still has no unique nominal type, checking emits a stable
diagnostic at the leading dot:

```text
omitted constructor requires an expected nominal type
```

If the expected type is nominal but the selected field set, variant name, or
associated function is not valid for it, diagnostics are the same as for the
explicit spelling. The omitted form must never change duplicate-field,
missing-field, visibility, generic-argument, or payload-shape checks.

## Enum variant lookup

For `.Red`, `.Data(value)`, or `.Resize { ... }`, the checker first obtains the
normalized expected type and then looks up a variant with that name in that
enum. Lookup succeeds only when exactly one variant belongs to the expected
enum. It does not search all visible enums, imports, traits, associated items,
or functions. A missing name reports the existing unknown-variant diagnostic;
a non-enum expected type reports:

```text
omitted variant requires an enum expected type
```

This rule also handles generic enums after their arguments have been resolved.
Variant identity passed to body IR, const IR, and pattern analysis is the same
canonical identity produced for `Enum::Red`.

## Associated-function lookup

For `.init()`, the expected result type must identify one concrete nominal
owner, such as:

```nia
let list: std::ArrayList = .init();
```

This is exactly the explicit call `std::ArrayList::init()`. The checker derives
the owner (including generic and const arguments) from `std::ArrayList`, then
reuses ordinary associated-function lookup, visibility checks, argument checks,
trait constraints, and return-type checking.

The expected type must identify the owner, not merely the return type of some
unknown function. Consequently, this remains explicit:

```nia
let value: Wrapper[Point] = Point::new();
```

when `Point::new()` returns `Wrapper[Point]`; `Wrapper[Point]` cannot uniquely
recover `Point` as the associated-function owner. The checker must not scan all
visible nominal types for a function named `new`.

For deterministic resolution, an enum expected type interprets `.name(...)`
as an enum variant, while a non-enum nominal expected type interprets it as an
associated function. An associated function on an enum therefore remains
explicit when its name could overlap with a variant.

## Pattern semantics

In a `match`, the scrutinee supplies the expected type for every arm pattern.
`.Red`, `.Data(value)`, and `.Resize { ... }` therefore elaborate to their full
enum constructors before usefulness and exhaustiveness analysis. Missing
witnesses, duplicate-arm checks, recursive payload checks, and diagnostics
remain identical to explicit patterns.

Payload patterns are part of the same feature because Nia already supports
their explicit forms:

```nia
match value {
    .Some(x) => x,
    .Point { x, y } => x + y,
}
```

These forms resolve their constructor first, then recursively check payload
patterns against the declared payload types.

## Lexical and grammar constraints

The lexer already distinguishes `.` from `..` and `..=`. The omitted-owner
marker is recognized only when a dot is followed by an identifier or `{`; `..`
and `..=` retain their range/rest meanings. Existing `value.field`, method
calls, floating-point literals, and qualified `::` paths are unaffected.

Whitespace is permitted after the marker (`. Red`), but style guidance should
prefer `.Red` and `.{ ... }`. A bare `.` is an error, not a recoverable
identifier. The parser must keep enough span information to anchor diagnostics
on the marker while preserving the child spans of fields and expressions.

## AST and implementation shape

Add syntax-only nodes (names are illustrative):

* `ExprKind::OmittedAggregateLiteral { fields }`;
* an omitted member/variant expression carrying a name and optional call or
  named-payload arguments;
* an omitted constructor expression usable as the constructor of an existing
  nominal pattern node.

These nodes should be accepted by parser and AST walkers, local resolution, and
const/static traversal. During body/const checking they are immediately
elaborated into the existing typed nominal literal or enum-variant forms. All
downstream consumers, including function lowering and `nia-pattern-analysis`,
continue to receive canonical constructor identities.

The implementation should be staged:

1. Parser, AST, and parser regression tests for aggregate literals, unit and
   payload variants, associated calls, patterns, and lexical boundary cases.
2. Expected-type plumbing and elaboration in body checking, including explicit
   diagnostics for missing/non-nominal expected types.
3. Pattern elaboration before runtime and const usefulness analysis.
4. End-to-end tests covering locals, return values, call arguments, nested
   aggregate values, associated calls, match exhaustiveness, generic nominal
   arguments, const values, and error recovery.
5. Update the language specification and examples after the behavior is
   accepted and stabilized.

## Deliberate non-goals

The marker is not a general shorthand for field access, receiver method calls,
ordinary function names, associated constants, or arbitrary paths. It does not
infer a type from field names alone, introduce anonymous structural typing, scan
the global function namespace, or relax visibility rules. Optional and
error-union values retain their existing `?value`, `null`, `!value`, and error
propagation syntax; this proposal does not add a second spelling for them.

## Open questions for review

* Should an omitted associated call be allowed when the expected type is an
  alias that normalizes to a nominal owner? The proposed answer is yes, using
  the same alias normalization as explicit associated calls.
* Should references permit `&.{ ... }` directly, or should users write an
  explicit type target for readability? The elaborator can support the former
  once expected-type propagation through address-of is tested.
* When a generic function parameter is itself an enum or nominal type parameter,
  should omitted members be rejected until the parameter is concretely
  instantiated? The proposed rule rejects unresolved type variables to keep
  diagnostics stable.
