# Default Field Values

Status: proposal

This proposal allows ordinary structs to declare a default expression for a
field. A struct literal may omit fields that have defaults; the omitted fields
are filled by evaluating their declared expressions at the construction site.
The feature is a construction convenience, not a second `Default` protocol and
not an implicit call to a user-defined function.

## Motivation

Many data and configuration structs have a small required core and a larger set
of stable options. Without field defaults, every construction repeats the same
values or requires a builder or a family of configuration functions:

```nia
struct Config {
    address: Address,
    port: u16 = 8080,
    workers: u32 = 4,
    buffer: Buffer = .init(),
}

let config = Config { address: localAddress };
```

The explicit `Config` prefix still identifies the type. When the expected type
is already known, the omitted-constructor proposal composes with this feature:

```nia
let config: Config = .{ address: localAddress };
```

Defaults are useful for data whose ordinary value is stable and local to the
type. They do not replace named constructors such as `.configA()` when an
application needs multiple policies or wants construction behavior to be
obvious at the call site.

## Surface syntax

A field declaration may contain `= expression` after its type:

```text
field: type = expression,
```

The expression may be a literal, a call, or a block expression. It is parsed and
type-checked using the declared field type:

```nia
struct Point {
    x: i32 = 10,
    y: i32 = {
        let a = 8;
        let b = 12;
        (a + b) / 2
    },
}
```

The declaration does not make a field optional in the type. `Point` still has
the same fields and layout as an explicitly initialized value.

## Construction rules

For an ordinary struct literal, every field must either be written explicitly
or have a default expression. A missing field without a default remains a
compile-time error:

```nia
struct Request {
    method: Method,
    timeout: u64 = 30,
}

let ok = Request { method: Get };
let error = Request {}; // missing required field: method
```

An explicit field always overrides its declaration default:

```nia
let request = Request {
    method: Post,
    timeout: 120,
};
```

`Type {}` and `.{}` use defaults for all omitted fields. The latter requires an
expected nominal struct type under the omitted-constructors proposal; a bare
`.{}` remains invalid when no expected type exists.

The elaborated value is identical to an explicit literal containing the default
expressions at the omitted fields:

```nia
let p = Point {};
// equivalent construction:
let q = Point { x: 10, y: { let a = 8; let b = 12; (a + b) / 2 } };
```

No hidden call to `Default::default()`, trait lookup, or other global fallback
is performed.

## Evaluation and dependencies

Each omitted field expression is evaluated for each constructed value. It is not
evaluated when the type is declared, and it is not cached between constructions.
Consequently, a default may intentionally depend on runtime state, just as the
same expression written in a struct literal would.

Default expressions may refer to names visible at the struct declaration site,
but may not refer to another field, `self`, or a partially constructed value.
This keeps defaults equivalent to expressions copied into the construction site
without introducing field initialization order or recursive initialization
rules.

Struct literals evaluate field expressions in declaration order after explicit
and default values have been associated with their fields. Source order still
controls field-name association and diagnostics, but does not create a second
evaluation-order rule. This makes a default expression behave exactly like the
expression written for that field in a fully expanded literal. Diagnostics must
identify the field whose default expression failed to type-check or evaluate.

## Type checking and generics

The default expression is checked once in the context of the struct declaration
against the declared field type. Generic parameters and associated constraints
are allowed where they are valid in that declaration. A generic default must
not introduce a new implicit bound that an explicit field expression would not
need; any required bound is part of the struct's existing declaration contract.

Changing a default expression can change the value, cost, or side effects of
future constructions. Such a change is therefore a behavioral API change even
when the struct's layout is unchanged.

## `Default` and named constructors

Field defaults and a `Default` implementation are independent mechanisms:

```nia
extend Config {
    fn default() Config { ... }
    fn configA() Config { ... }
}
```

`Config {}` means “fill omitted fields from the declarations.” It does not mean
`Config::default()`, and the two may produce different values. Code that needs a
named policy or a centrally audited construction path should use `.default()` or
another named associated function. A future lint may warn when a type exposes
both mechanisms with visibly conflicting defaults, but the language does not
silently merge them.

## Visibility and API evolution

A field with a default is still subject to ordinary field visibility. A default
does not grant construction access to a private field, and omitting such a field
does not bypass the visibility check. Public construction from outside a module
therefore requires the same visibility permissions as an otherwise fully
explicit literal; code that needs to hide representation should expose an
associated constructor such as `.default()` instead. Nia currently has no
field-level visibility syntax, so this rule becomes relevant when that facility
is introduced and does not add a new visibility mechanism here.

Adding a default to an existing public field can make previously rejected
partial literals compile, and changing or removing a default can break them.
Library authors should treat default declarations as part of the source API and
document compatibility expectations accordingly.

## Initial scope

The initial feature is limited to ordinary `struct` declarations:

* tuple structs and tuple enum variants do not gain positional defaults;
* unions do not gain defaults, because exactly one active field must be chosen;
* `extern struct` does not gain defaults, keeping ABI declarations free of
  language-level construction behavior;
* patterns do not use field value defaults. Pattern `..` keeps its existing
  meaning of ignoring fields, rather than constructing values;
* defaults do not affect layout, drop behavior, or field access.

These limits avoid creating a second set of rules for active union members,
positional fields, ABI declarations, or pattern matching.

## Diagnostics

Missing required fields continue to use the existing missing-field diagnostic.
New diagnostics should be stable and point at the relevant field or literal:

```text
field `method` has no default value
default value for field `workers` has type `usize`, expected `u32`
default value for field `value` cannot refer to another field
```

An explicit field and a default never conflict; the explicit value wins. Duplicate
field names remain errors as they are for ordinary struct literals.

## Implementation shape

The implementation should elaborate defaults before ordinary aggregate checking:

1. Parse an optional default expression in each ordinary struct field and retain
   its syntax span in the AST.
2. Type-check the expression against the field type in declaration context.
3. At each struct literal, associate explicit fields, insert defaults for the
   remaining defaulted fields, and report missing required fields.
4. Lower the result to the existing fully-populated nominal aggregate form so
   layout, const evaluation, borrow checking, and code generation need no new
   aggregate representation.
5. Add tests for runtime and const construction, block defaults, explicit
   overrides, generic structs, visibility, evaluation order, omitted
   constructors, and all rejection cases in the initial-scope list.

## Deliberate non-goals

This proposal does not add anonymous structural literals, implicit
`Default::default()` calls, default arguments for functions, field-to-field
initialization, lazy defaults, memoization, or a new configuration inheritance
system. It also does not make a field optional for pattern matching or alter
whether a struct can be constructed across a visibility boundary.

## Resolved design points

The initial proposal deliberately fixes the following points:

* Default expressions are evaluated in struct declaration order. They are not
  evaluated at type declaration time and are not cached.
* A default never bypasses field visibility. Inaccessible fields must be handled
  by an associated constructor in the owning module.
* `.{}` is valid when an expected nominal struct type is available, including
  when every field is supplied by a default. Without an expected type, `.{}` is
  still rejected by the omitted-constructors proposal.

The remaining review questions are narrower implementation and API questions:

* Should a future lint flag a public type whose `default()` result differs from
  its field-default expansion, or should libraries be required to choose one
  mechanism?
* Should default expressions be permitted on generic fields whose constraints
  are only satisfied at some instantiations, or must all constraints be proven
  by the struct declaration itself?
