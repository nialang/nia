# Nia Examples

Small, complete programs that use the language and the standard library
together. Each one builds into an executable and prints a few lines that show
what it computed. They are meant to be read in order; the
[Learning Nia](../docs/learn/README.md) chapters explain the same features in prose,
and the [language reference](../docs/reference/language.md) has the complete
rules.

| Example | Shows |
| --- | --- |
| [`hello.nia`](hello.nia) | The standard entry point and printing |
| [`basics.nia`](basics.nia) | Functions, structs and methods, enums and `match`, loops, `defer` |
| [`data.nia`](data.nia) | Arrays, slices, text and byte literals, `const`, `static` |
| [`errors.nia`](errors.nia) | Optional values, error unions, `.?` propagation, `IntoError` |
| [`traits.nia`](traits.nia) | Generics, traits and default methods, operator traits, trait objects |
| [`closures.nia`](closures.nia) | Captures, callable views, function pointers, parameter inference, iterators |
| [`collections.nia`](collections.nia) | Allocators, `ArrayList`, `HashMap`, `TreeMap`, `String`, cleanup order |
| [`io.nia`](io.nia) | Buffered stdout and format specifications |
| [`process.nia`](process.nia) | Child processes with arguments, environment, and a pipe (POSIX hosts) |
| [`modules/main.nia`](modules/main.nia) | A multi-file program with a re-exporting facade |

Build and run one from the repository root:

```sh
nia emit --exe examples/basics.nia -o basics
./basics
```

From a source checkout, use `tools/nia-dev` in place of `nia`, or pass
`--resource-root lib` to a freshly built `target/release/nia`. A multi-file
example is built from its root file, such as `examples/modules/main.nia`.

Each example's expected output is checked by the compiler's test suite, so the
programs stay current with the language.
