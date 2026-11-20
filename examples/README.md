# Nia Examples

Small, complete programs that use the language and the standard library
together. Each one builds into an executable and prints a few lines that show
what it computed. They are meant to be read in order. The
[Learning Nia](../docs/learn/README.md) chapters explain the same features in
prose, and the [Language Reference](../docs/reference/language.md) has the
complete rules.

| Example | Shows |
| --- | --- |
| [`hello.nia`](hello.nia) | The standard entry point and one line of output |
| [`basics.nia`](basics.nia) | Functions, structs and methods, enums and `match`, loops, `defer` |
| [`data.nia`](data.nia) | Arrays, slices, text and byte literals, `const`, `static` |
| [`errors.nia`](errors.nia) | Optional values, error unions, `.?` propagation, `IntoError` |
| [`traits.nia`](traits.nia) | Generics, traits and default methods, operator traits, trait objects |
| [`closures.nia`](closures.nia) | Captures, callable views, function pointers, parameter inference, iterator adapters |
| [`collections.nia`](collections.nia) | Allocators, `ArrayList`, `HashMap`, `TreeMap`, `String`, cleanup order |
| [`io.nia`](io.nia) | Buffered stdout and format specifications |
| [`process.nia`](process.nia) | A child process with arguments, an environment, and a pipe (POSIX hosts) |
| [`modules/main.nia`](modules/main.nia) | A multi-file program with a re-exporting facade |

Build and run one from the repository root:

```sh
nia emit --exe examples/basics.nia -o basics
./basics
```

```text
distance=41 total=211 evens=5 level=1 high=1
defer order=3 2 1
```

A multi-file example is built from its root file, such as
`examples/modules/main.nia`. From a source checkout, use `tools/nia-dev` in
place of `nia`, or pass `--resource-root lib` to a freshly built
`target/release/nia`.

The compiler's test suite runs every example and compares its output with
the expected text, so the programs stay current with the language. On Windows
it only checks `process.nia`, which needs a POSIX shell.
