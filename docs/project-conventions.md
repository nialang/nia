# Nia Project Conventions

This document records repository-level conventions for the first public Nia
release. It is about how the project is maintained, not about user-facing
language syntax.

## No Historical Compatibility Surface

Nia has not had a public release yet. Temporary syntax and behavior that existed
during development are not part of the language contract.

Do not keep special parser paths, diagnostics, examples, or tests whose main
purpose is to explain an old Nia spelling. If a removed spelling is written
today, it should be treated like any other invalid syntax.

Examples:

- Do not keep migration hints for removed generic-call spellings such as
  `callee::[T]`.
- Do not keep positive tests for syntax that is no longer in the language.
- If an old construct now represents an important boundary, test it as a normal
  rejection of the current language, not as a compatibility case.

## Test Intent

Tests should document the current language and compiler behavior.

Use positive tests for accepted syntax and semantics. Use negative tests for
current semantic boundaries and diagnostics that matter to users. Avoid keeping
tests only because they increase test count or preserve development history.

When reviewing tests, delete obsolete cases or rewrite them into explicit
current-language rejection tests.
