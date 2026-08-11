# Nia Maintenance Tools

The repository maintenance interface is `python3 -m tools`. It owns command
discovery, the Python runtime boundary, common error behavior, and the complete
fast tooling gate. Domain logic remains in the module that owns its inputs and
output schema.

The required CPython major/minor version is declared in the repository
`.python-version`. Managed workflows consume that file directly. Local tools
must use the same major/minor version; changing it requires running the complete
tool test and audit gate on the replacement interpreter.

Commands are grouped by purpose:

```text
python3 -m tools audit compatibility
python3 -m tools audit std-build-host
python3 -m tools report crate-boundaries
python3 -m tools baseline compiler
python3 -m tools baseline compare <baseline.json> <candidate.json>
python3 -m tools baseline build
python3 -m tools check
```

`audit` commands enforce repository invariants and return no report on success.
`report` commands produce deterministic maintainer evidence without changing
the repository. `baseline` commands own repeatable measurements and their
machine-readable schemas. `check` runs all tool tests followed by every fast
audit; it does not run compiler or build baselines.

Fixtures belong in `tools/fixtures/`, tests in `tools/tests/`, and shared
implementation support in `tools/nia_tools/common/`. Do not add another
top-level executable script; add a command whose domain module can also be
tested directly.
