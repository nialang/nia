# Nia Documentation

## Using Nia

Start with the guide, then use the reference when you need exact rules.

| Document | Read it for |
| --- | --- |
| [Getting started](guide/getting-started.md) | Installing Nia, a first program, and the `nia` commands |
| [Language tour](guide/tour.md) | The language from simple values to traits, closures, and modules |
| [Build scripts](guide/build-scripts.md) | `nia.toml` manifests, dependencies, and workspaces; writing `build.nia`: modules, executables, steps, and test suites |
| [C interop](guide/c-interop.md) | Calling C, linking native libraries, and exporting Nia functions |
| [Examples](../examples/README.md) | Complete programs arranged as reading material |

## Reference

| Document | Covers |
| --- | --- |
| [Language reference](reference/language.md) | Syntax and semantics of the language |
| [Built-in operations](reference/builtins.md) | `std::builtin` intrinsics, SIMD, atomics, and inline assembly |
| [Standard library](../lib/README.md) | Library modules and their ownership conventions |
| [ABI reference](reference/abi.md) | Data representation, layout, and C boundary rules |
| [Platform support](reference/platform-support.md) | Targets, runtime model, and current limits |

The command-line help (`nia help`, `nia help <command>`, and
`nia help emit --<kind>`) is the reference for command options.

## Developing Nia

| Document | Covers |
| --- | --- |
| [Contributing](dev/contributing.md) | Project policy, change routing, code standards, and review |
| [Development setup](dev/setup.md) | Rust, LLVM, and LLD setup on Linux and Windows |
| [Testing](dev/testing.md) | Validation gates, test layout, and harness rules |
| [Compiler architecture](dev/architecture.md) | Pipeline, query model, crate layers, and data flows |
| [ABI internals](dev/abi-internals.md) | How the compiler implements the ABI |
| [Closure inference](dev/closure-inference.md) | Generic callable closure inference specification |
| [Build system internals](../crates/nia-build/README.md) | Build plans, caching, execution, progress observation, and publication |
| [Package metadata](../crates/nia-package-metadata/README.md) | Version selection, registry index and archive formats, `nia.lock`, and source identity |
| [Native Git client](../crates/nia-git/README.md) | Security model, enforced limits, and known gaps of `nia-git` |
| [Repository maintenance](../maintain/README.md) | Audits, reports, and [performance baselines](../maintain/performance.md) |
| [Release build](../tools/release/README.md) | Building and packaging release archives |

Historical documents and temporary working notes are archived in `dev/archive/`.

Each crate documents its own responsibilities in its `//!` crate docs
(`cargo doc --workspace --no-deps`).
