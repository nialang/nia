---
name: writing-nia
description: Use before writing, editing, or reviewing Nia code (`.nia` files, `build.nia`, Nia code blocks in documentation). Nia looks like Rust but differs in many details (error unions `E!T`, `.?`, `p.*`, `and`/`or`/`not`, explicit closure captures, extension methods that need `using`, allocators passed explicitly); this skill gives the workflow, what to read first, and the checklist of Rust and C habits that do not carry over.
---

# Writing Nia

Nia is not Rust. Most mistakes come from writing Rust with Nia keywords: a
`match` on every result, a missing `using` for an extension method, `*p`,
`&&`, `==` on strings, a literal returned as a slice. The rules live in
Nia's documentation; this skill says what to read and how to work. Do not
answer questions about Nia from what Rust or C would do.

## Where The Sources Are

- **Documentation.** In a checkout of the Nia repository, read `docs/`.
  Otherwise read it at <https://github.com/nialang/nia/tree/main/docs>: at
  the tag `v<version>` for a released `nia --version`, on `main` for a
  `-dev` version.
- **Standard library source.** `lib/std/` in the repository, or `lib/std/`
  next to `bin/` in an unpacked toolchain.
- **The json library**, an official package written as an example of
  idiomatic Nia: <https://github.com/nialang/json>.

Read the files themselves, not this summary, when a detail matters.

## Read First

1. `docs/guides/writing-nia.md`: how idiomatic Nia is written (imports,
   errors, optionals, iteration, closures, ownership, abstractions, layout,
   comments, tests) and the table "Habits From Other Languages". Read it
   whole before the first edit.
2. The chapter that owns the feature in use, under `docs/learn/`:
   `errors.md` (the combinator table in "Transforming Results", `defer` in
   "Cleaning Up"), `modules.md` (which module declares which extension
   method, in "Extension Methods"), `closures.md`, `memory.md`,
   `generics-and-traits.md`, `pointers-and-slices.md`, `testing.md`.
3. `docs/reference/language.md` when a rule must be exact (precedence,
   literal typing, patterns); `docs/reference/builtins.md` for `@` builtins;
   `docs/guides/build-scripts.md` and `docs/reference/manifest.md` for
   `build.nia` and `nia.toml`.
4. The standard library source for the API itself. Before writing something
   non-trivial, find where std or json does the same thing and follow it.
   Files worth reading for style:

   | File | Shows |
   | --- | --- |
   | `lib/std/io/traits.nia` | small traits, default methods built on one primitive |
   | `lib/std/iter.nia` | lazy adapters that own their callbacks through `where` |
   | `lib/std/result.nia`, `lib/std/option.nia` | the combinators to use instead of `match` |
   | `lib/std/error/cleanup.nia` | keeping every cleanup failure |
   | `lib/std/collections/array_list/list.nia` | an owner: an allocator per call, `init` and `deinit` |
   | `lib/std/io/read_to_end.nia` | a short algorithm whose comments say why |
   | json `lib/error.nia` | a module error type that keeps its causes |
   | json `lib/write.nia` | one writer, generic over `io::Writer` |
   | json `examples/config.nia` | a complete program, matching errors at the boundary |
   | json `tests/unit.nia` | case tables and test comments |

## Workflow

1. Read the guide, then the std module about to be called.
2. Write the code the way the guide says: a typed error per module passed on
   with `.?`, `mapError` and `okOr` instead of matches, adapters or a plain
   loop, views in and owners out, the allocator passed to every call that
   allocates.
3. Add a `using` for the module of every extension method called.
4. Run `nia check` on the file or package. It checks every body of the tree
   and reports every import that nothing uses: remove those imports.
5. Lay the code out as the guide's "Layout" section says. `nia fmt <file>`
   prints a normalized copy to stdout, but it does not indent continuation
   lines as std does, so do not replace a file with its output unchecked.
6. Run every program whose output is stated in documentation, a test, or an
   answer, and compare: `nia build` for a package (`nia build run-<name>`,
   `nia build test`), `nia emit --exe file.nia -o out` for a single file.
   `io::debugPrint` writes to standard error.
7. When the compiler rejects code the documentation says is valid, reduce it
   to a small program: it may be a compiler bug, to report rather than work
   around silently.

## Checklist

Check what was written against each item; each is a habit that fails to
compile or means something else in Nia. The guide's table "Habits From Other
Languages" has the full list, and the sections above it explain each item.

- [ ] Error unions are `E!T`, error first; optionals are `?T`. Propagate
      with `.?`; build a failure with `error!` and a success with `!value`.
- [ ] No `match` whose arms only rewrap or return the error: use `.?`,
      `mapError(.Variant)`, `okOr`, `orElse`, `andThen`, or an `IntoError`
      implementation. Match where a failure's meaning is decided.
- [ ] `p.*`, not `*p`; `and`, `or`, `not`, not `&&`, `||`, `!`.
- [ ] Bit tests are parenthesized: `(flags & mask) == 0`.
- [ ] Slices compare with `.equals(...)` (`using std::slice;`), never `==`;
      `==` on pointers compares addresses.
- [ ] Closures list their captures: `\[min] x -> x > min`, `[&mut sum]`.
- [ ] The module of every extension method is imported (`std::iter`,
      `std::option`, `std::slice`, `std::result`, and others). Trait methods
      such as `len`, `iter`, and `next` need no import.
- [ ] Unused results are discarded explicitly: `_ = f();`. A cleanup result
      is kept with `defer x.deinit(allocator).?;`, and discarded with
      `defer _ = x.deinit(allocator);` only where it cannot matter.
- [ ] A slice that outlives its function points into a `static`, never into
      a literal.
- [ ] Integer literals take their type from their own context, `i32` by
      default: annotate (`let n: usize = 0;`). Overflow and shifted-out bits
      trap; use `wrapping*`, `truncating*`, or `checked*` deliberately.
- [ ] Arrays are not iterable: `for &x in &array[..]`; slices yield `&T`.
- [ ] Bounds go in `where`: `fun f[T](x: T) where T: Ord[T]`. In brackets,
      `N: usize` declares a const parameter.
- [ ] Functions, values, and constants are `camelCase`; types and variants
      `PascalCase`; files `snake_case`.
- [ ] Only `//` comments. Each item says what it is or returns. Lines stay
      within 100 columns; a long chain breaks before `.`, a long condition
      before `and` or `or`, a long literal into adjacent literals.
- [ ] Repeated cases are a table and a loop, and each test has a comment
      stating the property it checks.
