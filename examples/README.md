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

## Allocators

Nia allocation is explicit. Containers receive `&mut mem::Allocator`, and the
same allocator must later deinitialize or free the allocation.

- Use `std.mem.FixedBufferAllocator` when storage is caller-provided and bounded.
- Use `std.mem.ArenaAllocator` for phase-local allocations that can all be
  invalidated together with `reset` or `deinit`.
- Use `std.mem.GeneralPurposeAllocator` for ordinary heap-backed containers and
  mixed allocation lifetimes. Its `deinit` returns `mem::DeinitStatus::Leak`
  when allocations are still live, so examples that expect clean shutdown check
  it with `.ok().?`.
- Use `std.mem.PageAllocator` as low-level backing storage for other
  allocators, or when whole page mappings are exactly what the program wants.

## Reading Order

- `00_minimal.nia`: the smallest debug-printing executable entry.
- `01_values_control_flow.nia`: structs, methods, slices, ranges, `switch`,
  `defer`, debug printing, and exit-code checks.
- `02_slices_and_strings.nia`: arrays, slices, byte strings, C strings, and
  mutable slice writes.
- `03_stdout.nia`: standard-library stdout through `std.io` and `std.fmt`.
- `04_array_list.nia`: `std::ArrayList` with `std.mem.FixedBufferAllocator`,
  slice iteration, and formatted list output.
- `05_traits_generics.nia`: traits, generic bounds, operator traits, and a
  formatted computed total.
- `06_optional_error.nia`: optional values, error unions, propagation, and
  printed success/error state.
- `07_arena_allocator.nia`: `std.mem.ArenaAllocator` over `PageAllocator`,
  `ArrayList` allocation, retained-capacity reset, and scratch slices.
- `08_general_purpose_allocator.nia`: `std.mem.GeneralPurposeAllocator` over
  `PageAllocator`, ordinary heap allocation, `ArrayList`, and cleanup with
  `defer`.
- `modules/main.nia`: file modules, aliases, selected `using`, `pub using`, and
  formatted results from imported code.
