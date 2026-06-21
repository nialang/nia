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
    exe.library_path("../../target/release/deps").?;
    exe.library("nia_capi").?;
    exe.rpath("../../target/release/deps").?;
    exe.dynamic_linker_auto()
}
```

Link options belong to the executable handle. They are not global package
options. The current graph format accepts exactly one package and one
executable; duplicate package entries, multiple executables, and link options
that reference an unknown artifact are rejected. Relative paths in `build.nia`
are resolved from the package root.

## Commands

```text
paw check [root]
paw build [root]
paw objects [root]
```

- `check` checks the package executable described by `build.nia`.
- `build` emits and links the package executable into `<root>/.nia-build/`.
- `objects` emits the package's object files into
  `<root>/.nia-build/objects`, one file per codegen unit.

If `root` is omitted, `paw` uses the current directory.

## Bootstrapping

`paw` is self-hosted through one compiler-built seed binary. From the repository
root, first build the release compiler, release C API library, and create a
local bootstrap directory:

```sh
cargo build --release -p nia-cli -p nia-capi
mkdir -p build/paw-bootstrap
```

Then build the seed `paw` with `nia`:

```sh
target/release/nia emit --exe tools/paw/src/main.nia \
  -M paw=tools/paw/src/root.nia \
  -L target/release/deps -l nia_capi \
  --rpath target/release/deps --dynamic-linker auto \
  -o build/paw-bootstrap/paw-step1
```

Use the seed to build the normal `paw` binary:

```sh
build/paw-bootstrap/paw-step1 check tools/paw
build/paw-bootstrap/paw-step1 build tools/paw
cp tools/paw/.nia-build/paw build/paw-bootstrap/paw
```

A basic self-hosting check builds one more generation and verifies object output:

```sh
build/paw-bootstrap/paw build tools/paw
cp tools/paw/.nia-build/paw build/paw-bootstrap/paw-step2
build/paw-bootstrap/paw objects tools/paw
```

The `build/` directory is ignored by this repository and is used here for
bootstrap binaries. Package build outputs go under `.nia-build/` inside the
package root. Future package builds should keep reusable compiler or package
cache entries separate from those build outputs; `.nia-cache/` is the likely
shape for that cache, but `paw` does not own it yet.

## Current Limits

`paw` is not a complete package manager yet. The current implementation has:

- one package and one executable per build graph;
- no dependency resolver;
- no package cache;
- no install or publish workflow;
- no dedicated test command.

Those limits are represented directly in the implementation instead of being
papered over with compatibility shims.
