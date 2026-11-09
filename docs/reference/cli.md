# Command-Line Reference

The `nia` command creates packages, builds them, and runs the compiler
directly on source files. This page describes each command, the options they
share, and the output formats of `nia build`. `nia help <command>` lists every
option of a command, and `nia help emit --<kind>` those of one emit kind.

## Commands

| Command | What it does |
| --- | --- |
| `nia new <path>` | Creates a package or workspace from a template; see [Packages](../guides/packages.md#creating-a-package). |
| `nia init [path]` | Creates the same template in an existing directory without replacing files. |
| `nia add <name>` | Adds a path, Git, or registry dependency to the package manifest. |
| `nia build [step]` | Builds the package: runs the named step, or the default step. |
| `nia update [name ...]` | Refreshes Git revisions and registry versions in `nia.lock`. |
| `nia clean` | Removes build outputs and caches, keeping fetched dependencies. |
| `nia check <path>` | Type-checks a source file or a library package without producing output. |
| `nia emit --<kind> <path>` | Prints an intermediate form of a program, or writes object files or an executable. |
| `nia help [command]` | Shows the options of a command. |

`nia build`, `nia update`, `nia clean`, and `nia add` search upward from the
current directory for the package or workspace, or start at `--root <dir>`.

## Options

These options apply to every command and may appear before or after it:

| Option | Effect |
| --- | --- |
| `-O0` … `-O3`, `-Os`, `-Oz`, `-O` | Optimization level; `-O` means `-O2`. |
| `--debug`, `--release`, `--profile <debug\|release>` | Build profile; the default optimization is `-O0` for `debug` and `-O2` for `release`. |
| `--target <name>` | Generate code for a [supported target](platform-support.md#targets) instead of the host. |
| `--diagnostics-format <text\|json>` | Human-readable or machine-readable diagnostics. |
| `--timings[=summary\|detail]`, `--timings-format <text\|json>`, `--timing-trace <off\|events>` | Compiler timing reports. |
| `--resource-root <path>` | Use the standard library and runtime from this toolchain directory. |
| `-V`, `--version` | Print the compiler version. |

Every option that takes a value accepts both `--option value` and
`--option=value`, and the next argument is always the value, even when it
starts with `-`. When an option is repeated, the last value wins; `-M`, `-p`,
`-L`, `-l`, `--link-arg`, and `--rpath` accumulate instead. `--` ends the
options, so later arguments are taken literally; `nia build` passes them to a
run step's program instead (see [`nia build`](#nia-build)).

| Exit status | Meaning |
| --- | --- |
| `0` | success |
| `1` | a failed check, build, or test |
| `2` | a malformed command line |

## `nia new` And `nia init`

```text
nia new <path> [--lib | --exe] [--name <name>]
nia new <path> --workspace [--member <name> ...]
nia init [path] [same options]
```

`nia new` creates the directory and fails if it already exists. `nia init`
fills an existing directory, the current one by default; it keeps an existing
`.gitignore` and fails, without writing anything, when the directory already
has a `nia.toml` or any other file the template would create.

| Option | Effect |
| --- | --- |
| `--lib` | a library and its tests, without an executable |
| `--exe` | an executable and its tests, without a library |
| `--name <name>` | the package name, instead of the directory name; for a workspace, the name of its first member |
| `--workspace` | a workspace root with members under `packages/` |
| `--member <name>` | a workspace member; repeat it for several. Without it, one member named after the workspace |

Run inside a workspace, either command creates a member: it inherits the
workspace's `[workspace.package]` fields, and its path is added to `members`
unless a member pattern already covers it.

## `nia add`

```text
nia add <name> --path <dir>
nia add <name> --git <url> [--rev <commit> | --tag <tag> | --branch <branch>]
nia add <name> --registry <registry> --package <namespace/name> --version <requirement>
```

Adds one dependency to the manifest of the package found from the current
directory. `--scope <runtime|dev|build|optional|lazy>` picks the dependency
table, `[dependencies]` by default. At a virtual workspace root, `--root <dir>`
selects the member whose manifest is edited. A path dependency must already be
a valid package. The [manifest reference](manifest.md#dependency-tables)
describes each source.

## `nia update` And `nia clean`

`nia update` re-resolves every Git selector and registry requirement and
writes the results to `nia.lock`. With names, `nia update json regex`, only
those dependencies are refreshed; each must be a locked Git or registry
dependency. It reports Git revision changes and registry version changes
separately.

`nia clean` removes `build/out/`, `build/cache/`, and `build/tmp/`, keeping
fetched dependencies. Run inside a workspace member, it removes that member's
outputs and the workspace's shared caches. It fails outside a package.

## `nia check` And `nia emit`

`nia check` and `nia emit` run the compiler on one module tree without a build
graph. The path is a source file, or a directory that holds a `main.nia` or a
`nia.toml` with a `[library]` path; that file roots the tree, and its child
modules are found beside it.

```sh
nia check lib/root.nia
nia emit --llvm src/main.nia
nia emit --exe src/main.nia -o hello
```

They do not read the dependencies in `nia.toml`. When the source file lies
inside a package that declares a `[library]`, the package's own name opens
that library, as it does under `nia build`; any other package name the code
imports is mapped with `-M <name>=<path>`, where `<path>` is a source file or
a package directory:

```sh
nia check src/main.nia --runtime freestanding
nia check src/main.nia --runtime freestanding -M json=../json
```

Inside a package, `nia build` is usually simpler: it checks and builds every
artifact with its dependencies.

| Option | Effect |
| --- | --- |
| `--runtime <bare\|freestanding>` | `bare` (the default) checks every function as an ordinary function; `freestanding` also checks the standard `main` entry. `--exe` always builds `freestanding`. |
| `-M, --module <name=path>` | Map a package name for `using`; repeatable. |
| `--cache-dir <path>` | Reuse compiler products from this directory. |
| `--opt-report` | Print the optimization report to stderr. |

`emit` takes one kind:

| Kind | Output |
| --- | --- |
| `--tokens`, `--ast`, `--checked`, `--backend`, `--llvm` | prints that form of the program |
| `--obj` | writes native object files; `-o` for a single object, `--out-dir` when there are several |
| `--exe` | writes a freestanding executable to `-o <file>`, or without `-o` next to the source file, as `main` (`main.exe` on Windows) for `main.nia` |

`--exe` also accepts the linking options: `-L <dir>` and `-l <name>` for native
libraries, `--link-arg <arg>`, `--lto <off|thin|full>`, `--linker <program>`
and `--linker-flavor <flavor>` to replace LLD, and on Linux
`--executable <static-pie|static|dynamic>`, `--interpreter <path>`, and
`--rpath <path>`. [C interop](../guides/c-interop.md#linking-a-c-library)
shows them in use.

An executable is compiled as several codegen units, which build in parallel
and are cached separately. Without `--lto`, an executable built at `-O1` or
above links them with ThinLTO, so a call into another unit can still be
inlined; at `-O0` the units are linked as they are. `--lto off` links them
without cross-unit optimization at any level, and `--lto full` merges them into
one unit before optimizing, which takes longer to build and can produce smaller
output. Build scripts choose the same way with `withLto`; an artifact that
does not call it follows this rule.

## `nia build`

```text
nia build [step] [options] [-- <program-args>...]
```

`nia build` runs the named step, or the package's default step, after every
step it depends on. Steps come from the manifest and from `build.nia`;
`nia build --list` prints them. Outputs go to `build/out/`, as described in
[The Build Directory](#the-build-directory). When a run step's program exits
with a nonzero status, `nia build` reports the step as failed and exits with
status 1.

Arguments after `--` are passed to the program of the selected run step,
after the arguments its build script configures:

```sh
nia build run-greet -- --verbose "two words"
```

The step must be named before `--` and must be a run step: `nia build -- run-greet`
names no step, and `nia build unit -- x` names a test suite; both are errors.
The arguments are not part of the configuration. `build.nia` does not see
them, and changing them reuses the configured plan and the compiled program.

A step name consists of ASCII letters, digits, `-`, `_`, and `.`. A public
step name, one that `nia build` can select, must also not start with `-` or be
`.` or `..`, so that it cannot be read as an option or a path, and must not
start with `nia-internal-test-compile.`, which names the steps that compile
test suites. Manifest artifact names follow the same rule, because each one
names a step.

| Option | Effect |
| --- | --- |
| `--root <dir>` | starts package discovery at `<dir>` |
| `-p, --package <name>` | selects a workspace member; repeatable |
| `--workspace` | selects every workspace member |
| `--locked` | fails instead of changing `nia.lock` |
| `--offline` | uses cached Git checkouts, registry indexes, and registry archives only |
| `-k, --keep-going` | after a failure, keeps running the actions that do not depend on it |
| `-j, --jobs <count>` | limits concurrent actions |
| `--lto <off\|thin\|full>` | selects link-time optimization for the build runner, which is built at `-O0`; artifacts use `withLto` |
| `-q`, `-v` … `-vvv`, `--progress <mode>` | progress output; see [Progress Output](#progress-output) |

These options inspect the build instead of, or in addition to, running it:

| Option | Effect |
| --- | --- |
| `--list` | prints every step and its kind without running any |
| `--dry-run` | configures and validates the graph, and predicts each action's cache decision (`hit`, `miss`, or `unavailable` when the action's inputs do not exist yet) without running build actions |
| `--explain` | shows the selected step, targets, configuration, dependencies and whether each was loaded and used, observations, tools, the action closure, and every cache decision with its invalidated components |
| `--print-plan` | prints the frozen graph and selection context, including module imports, artifact outputs, action outputs, and step dependencies |
| `--print-cache` | prints configure and action cache hits and misses; with `--dry-run`, reports predicted outcomes without restoring outputs or publishing records |
| `--print-observations` | prints the facts recorded by configuration probes |

Text cache records show the outcome and reason, publication status, declared
outputs, and any invalidated identity components. With
`--diagnostics-format=json`, these products are JSON records with the same
information.

## The Build Directory

Everything Nia writes lives in one `build/` directory beside the root
`nia.toml`, the workspace root in a workspace:

| Path | Contents |
| --- | --- |
| `build/out/` | build outputs; `build/out/<member path>/` for a workspace member |
| `build/cache/` | reusable results: compiled build scripts, configurations, compiler products, command results |
| `build/tmp/` | files of builds in progress |
| `build/packages/` | fetched Git checkouts and registry packages |
| `build/registry/` | downloaded registry indexes and archives |
| `build/packages.lock` | a lock that keeps concurrent `nia` commands from changing fetched packages at the same time; not the dependency lockfile `nia.lock` |

`nia clean` removes the first three and keeps the rest.

## Progress Output

`nia build` reports progress on stderr. The products you ask for
(`--print-plan`, `--print-cache`, `--print-observations`, `--explain`,
`--list`) go to stdout, as does the standard output of external
commands and test suites in text modes. A failed command's report names its
exit status; it repeats the command's last output only with
`--diagnostics-format=json`, where that output was captured instead of shown.

Each action the selected steps need is counted once, and ends with one outcome:

| Outcome | Meaning |
| --- | --- |
| cached | the result was reused from `build/cache/` |
| executed | the action ran |
| no-work | the step has nothing of its own to do, such as an aggregate |
| failed | the action failed |
| cancelled | the action did not run because another one failed |

The build finishes with a summary such as
`Finished 3 actions in 982ms (1 cached, 1 executed, 1 no-work)`, with failed
and cancelled counts added when there are any. A `--dry-run` or
`--list` ends with `Configured build in ... (N selected actions)`.

A run step, or any external command that declares no outputs, gets the
terminal instead: the summary of what finished so far comes before it,
followed by `Running root:run`, and nothing is printed after the program
unless the build fails. `nia build run` therefore ends with the program's own
output, as `cargo run` does. From `-v`, progress keeps reporting every action
and closes with the full summary.

Repeat `-v` for more detail. Each level includes the one before it:

| Level | Text output |
| --- | --- |
| `-q`, `--quiet` | no progress; diagnostics and requested products remain |
| default | the summary, or the summary and `Running` line before a run step |
| `-v` | whether the build runner and plan were reused, then one line per action in plan order, such as `[1/3] Cached root:app (compiler-emit, 699ms)`; a no-work action is shown as `Complete` |
| `-vv` | waves of ready actions, each action's start with its queue and resource-permit wait, waits for dependencies, locks, and resource permits, and resource-permit usage |
| `-vvv` | the cache decision on each completion line, such as `; cache miss: not found`, and the compiler's checking, code generation, and linking activities |

`-q` cannot be combined with `-v`, and the level stops at `-vvv`.

`--progress <mode>` (or `--progress=<mode>`) selects the output form:

| Mode | Output |
| --- | --- |
| `auto` (default) | a live status that redraws in place, when stderr is a terminal, `TERM` is not `dumb`, and `CI` is unset; plain text otherwise. With `--diagnostics-format=json`, no progress |
| `plain` | append-only text lines |
| `json` | JSON Lines, described below |
| `none` | no progress |

The live status is one line, `Building [completed/total actions, n active]`
followed by the first active action and the elapsed time. From `-vv` it becomes
a panel of up to eight rows: the wave and counts, resource-permit usage with
the number of active, queued, and dependency-blocked actions, then the pending
actions in plan order with their waits. It first appears after 250ms and
redraws at most every 100ms, so short builds print only the summary.

### JSON Progress

`--progress=json` makes stderr entirely machine-readable: diagnostics default
to JSON, and timings to JSON when `--timings` is on. Combining it with an
explicit text format, or with `-q`, is an error, as is `--progress=plain` with
`--diagnostics-format=json`. External command output is captured rather than
forwarded.

Every line is one object with `"kind": "build-progress"`, `"schema_version": 1`,
`event`, `sequence`, and `elapsed_ms`. `sequence` increases but may skip
numbers. Actions are written as `{"package": ..., "name": ...}`, and an action's
`ordinal` is its 1-based position in plan order.

| Event | Level | Fields |
| --- | --- | --- |
| `build-started` | default | none |
| `build-finished`, `build-failed` | default | `completed`, `total` (omitted when the build failed before selecting actions) |
| `phase-started`, `phase-finished` | default | `phase`: `resolve`, `prepare`, `runner`, `configure`, `preview`, or `execute`; finish adds `duration_ms`, `success` |
| `selection-ready` | default | `total`, `actions` in plan order |
| `action-finished` | default | `action`, `ordinal`, `action_kind`, `outcome`, `cache` (for cacheable actions), `duration_ms`, `completed`, `total` |
| `phase-cache-decision` | `-v` | `phase`, `reused` |
| `action-started` | `-v` | `action`, `resource_class`, `wave`, `scheduler_wait_ms`, `resource_wait_ms` |
| `action-cache-decision` | `-v` | `action`, `cache` |
| `wave-started` | `-vv` | `wave` (0-based), `actions` |
| `action-queued` | `-vv` | `action`, `resource_class`, `wave` |
| `action-wait-started`, `action-wait-finished`, `phase-wait-started`, `phase-wait-finished` | `-vv` | `wait`, then `outcome` and `duration_ms` |
| `resource-usage-changed` | `-vv` | `capacity`, `used`, `generation` |
| `action-described` | `-vvv` | `action_kind`, `inputs`, `outputs`, `total_inputs`, `total_outputs` |
| `action-activity-started`, `action-activity-finished` | `-vvv` | `activity`: `checking`, `codegen`, `linking`, or `compiling and linking`; finish adds `duration_ms`, `success` |

`outcome` on `action-finished` is `cached`, `executed`, `nowork`, `failed`, or
`cancelled`. `cache` is `{"status": "hit"}` or `{"status": "miss", "reason":
...}` with `reason` `not_found`, `invalidated`, `uncacheable`, `corrupt`,
`read_error`, or `write_error`. `wait` is `dependency`, `resource permit`, `output lock`, or
`cache lock`, and a wait's `outcome` is `acquired`, `cancelled`, or `failed`.
`resource_class` is `cpu`, `io`, or `conservative`. Paths in `inputs` and
`outputs` are `{"root": {"kind": ...}, "path": ...}`, relative to a package,
the build or cache directory, the toolchain, or an artifact. Progress never
includes process arguments, environment values, or generated file contents.
Later releases may add fields within schema version 1.
