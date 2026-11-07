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
package's own executables and tests open it with `using <package name>;`.
Importing a package that declares no library is an error.

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

An executable produces the step `<name>`, and `run-<name>` when the build's
target is the host; a test suite produces the step `<name>`, which builds and
runs it, and joins `test`. Names follow the
[step-name rule](cli.md#nia-build), must be unique across both tables, and
cannot be `all`, `test`, or start with `run-`.

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

## Git Sources

Nia fetches Git dependencies itself, with a shallow fetch of the selected
commit. Git URLs may use `https://`, `http://`, `git://`, `ssh://`, SCP-style
`git@host:path`, or `file:///`, and cannot embed credentials. `http://` and
`git://` are neither encrypted nor authenticated; prefer HTTPS or SSH.

### HTTPS Authentication

Nia first requests the repository anonymously. When the server answers `401`,
Nia asks `git credential fill` for a username and password matching the
repository host and path and retries with HTTP Basic authentication; public
repositories never reach the credential helper. Accepted credentials are
reported back with `git credential approve`, and rejected ones with
`git credential reject`, so the helper keeps only working credentials. This
uses the configured Git credential helper (such as Git Credential Manager); the
Git executable is needed only for this lookup, while Nia still performs the
fetch itself. Credentials are held in memory for the request and are not
written to the manifest, lockfile, or cache. Nia does not send credentials over
plain HTTP.

### Proxies

HTTP Git sources honor `HTTP_PROXY` and `http_proxy`; HTTPS sources honor
`HTTPS_PROXY` and `https_proxy`. Both fall back to `ALL_PROXY` or `all_proxy`,
and `NO_PROXY` or `no_proxy` can bypass the proxy for listed hosts or domains,
optionally restricted to a port. Proxies must be unauthenticated `http://`
URLs. HTTPS requests use CONNECT and validate TLS against the repository host;
repository credentials are sent only inside that encrypted connection.

### SSH Authentication

SSH sources verify the server key against `~/.ssh/known_hosts` (or
`%USERPROFILE%\.ssh\known_hosts` on Windows), accepting plain and hashed
OpenSSH entries, and reject unknown or changed host keys. Nia offers the
Ed25519, ECDSA P-256, and RSA (with RSA-SHA2-512 signatures) identities in the
running SSH agent (`SSH_AUTH_SOCK` on Unix-like systems or the OpenSSH agent
named pipe on Windows) in the order the agent lists them, as `ssh-add -l`
shows, followed by an unencrypted `~/.ssh/id_ed25519`. Like OpenSSH, it asks
the server whether it accepts each key before signing, and it offers at most
five keys per connection to stay under the server's default attempt limit. A
key the agent declines to sign, for example a declined `ssh-add -c`
confirmation, is skipped for the next one. When authentication fails, the
error lists each offered key's fingerprint as `ssh-keygen -l` prints it and
what happened to it. Only public-key authentication is supported, so a server
that also requires a password or other method fails after accepting the key.
Other key types and encrypted key files are not read directly; load a supported
key into the agent instead.

## `[registries]`

Maps registry names to `https://` base URLs without credentials, a query, or a
fragment. Names use lower-case ASCII letters, digits, and `-`. Only the root
manifest (the workspace root in a workspace) may declare registries.

## `[workspace]`

| Key | Meaning |
| --- | --- |
| `members` | Member package directories; glob patterns such as `packages/*` are allowed. |
| `default-members` | Member paths or glob patterns built by `nia build` at the root; all members when omitted. It must select at least one member. |
| `exclude` | Directories under the root that are not members. |

`[workspace.package]` holds `namespace`, `version`, and `protocol-epoch` for
members to inherit. `[workspace.dependencies]` holds dependencies, in the
dependency-table form above, that members reference with
`{ workspace = true }`; their paths are relative to the workspace root.

A package under the workspace directory that is neither a member nor excluded
cannot be built; building inside it fails. A nested workspace cannot be a
member of its parent.

## `nia.lock`

`nia.lock` is written next to the root manifest and records the resolved source
of every dependency. It is maintained by `nia build` and `nia update`; do not
edit it by hand. A lockfile from an older format is rejected and must be
regenerated by deleting it and building again.
