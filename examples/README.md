# Nia Examples

Small executable Nia programs for the current compiler and standard library.
The source files are intended to be read as tutorials, not just copied as smoke
tests: inline comments call out Nia-specific syntax, ownership rules, runtime
entry points, and standard-library idioms.

Each top-level `.nia` file uses the current entry contract:
`pub fn main(process::Init) process::ExitCode!void`.

Most examples print their results through `std::debug::print`, a stderr debug
printing helper for small programs and diagnostics. It keeps example programs
focused on the language feature being shown; if the underlying stderr write or
flush fails, it traps instead of silently ignoring the failure.

`03_stdout.nia` shows the explicit application-output path with
`std::io.FileWriter` and `std::fmt`: create a stdout buffer, write formatted text,
flush the writer, and map I/O error unions into process exit codes with
`io_call().exit().?`. Format arguments are passed as a slice of trait-object
handles, usually written as `&[&value, &count]`; Nia converts the array pointer
to the expected slice.

`std::fmt` placeholders are positional. Use `{}` for display formatting and
`{:...}` for format options: alignment and fill (`{:>5}`, `{:_>5}`), text
precision (`{:.3}`), dynamic width and precision from following `usize`
arguments (`{:<{}}`, `{:>{}.{}}`), integer bases (`{:x}`, `{:X}`, `{:b}`,
`{:o}`), signs (`{:+}`), alternate prefixes (`{:#x}`), zero padding (`{:05}`), and pointer
addresses (`{:p}`). Literal braces are written as `{{` and `}}`.

Value parsing is independent of formatting. Import `std::parse` and use
`parse::value[T](input)` or `parse::radix[T](input, radix)` for primitive
integers and bools. Ordinary parsing uses `parse::From[Input]`; explicit-radix
integer parsing uses `parse::FromRadix[Input]`. Both protocols have an
associated error type, so user-defined values can retain domain-specific
parse failures. Character text, byte text, C-string
views, and process argument/environment views share the same entry points.
Integer parsing accepts prefixes such as `0x`, `0b`, and `0o`; radix parsing is
for bare digits in an explicit radix.
Process arguments and environment entries are C-string-backed views; use
`arg.bytes()` for raw argument bytes, `arg.cstring()` for the underlying
`std::CStringView`, parse the argument directly with `parse::value[T](arg)`, or
format the argument value directly with `{}`.
Use `init.args().program()` for argv[0], `for arg in init.args().skipProgram()`
for application arguments, and `for env in init.env().iter()` for environment
traversal.
`10_process_command.nia` shows the typed child-process path: construct a
`Command` from a `PathView` and `Env`, pass scalar arguments with
`withArguments`, replace the inherited environment with borrowed scalar
`EnvEntry` values through `withEnvironment`, take the role-specific
`ChildStdout`, read it directly as an `io::Reader`, and inspect the returned
`Term`.
The command inserts its path as argv[0] and performs UTF-8/C argv and envp
lowering only during `spawn` or `run`; ordinary callers do not build
`CStringView` values or raw pointer arrays. `withoutEnvironment` selects an
explicitly empty envp.

Run an example from the repository root with:

```sh
cargo run -p nia-cli -- check examples/00_minimal.nia --runtime freestanding
cargo run -p nia-cli -- emit --llvm examples/00_minimal.nia
```

Check all examples from the repository root with:

```sh
for file in examples/*.nia; do cargo run -p nia-cli -- check "$file" --runtime freestanding; done
cargo run -p nia-cli -- check examples/modules/main.nia --runtime freestanding
```

The CLI test suite parses every repository example and freestanding-checks
representative minimal, collection-heavy, and multi-module examples. Before a
release, run the full loop above as a manual gate so every example is checked
against the executable runtime.

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
The usual pattern is to create a concrete allocator and bind its pointer handle,
such as `let allocator = &mut gpa;`. You can also take the handle at creation
time, for example `let gpa = &mut mem::GeneralPurposeAllocator::init(&mut page);`.
The `let` fixes the handle itself, while operations through the handle can still
mutate the allocator state. Every container operation that may allocate or free
should receive that same handle.

- Use `std::mem.FixedBufferAllocator` when storage is caller-provided and bounded.
- Use `std::mem.ArenaAllocator` for phase-local allocations that can all be
  invalidated together with `reset` or `deinit`.
- Use `std::mem.GeneralPurposeAllocator` for ordinary heap-backed containers and
  mixed allocation lifetimes. Its `deinit` returns `mem::DeinitStatus::Leak`
  when allocations are still live, so examples that expect clean shutdown check
  it with `deinit().ok().?`.
- Use `std::mem.PageAllocator` as low-level backing storage for other
  allocators, or when whole page mappings are exactly what the program wants.

## Reading Order

- `00_minimal.nia`: the smallest debug-printing executable entry.
- `01_values_control_flow.nia`: structs, methods, slices, ranges, `switch`,
  `defer`, debug printing, and exit-code checks.
- `02_slices_and_strings.nia`: arrays, slices, byte strings, string pointers,
  mutable slice writes, and count-returning `copyFrom`.
- `03_stdout.nia`: standard-library stdout through `std::io` and `std::fmt`.
- `04_array_list.nia`: `std::ArrayList` with `std::mem.FixedBufferAllocator`,
  slice iteration, and formatted list output.
- `05_traits_generics.nia`: traits, generic bounds, operator traits, and a
  formatted computed total.
- `06_optional_error.nia`: checked slice access, `if ... is` optional matching,
  error unions, propagation, and printed success/error state.
- `07_arena_allocator.nia`: `std::mem.ArenaAllocator` over `PageAllocator`,
  `ArrayList` allocation, retained-capacity reset, and scratch slices.
- `08_general_purpose_allocator.nia`: `std::mem.GeneralPurposeAllocator` over
  `PageAllocator`, ordinary heap allocation, `ArrayList`, and cleanup with
  `defer`.
- `09_hash_map.nia`: `std::HashMap` with explicit allocator ownership,
  `getOrInsert` entry slots, mutable entry/value iteration, removal, and
  formatted output.
- `10_process_command.nia`: typed child-process paths, scalar arguments and
  exact environment entries, inherited stdio, and termination status handling.
- `modules/main.nia`: file modules, aliases, selected `using`, `pub using`, and
  formatted results from imported code.
