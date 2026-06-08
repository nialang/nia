# Nia Examples

Small executable Nia programs for the current compiler and standard library.
Each top-level `.nia` file uses the current entry contract:
`pub fn main(process::Init) process::ExitCode!void`.

Most examples print their results through `std.debug.print`, a stderr debug
printing helper for small programs and diagnostics. It keeps example programs
focused on the language feature being shown; if the underlying stderr write or
flush fails, it traps instead of silently ignoring the failure.

`03_stdout.nia` shows the explicit application-output path with
`std.io.FileWriter` and `std.fmt`: create a stdout buffer, write formatted text,
flush the writer, and map I/O error unions into process exit codes with
`io_call().exit().?`. Format arguments are passed as ordinary array literals,
such as `[&value, &count]`; Nia converts the array to the expected slice.

Run an example from the repository root with:

```sh
cargo run -p nia-cli -- check --exe examples/00_minimal.nia
cargo run -p nia-cli -- emit --llvm examples/00_minimal.nia
```

Check all examples from the repository root with:

```sh
for file in examples/*.nia; do cargo run -p nia-cli -- check --exe "$file"; done
cargo run -p nia-cli -- check --exe examples/modules/main.nia
```

Build and run an executable:

```sh
cargo run -p nia-cli -- emit --exe examples/03_stdout.nia -o build/nia-stdout
build/nia-stdout
```

`emit --obj` and `emit --exe` create missing output directories, so `build/`
does not need to exist before these commands run. The examples do not depend on
libc or a C runtime; external C ABI interop belongs in dedicated interop
examples, not in the default executable path.

## Reading Order

- `00_minimal.nia`: the smallest debug-printing executable entry.
- `01_values_control_flow.nia`: structs, methods, slices, ranges, `switch`,
  `defer`, debug printing, and exit-code checks.
- `02_slices_and_strings.nia`: arrays, slices, byte strings, C strings, and
  mutable slice writes.
- `03_stdout.nia`: standard-library stdout through `std.io` and `std.fmt`.
- `04_array_list.nia`: `std::ArrayList` with `std.mem.PageAllocator`, printed
  length and capacity.
- `05_traits_generics.nia`: traits, generic bounds, operator traits, and a
  formatted computed total.
- `06_optional_error.nia`: optional values, error unions, propagation, and
  printed success/error state.
- `modules/main.nia`: file modules, aliases, selected `using`, `pub using`, and
  formatted results from imported code.
