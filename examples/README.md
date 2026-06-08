# Nia Examples

Small teaching programs for the current Nia compiler. Each file focuses on one
part of the language, while `modules/` shows multi-file imports, `using`, and
`pub using`.

Run an example from the repository root with:

```sh
cargo run -p nia-cli -- check examples/00_hello_main.nia
cargo run -p nia-cli -- emit --llvm examples/00_hello_main.nia
```

Most examples are hosted or inspection-oriented examples. They can be checked,
emitted as LLVM IR, or emitted as object files for an external build flow.

The freestanding executable example uses the current `emit --exe` startup
contract:

```sh
cargo run -p nia-cli -- check --exe examples/12_freestanding_executable.nia
cargo run -p nia-cli -- emit --exe examples/12_freestanding_executable.nia -o build/nia-freestanding
build/nia-freestanding
```

`emit --obj` and `emit --exe` create missing output directories, so `build/`
does not need to exist before these commands run.

For the module example, check the root file:

```sh
cargo run -p nia-cli -- check examples/modules/main.nia
cargo run -p nia-cli -- emit --llvm examples/modules/main.nia
```

The examples are intentionally compact, but hosted examples may call libc
directly through `extern fn` declarations. That keeps the language core small
while still making emitted IR and object files observable through external
build flows.

## Reading Order

- `00_hello_main.nia`: hosted `main`, libc `printf`, basic functions.
- `01_bindings_and_literals.nia`: `let`, `var`, primitive literals, casts.
- `02_arrays_slices_strings.nia`: arrays, typed array literals, slices,
  strings, C strings, pointers.
- `03_structs_unions_enums.nia`: aggregates, enum switches, switch pattern
  lists, switch ranges, typed struct literals, and temporary references.
- `04_control_flow_defer.nia`: `if` expressions, block expressions, `for`
  ranges, typed loop bindings, `while`, `loop`, `break`, `continue`, `return`,
  `switch`, and `defer` cleanup order.
- `05_functions_pointers_extern.nia`: function items, function pointers,
  `extern`.
- `06_generics_methods.nia`: generic functions, generic structs, typed generic
  literals, methods.
- `07_traits_operators.nia`: traits, `where`, default methods, operator traits.
- `08_associated_types_trait_objects.nia`: associated types, trait objects,
  explicit supertrait associated type bindings, object upcasts.
- `09_comptime_and_layout.nia`: `comptime`, `comptime if`,
  `@builtin().target`, attributes, static data, `@size`, `@align`.
- `10_inline_asm.nia`: `@asm` configuration shape.
- `11_optional_error_union.nia`: `?T`, `E!T`, `null`, `.?`, and `switch`
  destructuring.
- `12_freestanding_executable.nia`: direct `std.process` and `std.io` imports,
  plus the `emit --exe` freestanding startup contract.
- `modules/main.nia`: import paths, aliases, `using`, grouped `using`, and
  re-exports.
