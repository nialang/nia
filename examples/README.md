# Nia Examples

The examples are small, complete programs that use the current language and
standard library together. They are organized by the kind of program a reader
is trying to understand, not by the date a feature was added. No example is an
exhaustive language test; the specification and the compiler test suites define
the complete surface.

Every executable uses the same entry point:

```nia
pub fn main(process::Init) process::ExitCode!()
```

The examples use `std::io::debugPrint` for compact diagnostic output. `io.nia`
shows the explicit stdout path. Errors are propagated with `.?`, and mutable
resources are released with `defer` in the order required by their owners.
`collections.nia` shows the usual ownership arrangement: containers borrow a
general-purpose allocator and release their storage before the allocator is
deinitialized.

## Reading Order

1. `hello.nia` introduces an executable and the standard entry contract.
2. `basics.nia` combines functions, structs, methods, enums, `match`, ranges,
   slices, loops, and `defer` in one small calculation.
3. `data.nia` develops arrays, slice views, mutable slices, text and byte
   literals, `const`, and static storage.
4. `errors.nia` uses optional values, error unions, pattern matching, and
   propagation to handle a missing element.
5. `generic.nia` combines trait implementations, operator overloading, generic
   bounds, and borrowed iterator closures.
6. `collections.nia` puts explicit allocator ownership together with
   `ArrayList`, `HashMap`, mutation, lookup, and cleanup.
7. `io.nia` writes formatted application output through `std::io`.
8. `process.nia` starts a child process with typed arguments, environment
   entries, a piped stdout handle, and a termination status.
9. `modules/main.nia` is a multi-file program with public modules, aliases,
   selected imports, and a facade that re-exports an API.

Run one example from the repository root:

```sh
cargo run -p nia-cli -- --resource-root lib check examples/hello.nia --runtime freestanding
cargo run -p nia-cli -- --resource-root lib emit --exe examples/io.nia -o build/nia-io
```

Check the complete example set:

```sh
for file in examples/*.nia; do
  cargo run -p nia-cli -- --resource-root lib check "$file" --runtime freestanding
done
cargo run -p nia-cli -- --resource-root lib check examples/modules/main.nia --runtime freestanding
```

The examples intentionally favor explicit ownership and ordinary library APIs.
They are executable reading material, while edge cases and rejected forms
belong in focused compiler and standard-library tests.
