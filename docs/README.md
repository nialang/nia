# Nia Documentation

## Learning Nia

New to Nia? Start with the chapters in [Learning Nia](learn/README.md). They
teach the language in order, from installing it to testing a package, and every
program in them builds and runs.

| Chapter | Covers |
| --- | --- |
| [Getting Started](learn/getting-started.md) | Installing Nia, a first program, reading errors, a first package |
| [Values, Types, And Functions](learn/basics.md) | Bindings, numbers, functions, blocks, loops, `defer` |
| [Structs, Enums, And Patterns](learn/structs-and-enums.md) | Your own types, methods, `match`, patterns |
| [Pointers, Arrays, And Slices](learn/pointers-and-slices.md) | Pointers, automatic addresses, arrays, slices, text, valid pointers |
| [Optionals And Errors](learn/errors.md) | `?T`, `E!T`, propagation, conversion, cleanup on failure |
| [Memory And Allocators](learn/memory.md) | Allocators, containers, owners, leaks |
| [Generics And Traits](learn/generics-and-traits.md) | Generic code, bounds, traits, operators, iterators, trait objects |
| [Closures](learn/closures.md) | Captures, callable views, function pointers |
| [Modules And Visibility](learn/modules.md) | Modules, `using`, visibility, extension methods, facades |
| [Testing](learn/testing.md) | Test suites, assertions, reporting failures, test-only code |

The [examples](../examples/README.md) are complete programs to read alongside
the chapters.

## Guides

Guides cover a task from start to finish.

| Guide | Read it for |
| --- | --- |
| [Packages And Dependencies](guides/packages.md) | Creating packages, building and testing them, dependencies, the lockfile, workspaces |
| [Custom Builds With `build.nia`](guides/build-scripts.md) | Generated files, external commands, extra artifacts and steps, configuration probes |
| [Targets And Runtimes](guides/targets.md) | Cross-compiling, platform-specific code, build profiles, programs without an operating system |
| [C Interop](guides/c-interop.md) | Calling C, linking native libraries, and exporting Nia functions |

## Reference

References state the exact rules.

| Reference | Covers |
| --- | --- |
| [Language](reference/language.md) | Syntax and semantics of the language |
| [Built-in operations](reference/builtins.md) | `std::builtin` intrinsics, SIMD, atomics, and inline assembly |
| [Standard library](../lib/README.md) | Library modules and their conventions |
| [Manifest](reference/manifest.md) | Every `nia.toml` table and key |
| [Command line](reference/cli.md) | `nia` commands, options, exit statuses, and build output formats |
| [ABI](reference/abi.md) | Data representation, layout, and C boundary rules |
| [Platform support](reference/platform-support.md) | Targets, target facts, runtime model, and current limits |

`nia help <command>` lists every option of a command.

## Developing Nia

| Document | Covers |
| --- | --- |
| [Contributing](dev/contributing.md) | Project policy, change routing, code standards, and review |
| [Development setup](dev/setup.md) | Rust, LLVM, and LLD setup on Linux and Windows |
| [Testing](dev/testing.md) | Validation gates, test layout, and harness rules |
| [Compiler architecture](dev/architecture.md) | Pipeline, query model, crate layers, and data flows |
| [ABI internals](dev/abi-internals.md) | How the compiler implements the ABI |
| [Closure inference](../crates/nia-body-check/README.md) | How the checker infers closures passed to generic calls |
| [Build system internals](../crates/nia-build/README.md) | Build plans, caching, execution, progress observation, and publication |
| [Package metadata](../crates/nia-package-metadata/README.md) | Version selection, registry index and archive formats, `nia.lock`, and source identity |
| [Native Git client](../crates/nia-git/README.md) | Security model, enforced limits, and known gaps of `nia-git` |
| [Formal models](../formal/README.md) | Lean models of compiler contracts and how they are tied to compiler tests |
| [Repository maintenance](../maintain/README.md) | Audits, reports, and [performance baselines](../maintain/performance.md) |
| [Release build](../tools/release/README.md) | Building and packaging release archives |

Each crate documents its own responsibilities in its `//!` crate docs
(`cargo doc --workspace --no-deps`).
