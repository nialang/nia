# Omitted Constructors

Status: proposal

This proposal introduces a leading `.` spelling for constructors whose type or
enum owner is known from context. It is intentionally an elaboration feature:
the parser records an omitted constructor, and semantic checking resolves it to
the same nominal constructor used by the explicit spelling. No new runtime
representation or pattern-analysis constructor is introduced.

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

match color {
    .Red => 0,
    .Blue => 1,
}
```

The feature is useful only where a unique expected type exists. It must not
turn an otherwise ambiguous expression into an arbitrary constructor choice.

## Surface syntax

The leading dot is a distinct constructor marker, not field access. The first
version supports:

```text
.{ field: expression, ... }       // inferred nominal struct
.Variant                          // inferred unit enum variant
.Variant(expression, ...)         // inferred tuple-payload variant (future)
.{ field: pattern, ... }          // inferred nominal pattern (future)
```

The MVP should enable only inferred struct values and unit enum variant values,
plus unit enum variant patterns. Payload-carrying enum declarations are not
currently available in Nia, so their omitted syntax is reserved rather than
partially accepted.

An omitted aggregate is not an anonymous structural value. For example,
`let p = .{ x: 1 }` remains invalid because no expected type reaches the
constructor. The existing `{ ... }` block-expression grammar is unchanged.

## Expected-type rules

An omitted constructor requires an expected type supplied by one of these
contexts:

* an explicit local, parameter, return, or field type;
* an argument position whose parameter type is known;
* an arm/result position whose enclosing expression has an expected type;
* a match pattern, where the scrutinee type is the expected type.

Expected types propagate through parentheses, references, and ordinary coercion
sites before constructor resolution. They do not propagate backwards through
an unconstrained generic variable, an overloaded call, or a type inference
hole. If resolution still has no unique nominal type, checking emits a stable
diagnostic at the leading dot:

```text
omitted constructor requires an expected nominal type
```

If the expected type is nominal but the selected field set or variant name is
not valid for it, diagnostics are the same as for the explicit constructor.
The omitted spelling must never change duplicate-field, missing-field,
visibility, or payload-shape checks.

## Enum variant lookup

For `.Red`, the checker first obtains the normalized expected type and then
looks up a variant named `Red` in that enum. Lookup succeeds only when exactly
one variant belongs to the expected enum. It does not search all visible enums,
imports, traits, associated items, or functions. A missing name reports the
existing unknown-variant diagnostic; a non-enum expected type reports:

```text
omitted variant requires an enum expected type
```

This rule also handles generic enums after their arguments have been resolved.
Variant identity passed to body IR, const IR, and pattern analysis is the same
canonical identity produced for `Enum::Red`.

## Pattern semantics

In a `match`, the scrutinee supplies the expected type for every arm pattern.
`.Red` therefore elaborates to `Color::Red` before usefulness and exhaustiveness
analysis. Missing witnesses, duplicate-arm checks, and diagnostics remain
identical to explicit patterns.

The MVP accepts only unit variants. Struct-pattern omission and payload variant
patterns should be added separately once payload pattern syntax is finalized:

```nia
match value {
    .Some(x) => x,
    .Point { x, y } => x + y,
}
```

These forms must resolve their constructor first, then recursively check payload
patterns against the declared payload types.

## Lexical and grammar constraints

The lexer already distinguishes `.` from `..` and `..=`. The constructor marker
is recognized only when a dot is followed by an identifier or `{`; `..` and
`..=` retain their range/rest meanings. Existing `value.field`, method calls,
floating-point literals, and qualified `::` paths are unaffected.

Whitespace is permitted after the marker (`. Red`), but style guidance should
prefer `.Red` and `.{ ... }`. A bare `.` is an error, not a recoverable
identifier. The parser must keep enough span information to anchor diagnostics
on the marker while preserving the child spans of fields and expressions.

## AST and implementation shape

Add syntax-only nodes (names are illustrative):

* `ExprKind::OmittedStructLiteral { fields }`;
* `ExprKind::OmittedVariant { name, payload }` (unit payload in the MVP);
* `PatternKind::OmittedVariant { name, fields }` (unit fields in the MVP).

These nodes should be accepted by parser and AST walkers, local resolution, and
const/static traversal. During body/const checking they are immediately
elaborated into the existing typed nominal literal or enum-variant forms. All
downstream consumers, including function lowering and `nia-pattern-analysis`,
continue to receive canonical constructor identities.

The implementation should be staged:

1. Parser, AST, and parser regression tests for the three MVP forms and lexical
   boundary cases.
2. Expected-type plumbing and elaboration in body checking, including explicit
   diagnostics for missing/non-nominal expected types.
3. Pattern elaboration before runtime and const usefulness analysis.
4. End-to-end tests covering locals, call arguments, match exhaustiveness,
   generic enum arguments, const values, and error recovery.
5. Update the language specification and examples after the behavior is
   accepted and stabilized.

## Deliberate non-goals

The marker is not a general shorthand for field access, methods, function
names, associated constants, or arbitrary paths. It does not infer a type from
field names alone, introduce anonymous structural typing, or relax visibility
rules. A future extension may consider optional/error-union constructors, but
those should use the same expected-type gate and canonical constructor identity
rather than adding implicit conversions.

## Open questions for review

* Should `.Variant` be allowed in `let` bindings without an explicit type when
  the initializer's type is constrained by a later use? The conservative MVP
  says no; inference is local and forward-only.
* Should references permit `&.{ ... }` directly, or should users write an
  explicit type target for readability? The elaborator can support the former
  once expected-type propagation through address-of is tested.
* When a generic function parameter is itself an enum type parameter, should
  `.Variant` be rejected until the parameter is concretely instantiated? The
  proposed rule rejects unresolved type variables to keep diagnostics stable.
