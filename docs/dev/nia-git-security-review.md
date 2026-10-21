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
  entry-count, and decoded-byte budgets. Output traversal, exclusive file
  creation, temporary metadata creation, and metadata cleanup are relative to
  opened `cap-std` directory handles; replacing the destination pathname after
  opening its root does not redirect output to a linked replacement.
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

- The path-based `checkout_source` API still validates ancestors before its
  ambient root open, so another local process with write access to a checkout
  ancestor can race that initial lookup. Callers that already own a directory
  handle can use `checkout_source_into`; worktree output then stays relative to
  that handle. Package Git fetches use this handle-based entry point. Remote
  pack objects are decoded from the validated response bytes without a
  temporary `.git` tree or staging-path lookup. Local repository object and ref
  reads remain pathname-based and require repository ownership against local
  races.
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
  operating-system facilities. Unix SSH agent socket and Windows named-pipe
  reads and writes have a 30-second timeout. Git credential-helper processes
  have a 30-second deadline and a 64 KiB output limit; timeout cleanup targets
  the Unix process group or Windows process tree.
- HTTP redirects, proxy authentication, and interim HTTP responses are not
  supported. Unsupported response forms fail rather than being followed.
- Stable property tests exercise bounded generated inputs but do not provide
  coverage-guided fuzzing, exploitability proof, or external assurance.

These limits are documented constraints, not claims that the corresponding
threats have been eliminated. Changes to parser boundaries, transport state,
filesystem output, cryptography, or resource accounting should extend the
regression suite and revisit this review.

## Handle-Based Checkout Follow-Up

On 2026-10-22, package Git fetches moved to `checkout_source_into`, which keeps
worktree writes on an already opened directory handle and decodes remote pack
objects directly from the validated response.
Regression tests replace the original destination path with a symlink or
junction after opening the handle and verify output remains in the opened
directory. Windows and Unix variants passed locally. The current focused run
passed 111 `nia-git` unit tests, 4 Git checkout integration tests, and 76
`nia-package-metadata` tests; Clippy and `cargo maintain check` also passed.

## Post-Review Fuzzing

On 2026-10-21, the initial six coverage-guided parser targets completed short
local campaigns under Ubuntu 24.04 in WSL: packet lines, loose objects, packed
objects, HTTP responses, SSH agent identities, and `known_hosts`. No crash was
observed. The follow-up targets for delta instructions, SSH KEXINIT, and
OpenSSH private identities also completed short campaigns without a crash.
All nine minimized corpora are checked in under
[`crates/nia-git/fuzz/corpus`](../../crates/nia-git/fuzz/corpus), and the
weekly Linux workflow repeats longer campaigns. These runs establish that the
harnesses execute and provide initial corpus coverage; they are not a sustained
fuzzing campaign or external assurance.
