# Manifest Reference

Every package has a `nia.toml` manifest at its root. This page lists every
table and key it accepts; an unknown table or key is an error.
[Packages and dependencies](../guides/packages.md) explains how to use them.

```toml
[package]
name = "app"
namespace = "acme"
version = "0.1.0"
protocol-epoch = 3

[library]
path = "lib/root.nia"

[source]
roots = ["."]
exclude = ["fixtures/large"]

[[executable]]
name = "app"
path = "src/main.nia"

[[test]]
name = "unit"
path = "tests/unit.nia"

[dependencies]
core = { path = "../core" }
regex = { git = "https://example.com/regex.git", tag = "v1.2.0" }
json = { registry = "nia", package = "acme/json", version = "^1.4" }

[dev-dependencies]
fixtures = { path = "tests/fixtures" }

[registries]
nia = "https://packages.example.test"
```

## `[package]`

| Key | Required | Meaning |
| --- | --- | --- |
| `name` | yes | The package name, a Nia identifier of at most 128 bytes. |
| `namespace` | no | Identity namespace; defaults to `local`. |
| `version` | yes | A strict SemVer version such as `0.1.0` or `1.2.0-beta.1`. |
| `protocol-epoch` | no | The compiler release line the package requires. A compiler from another line rejects the package. Releases `0.x` use epoch `x`. |

`namespace`, `version`, and `protocol-epoch` may be written
`{ workspace = true }` to inherit the value from
[`[workspace.package]`](#workspace).

A manifest without `[package]` must declare `[workspace]`; it is then a
virtual workspace root.

## `[library]`

| Key | Meaning |
| --- | --- |
| `path` | The file that roots the library's module tree, relative to the package root. It must lie inside the package's source roots. |

Other packages open the library with `using <dependency name>;`, and the
package's own executables and tests open it with `using <package name>;`. A
package that others depend on must declare a library.

## `[source]`

| Key | Default | Meaning |
| --- | --- | --- |
| `roots` | the whole package directory | Package-relative directories and files that belong to the package. |
| `exclude` | none | Package-relative paths left out of the roots. |

`.git` and `nia.lock` are always left out, and so are `build/`, `dist/`, and
`target/` directly under the package root unless `roots` names one of them.
A package's source content determines its identity in `nia.lock`.

## `[[executable]]` And `[[test]]`

Each entry declares one artifact:

| Key | Required | Meaning |
| --- | --- | --- |
| `name` | yes | The artifact name, which is also its step name. |
| `path` | yes | The file that roots the artifact's module tree, relative to the package root. |
| `runtime` | no | `freestanding` (default) for the standard process entry, or `bare` for none. |

An executable produces the steps `<name>` and `run-<name>`; a test suite
produces the step `<name>` and joins `test`. Names must be unique across both
tables and cannot be `all`, `test`, or start with `run-`.

## Dependency Tables

| Table | Scope |
| --- | --- |
| `[dependencies]` | the library, executables, and tests |
| `[dev-dependencies]` | test suites |
| `[build-dependencies]` | `build.nia` |
| `[optional-dependencies]` | modules that a build script explicitly imports it into |
| `[lazy-dependencies]` | the same as `[dependencies]` |

Each key is a dependency name, a Nia identifier unique across all of the
package's dependency tables. Its value is an inline table with exactly one
source:

| Keys | Source |
| --- | --- |
| `path` | A local package directory, relative to the declaring package. |
| `git`, plus at most one of `rev`, `tag`, `branch` | A Git repository. `rev` is a full 40-digit commit hash. Without a selector, the remote `HEAD`. |
| `registry`, `package`, `version` | A registry package: the registry name, `namespace/name`, and a SemVer requirement with an explicit operator (`^1.4`, `~1.4.2`, `>=1.1, <2`, `=1.4.2`, `1.*`). |
| `workspace = true` | The dependency of the same name in `[workspace.dependencies]`. No other key is allowed. |

Git URLs may use `https://`, `http://`, `git://`, `ssh://`, SCP-style
`git@host:path`, or `file:///`, and cannot embed credentials.

## `[registries]`

Maps registry names to `https://` base URLs without credentials, a query, or a
fragment. Names use lower-case ASCII letters, digits, and `-`. Only the root
manifest (the workspace root in a workspace) may declare registries.

## `[workspace]`

| Key | Meaning |
| --- | --- |
| `members` | Member package directories; glob patterns such as `packages/*` are allowed. |
| `default-members` | Members built by `nia build` at the root; all members when omitted. |
| `exclude` | Directories under the root that are not members. |

`[workspace.package]` holds `namespace`, `version`, and `protocol-epoch` for
members to inherit. `[workspace.dependencies]` holds dependencies, in the
dependency-table form above, that members reference with
`{ workspace = true }`; their paths are relative to the workspace root.

Every package under the workspace directory must be a member or excluded. A
nested workspace cannot be a member of its parent.

## `nia.lock`

`nia.lock` is written next to the root manifest and records the resolved source
of every dependency. It is maintained by `nia build` and `nia update`; do not
edit it by hand. A lockfile from an older format is rejected and must be
regenerated by deleting it and building again.
