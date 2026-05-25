# Nia Maintenance Workflow

This document describes the standard workflow for maintaining Nia features,
fixes, releases, and issue-linked development.

Nia is maintainer-led. The workflow should stay lightweight, but every merged
change should leave a clear trail from issue discussion to implementation,
review, tests, versioning, and tag.

## 1. Start From an Issue

Every non-trivial feature or bug fix should start from an issue. The issue may
be opened by the maintainer or by another contributor.

Use the issue to define the work before implementation starts:

```markdown
## Goal
What problem this issue solves.

## Scope
What this issue will change.

## Non-goals
What this issue explicitly will not change.

## Acceptance
- [ ] Accepted examples compile or run.
- [ ] Rejected examples produce diagnostics.
- [ ] Tests are added or updated.
- [ ] Documentation is updated if behavior is user-visible.
```

Keep semantic discussion, design tradeoffs, and scope decisions in the issue so
future readers can understand why the change exists.

## 2. Create a Branch

Create the branch from the latest `main`:

```sh
git switch main
git pull
git switch -c feat/1-void-entity
```

Use branch names that identify the change and, when possible, the issue:

```text
feat/<issue>-short-name
fix/<issue>-short-name
docs/<issue>-short-name
chore/<issue>-short-name
```

Examples:

```text
feat/1-void-entity
fix/12-parser-recovery
docs/18-release-workflow
```

## 3. Implement the Change

Implement the feature or fix on the branch. A complete change may include:

- compiler implementation;
- tests;
- documentation;
- diagnostics;
- examples;
- version bump, if the pull request is also a release point.

Prefer small, coherent changes. Do not include unrelated refactors or formatting
churn.

## 4. Version Policy

`Cargo.toml` is the source of truth for the released version.

The release tag and CLI version should follow it:

```text
Cargo.toml version = 0.1.1
git tag = v0.1.1
niac --version = 0.1.1
```

Do not make `niac` derive its version dynamically from git metadata. Source
archives, release packages, and installed binaries may not include a complete
git checkout.

Only bump the version when the pull request is intended to become a release
point. For the current early Nia workflow, if each feature merge is tagged, bump
the patch version before opening the pull request.

## 5. Local Quality Checks

Before committing a pull-request-ready change, run:

```sh
cargo fmt --check
cargo check --workspace
cargo test --workspace
```

These checks are mandatory for compiler changes.

## 6. Clippy Without Allows

Run Clippy as a separate quality gate:

```sh
cargo clippy --workspace --all-targets -- -D warnings
```

Do not add `allow`, `expect`, or broad lint suppression attributes only to pass
Clippy. Fix the code instead.

If a lint is genuinely wrong for Nia, discuss it explicitly before suppressing
it. Any suppression should be narrow, justified, and reviewed.

## 7. Commit and Push

Check the worktree before committing:

```sh
git status
```

Commit with a short conventional message:

```sh
git add .
git commit -m "feat: add first-class void entities"
git push -u origin feat/1-void-entity
```

Common prefixes:

```text
feat:     new user-visible behavior
fix:      bug fix
docs:     documentation-only change
test:     test-only change
refactor: internal restructuring without behavior change
chore:    maintenance work
```

## 8. Open a Pull Request

Open a pull request from the feature branch into `main`.

Use the pull request body to summarize behavior, link the issue, and record the
checks that were run:

```markdown
## Summary
- add first-class `void` values
- allow empty structs as zero-sized types
- require explicit casts to `&void`

Closes #1

## Tests
- cargo fmt --check
- cargo check --workspace
- cargo test --workspace
- cargo clippy --workspace --all-targets -- -D warnings

## Notes
- empty unions remain unsupported
```

Use `Closes #N` when merging the pull request should close the issue. Use
`Refs #N` when the pull request is related but does not complete the issue.

## 9. Review and Merge

Review the pull request on Codeberg before merging:

- read the diff;
- check the issue link and pull request description;
- verify CI or local check results;
- confirm tests cover the intended behavior;
- confirm the version was bumped if this pull request is a release point.

For one-feature branches, prefer squash merge to keep `main` readable. If the
branch has multiple meaningful commits, a normal merge is acceptable.

Delete the remote feature branch after merge. Codeberg can do this as part of
the merge operation.

## 10. Sync Local Main

After the pull request is merged:

```sh
git switch main
git pull
git branch -d feat/1-void-entity
git fetch --prune
```

## 11. Confirm the Version

Confirm that the CLI reports the same version as `Cargo.toml`:

```sh
cargo run -p nia-cli -- --version
```

The output should match the intended release, for example:

```text
niac 0.1.1
```

## 12. Tag the Release

Create the tag only from the merged `main` commit:

```sh
git tag -a v0.1.1 -m "v0.1.1"
git push origin v0.1.1
```

The tag must match the Cargo version and CLI version.

## 13. Optional crates.io Publishing

Publishing to crates.io is optional and may be introduced later.

If Nia starts publishing crates, do it after the release tag is created:

```sh
cargo publish -p nia-cli
```

The project should decide separately whether to publish only `nia-cli`, selected
library crates, or the full workspace.

## Recommended End-to-End Sequence

```text
Issue
-> discussion and scope decision
-> branch from main
-> implementation, tests, and docs
-> version bump if this is a release point
-> fmt/check/test
-> clippy without allows
-> commit and push
-> Codeberg pull request with Closes/Refs issue link
-> review
-> merge into main and delete remote branch
-> local main pull
-> delete local branch
-> confirm niac --version
-> create and push vX.Y.Z tag
-> optional crates.io publish
```
