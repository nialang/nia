# paw

`paw` is the Nia package builder and package-manager workbench. It is written in
Nia and lives under `tools/paw` while the package workflow is still being
designed.

The current implementation is intentionally small: a package root has a
`build.nia` file, `paw` runs that file to produce a build graph, then it asks the
Nia compiler C API to check, build, or emit package artifacts.

## Layout

```text
tools/paw/
  build.nia          package build script for paw itself
  src/main.nia       executable entry point
  src/root.nia       package root, similar to a library root
  src/root/*.nia     paw implementation modules
```

`main.nia` is only the process entry point. Shared implementation belongs under
`src/root.nia` and `src/root/`.

## Build Script

A package build script exports `build` and uses the `paw` build API:

```nia
using paw;

pub fn build(build: &mut paw::Build) paw::Error!void {
    build.package({ name: "paw", version: "0.1.0" }).?;
    var exe = build.executable({ name: "paw", root: "src/main.nia" }).?;
    exe.library_path("target/debug/deps").?;
    exe.library("nia_capi").?;
    exe.rpath("target/debug/deps").?;
    exe.dynamic_linker_auto()
}
```

Link options belong to the executable handle. They are not global package
options. The current graph format accepts exactly one package and one
executable; duplicate package entries, multiple executables, and link options
that reference an unknown artifact are rejected.

## Commands

```text
paw check <root>
paw build <root> <out>
paw objects <root> <out-dir>
```

- `check` checks the package executable described by `<root>/build.nia`.
- `build` emits and links the package executable to `<out>`.
- `objects` emits the package's object files into `<out-dir>`, one file per
  codegen unit.

## Bootstrapping

From the repository root:

```sh
cargo build -p nia-capi
cargo run -q -p nia-cli -- emit --exe tools/paw/src/main.nia \
  -M paw=tools/paw/src/root.nia \
  -L target/debug/deps -l nia_capi \
  --rpath target/debug/deps --dynamic-linker auto \
  -o /tmp/paw-current
```

Then use the emitted `paw`:

```sh
/tmp/paw-current check tools/paw
/tmp/paw-current build tools/paw /tmp/paw-self
/tmp/paw-current objects tools/paw /tmp/paw-objects
```

A basic self-hosting check is:

```sh
/tmp/paw-current build tools/paw /tmp/paw-self
/tmp/paw-self build tools/paw /tmp/paw-self2
```

## Current Limits

`paw` is not a complete package manager yet. The current implementation has:

- one package and one executable per build graph;
- no dependency resolver;
- no package cache;
- no install or publish workflow;
- no dedicated test command.

Those limits are represented directly in the implementation instead of being
papered over with compatibility shims.
