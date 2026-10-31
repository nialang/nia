# Archived Development Documents

This directory contains historical documents from the Nia compiler and standard library development. These files are retained for reference but are no longer current guidance.

## Categories

### Historical Bug Investigations
- `COMPILER_BUGS_2026-10-28.md` - Initial closure inference investigation; superseded by `closure-inference.md`
- `bug-enum-shorthand-generics.md` - Early hypothesis on enum shorthand; resolved

### Historical PRs and Proposals
- `pr-01-option-okOr.md` - Historical proposal for Option conversion APIs
- `pr-01-option-okOr-implemented.md` - Implementation report; APIs now in `lib/std/option.nia`

### Exploratory Reviews
- `stdlib-review-2026-10-28.md` - First-pass standard library review
- `stdlib-review-2026-10.md` - Active checklist; archived after completion
- `stdlib-refactor-examples.md` - Exploratory refactoring proposals

### One-Time Validation Reports
- `sized-provider-validation.md` - Sized/provider fix validation (Oct 30)
- `wyhash-runtime-validation.md` - Wyhash streaming validation (Oct 30)

## Current Documentation

For current compiler and standard library documentation, see the parent `docs/dev/` directory:
- `architecture.md` - Compiler pipeline and crate organization
- `closure-inference.md` - Generic callable closure inference specification
- `contributing.md` - Contribution policy and review process
- And others
