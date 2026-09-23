<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Diagnostics Roadmap

Nia's terminal diagnostics are a compiler product, not a last-mile string
formatter. A useful report must answer four questions in order:

1. What source construct is wrong?
2. Which compiler rule rejected it?
3. What other location or declaration explains the rule?
4. What can the user change to continue?

The implementation is being upgraded in bounded stages. Each stage must keep
the existing source spans and diagnostic codes stable unless the old contract
was actively misleading.

Status: in progress. The implemented paths below are incremental coverage, not
completion of the whole roadmap. Remaining work includes broader source-origin
and recovery provenance, rule-specific semantic diagnostics, terminal-platform
validation, and fixture/snapshot coverage for the complete diagnostic rule set.

## Invariants

- A user-facing error has a source-owned primary label whenever the compiler
  has a source location. Generated code and fallback locations are never shown
  as the primary location when the originating source location is known.
- A diagnostic has one root cause. Later phases may retain context internally,
  but they must not publish errors that only describe an invalid recovery value
  created by an earlier phase.
- Independent modules may continue checking after one module fails. Within one
  module, downstream checks suppress only diagnostics whose codes describe
  recovery products poisoned by an earlier root; independent resolution and
  constraint errors remain visible.
- Diagnostics are structured until the CLI boundary. Formatting, suppression,
  color, and terminal layout do not belong in semantic or query providers.
- Every diagnostic shown to a user has a stable code, a precise primary span,
  and either a useful label, note, or help action. Internal diagnostics are
  clearly separated from source errors.

## Stages

### 1. Cascade control

The query facade now gates per-module diagnostic collection by root diagnostic
code. Unresolved names and invalid signatures suppress only downstream type,
const, layout, and code-generation products while independent resolution and
constraint errors remain visible. Build-runner compilation performs a source
check first and publishes `build.nia` diagnostics instead of generated-wrapper
recovery errors when both are present.

Regression coverage includes incremental supertrait constraints, generated
runner ownership, and unresolved build-script values.

### 2. Source ownership and related locations

Introduce an explicit diagnostic origin/relationship model. A generated call
site may be retained as a secondary location, while the declaration or source
expression that caused it becomes primary. This is required for build runners,
macro-like generated code, imported declarations, and cross-module trait
resolution.

The first source-evidence chain is now implemented for failed `using` lookups:
the using scope retains the exposed name, selected-name span, directive span,
and a classified cause (`unknown`, private, not-public, or invisible namespace).
Value and type resolution consume that evidence at the actual use site, while
the using directive remains available as related context. The evidence is
indexed by local name so repeated lookups do not scan the complete import list.

Text reports load source text for explicit related paths, including dependencies
with no diagnostics of their own. Related locations use their owning file's
line and column; unavailable external sources retain byte spans instead of
borrowing the primary file's line numbers.

### 3. Semantic diagnostic contracts

Replace generic summaries such as `name is unresolved` and broad `type-check`
messages with rule-specific constructors. Each constructor owns:

- the diagnostic code;
- the primary and secondary labels;
- the expected/actual type rendering;
- optional candidate lists;
- a concrete help action.

The first targets are name lookup, qualified values, imports, error-union
propagation, callable resolution, and visibility.

Name lookup, qualified namespace lookup, and failed using directives now use
rule-specific summaries with primary labels, related source locations, and
actionable help. Ordinary unresolved body values now identify the missing name
and use the name-resolution code. Type and module namespaces used as values
also have distinct contracts with declaration evidence where available.
Assignment/place checking reuses the same import evidence and avoids publishing
secondary place errors after an unresolved expression, preventing syntax-
specific fallbacks such as `module value is unresolved` from hiding the original
import failure.

This coverage is still incremental: qualified callable failures, error-union
propagation failure variants beyond the current boundary, invalid-operand, and
missing-conversion cases, and other recovery-derived semantic rules need
equivalent rule-specific contracts and source fixtures.

### 4. Report organization

Make the report explicitly hierarchical: root errors first, related context
under each root, then independent errors. Suppression must say whether entries
were duplicates, downstream consequences, or a display limit. The report must
never present a generated wrapper before the source error that invalidated it.

The report layer now keeps deterministic duplicate, downstream-consequence, and
display-limit counts, counts retained errors and warnings, and ranks source-owned
primary spans ahead of generated wrappers. Downstream suppression counts survive
checked/codegen products and frontend check-certificate cache reuse. The full
root/related/independent hierarchy is now ordered explicitly when diagnostics
carry an unambiguous source-owned cause identity. That cause identity is retained
in stable diagnostic bundles. Failed `using` lookups now carry their emitted
root identity through value, type, and body resolution, so later value and type
use-site diagnostics are grouped under the import failure. A source fixture now
covers repeated use of the failed import in a function parameter and return
type, and snapshots the shared root identity. Other recovery-derived diagnostics
still need explicit provenance before this stage is complete.

### 5. CLI interaction contract

Add stable text and machine-readable output modes, explicit summary counts, and
consistent exit status behavior for check, emit, build, and test. Terminal
rendering should provide color and compact context when interactive, while
non-interactive output remains deterministic and snapshot-friendly.

Text and JSON reports now expose the same retained error/warning summary and
duplicate/display-limit counts. Interactive text diagnostics are colorized at
the CLI boundary, while captured and JSON output remains deterministic and
free of ANSI control sequences. CLI cases now assert exact success and failure
statuses for check, emit, build, and test. A Linux pseudo-terminal integration
test verifies color on interactive text output and ANSI-free JSON output on the
same terminal. Other terminal platforms and broader output snapshot coverage
remain to be verified.

### 6. Regression and quality gates

Every new diagnostic rule requires a source fixture and a snapshot containing:

- code and severity;
- primary path, line, and column;
- source excerpt and label;
- notes/help/related locations;
- suppression summary when applicable.

The failed-using provenance chain now has a source fixture and snapshot,
including the cause identity of repeated signature type diagnostics. Check-case
snapshots now include the rendered text report as well as structured fields,
covering severity, line/column, source excerpts, related locations, hierarchy,
and summary counts. This also covers reports after an incremental body edit.
A private-using fixture verifies related locations in a clean dependency whose
declaration is on a different line from the primary use site. Renderer tests
cover available and unavailable related sources and explicit same-file paths.
Unknown values, type-as-value misuse, and module-as-value misuse now each have
dedicated fixtures and complete structured/text snapshots.
Optional and error-union propagation boundaries, invalid propagation operands,
and missing `IntoError` conversions now share an end-to-end case with complete
structured/text snapshots, including the enclosing function return boundary.
The complete diagnostic rule set still needs equivalent fixture/snapshot
coverage.

The broad workspace test, clippy, formatting, and CLI case suites remain
mandatory for each stage.
