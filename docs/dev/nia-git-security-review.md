# nia-git Security Review

Date: 2026-10-21

## Status and Scope

This is a focused internal review of the native Git checkout implementation.
It is based on source inspection, regression tests, parser properties, and
local consumer tests. It is not an independent audit, a penetration test, or a
coverage-guided fuzz campaign.

The review covered repository object and ref reads, pack and delta decoding,
tree checkout, pkt-line and upload-pack responses, HTTP framing, SSH key
exchange and transport sequencing, host-key checking, identity and agent
parsing, and SSH channel lifecycle handling.

## Findings Addressed

- Pack integer truncation, checksum-boundary reads, truncated compression
  streams, unbounded aggregate inflation, unresolved forward deltas, and
  excessive delta depth now fail with bounded resource use.
- Checkout rejects linked destinations and ancestors, nonempty staging roots,
  path traversal and platform aliases. Tree expansion uses explicit depth,
  entry-count, and decoded-byte budgets, and creates files exclusively.
- Revision lookup validates ref names, rejects linked ref paths, resolves
  loose and packed refs consistently, and bounds symbolic-ref and annotated
  tag dereferencing.
- HTTP parsing rejects malformed status/header syntax, ambiguous body
  framing, incorrect content lengths, incomplete chunk terminators, forbidden
  framing trailers, and data after the final chunk. Response framing sections
  are bounded.
- Upload-pack sideband 2 progress is accepted; sideband 3 remains fatal.
- SSH channel processing validates recipients, flow-control windows, packet
  sizes, EOF and close ordering, process exit status, and session byte budgets.
  A failed remote process cannot publish its otherwise valid pack.
- SSH packet sequence numbers continue across NEWKEYS because strict key
  exchange is not negotiated. Plaintext KEX traffic and encrypted IGNORE/DEBUG
  messages are bounded and structurally checked.
- SSH host keys are checked against `known_hosts` after signature verification;
  private identity permissions are checked on Unix and Windows.

Regression evidence is in the owning modules and
[`testing.md`](testing.md). At review time, `PROPTEST_CASES=4096 cargo test -p nia-git`
passed 109 unit tests and 4 integration tests. Package metadata tests,
SSH build consumers, Clippy, workspace compilation, formatting, and the
maintenance check also passed locally. The Windows-specific tests ran in the
current Windows environment. No remote CI execution is claimed.

## Remaining Limits

- Filesystem path validation and subsequent reads/writes are not an atomic
  transaction. Another local process with write access to repository or
  checkout ancestors can race checks. Checkout callers must own the staging
  directory and discard partial output after an error.
- Byte budgets exclude allocator/container overhead and transient compression
  buffers. They bound decoded payload accounting, not exact process memory.
- The SSH implementation is a deliberately limited client: it supports the
  implemented X25519, Ed25519 host-key, AES-128-CTR, HMAC-SHA2-256, public-key
  authentication, and one upload-pack channel. It does not implement rekey,
  host certificates, encrypted identity files, or the full SSH connection
  protocol. Unsupported messages and algorithms fail closed. Interoperability
  coverage uses a local protocol fixture; no live third-party SSH server was
  used in this review.
- SSH agent and credential-helper behavior depends on local processes and
  operating-system facilities. Unix SSH agent socket reads and writes have a
  30-second timeout. Windows named-pipe agent I/O and Git credential-helper
  processes still lack a universal timeout.
- HTTP redirects, proxy authentication, and interim HTTP responses are not
  supported. Unsupported response forms fail rather than being followed.
- Stable property tests exercise bounded generated inputs but do not provide
  coverage-guided fuzzing, exploitability proof, or external assurance.

These limits are documented constraints, not claims that the corresponding
threats have been eliminated. Changes to parser boundaries, transport state,
filesystem output, cryptography, or resource accounting should extend the
regression suite and revisit this review.
