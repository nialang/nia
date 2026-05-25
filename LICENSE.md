# Repository Licensing

This repository contains the Nia compiler implementation, project
documentation, and repository metadata. These materials do not all share the
same license.

## Compiler Implementation

The `niac` compiler implementation is licensed under `GPL-3.0-or-later`.

This covers:

- `crates/`
- `Cargo.toml`
- `Cargo.lock`
- compiler-oriented test sources and fixtures in this repository

The full GPLv3 text is available at
[LICENSES/GPL-3.0-or-later.txt](LICENSES/GPL-3.0-or-later.txt).

The `or-later` grant means the covered compiler implementation may be used
under the terms of the GNU General Public License version 3, or any later
version published by the Free Software Foundation.

## Documentation

The documentation in `README.md` and `docs/` is not licensed under the compiler
implementation license unless a future document explicitly says otherwise.

This keeps the language specification and project notes separate from the
compiler's copyleft terms. A dedicated documentation license can be chosen later
without changing the compiler license.

## User Programs and Future Projects

Nia programs compiled with `niac` are not covered by the compiler license merely
because they were compiled with `niac`.

Future Nia standard library, package manager, build system, or ecosystem
repositories are separate projects and should declare their own licenses.
