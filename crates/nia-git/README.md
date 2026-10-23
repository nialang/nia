# nia-git

`nia-git` is the native Git client that fetches package dependencies. It
implements the smart protocol itself, without libgit2 or a system Git, over
local repositories, `git://`, HTTP(S), and SSH, and checks out one commit into
a caller-opened directory. The one exception is HTTPS credentials, which come
from `git credential` when a `git` executable is on `PATH`; without it, a
repository that answers `401` fails. Package resolution, caching, and
`nia.lock` belong to
[`nia-package-metadata`](../nia-package-metadata/); the user-facing source,
credential, and proxy contract is in
[Build scripts](../../docs/guide/build-scripts.md#dependency-sources).

The public surface is `checkout_source(source, revision, destination)` for
dependency checkouts and `fetch_https` for credential-free registry resources.

This README is the crate's security model: what the client enforces, the
limits it applies, and what it does not protect against. It is an internal
review, not an independent audit or penetration test. Module mechanics live in
each module's `//!` docs.

## Layout

| Module | Owns |
| --- | --- |
| `input` | Bounded reads shared by transports, repository files, and SSH configuration |
| `pktline` | Packet-line framing |
| `object/` | Loose objects, packs, delta application, ref and revision resolution, handle-relative repository access |
| `transport/local`, `transport/git_tcp` | Local-repository and `git://` checkout |
| `transport/http` | HTTP(S) URLs, proxies, request framing, response parsing, and the credential-helper bridge |
| `transport/protocol` | Upload-pack requests, advertised refs, and side-band responses |
| `transport/ssh/` | Key exchange, packet protection, wire encoding, channel, identities, agent, and `known_hosts` |

Checkout integration tests are in [`tests/`](tests/); parser fuzz targets and
their corpora are in [`fuzz/`](fuzz/).

## Enforced Boundaries

- **Checkout output.** `checkout_source` writes only through an already opened
  destination directory handle; replacing the destination pathname after it is
  opened does not redirect output. Output traversal, exclusive file creation,
  and metadata cleanup are relative to `cap-std` directory handles. The
  destination directory must be empty; opening it safely (no linked ancestors)
  and discarding partial output on failure belong to the caller. Trees are walked with an explicit
  stack, and each path is created exclusively. Tree entries cannot name Git
  metadata, traverse directories, or use Windows device names (including with
  trailing spaces or dots before an extension) and alternate data streams. Remote pack objects are decoded from the validated response
  bytes, without a temporary `.git` tree.
- **Local repositories.** Source paths are made absolute lexically and opened
  component by component without following links. Refs, loose objects, and
  pack files are read relative to those handles, so linked source components,
  ref paths, object fanout, and pack directories are rejected.
- **Revisions.** Ref names are validated before any file access. Loose and
  packed refs resolve the same way, and symbolic-ref and annotated-tag
  dereferencing is bounded.
- **Packs and deltas.** Pack inflation charges the declared size before reading
  and validates the decoded size of objects and delta instructions. Truncated
  streams, checksum-boundary reads, unresolved forward deltas, and excessive
  delta depth fail. Each delta is resolved once without cloning its base.
- **HTTP.** Status lines must be HTTP/1.0 or HTTP/1.1. Repeated or conflicting
  `Content-Length`/`Transfer-Encoding`, unsupported transfer codings, body
  length mismatches, incomplete chunk terminators, framing fields in trailers,
  and bytes after the final chunk are rejected. Chunk extensions are ignored
  after checking for control characters. Redirects and interim responses are
  not followed; any non-2xx status fails.
- **Credentials.** The first HTTPS request is anonymous. Only a `401` asks
  `git credential fill` (with `GIT_TERMINAL_PROMPT=0`) for credentials, and the
  request is retried once with them. Credentials that then succeed are reported
  with `git credential approve`; credentials that still receive `401` are
  reported with `git credential reject`. 403 and 404 are not treated as an
  authentication challenge. The helper runs with a 30-second deadline and a
  64 KiB output limit, and timeout cleanup kills its Unix process group or
  Windows process tree. Credentials are held in zeroized memory, sent only over
  TLS (directly or inside an HTTPS proxy `CONNECT` tunnel), and never written to
  manifests, lockfiles, caches, or diagnostics. URLs with embedded credentials
  are rejected. Registry resource fetches never request credentials.
- **Upload-pack.** Sideband 2 progress is ignored; sideband 3 is fatal.
- **SSH transport.** Host keys are checked against `known_hosts` after the
  server signature verifies. MAC sequence numbers continue from the plaintext
  handshake through NEWKEYS, because strict key exchange is not negotiated. The
  handshake bounds ignored and debug messages, discards one guessed server
  key-exchange packet when `first_kex_packet_follows` is wrong, and rejects
  noncanonical booleans, a nonzero KEXINIT reserved field, and trailing bytes in
  authentication success. Authentication banners are checked for framing and
  discarded, not displayed.
- **SSH channel.** The channel validates recipients, flow-control windows,
  packet sizes, EOF and close ordering, and exit requests, both in normal reads
  and while waiting for send-window space. EOF closes only the remote data
  direction; checkout reads through channel close, so a nonzero exit status or
  exit signal fails the transfer before any file is written. A transport EOF
  before channel close is an error, while a missing optional exit status is
  not. Unknown requests that want a reply receive `CHANNEL_FAILURE`, and
  incoming stderr and ignored control messages count toward the session
  traffic budget.
- **SSH host matching.** A bare `known_hosts` pattern trusts port 22 only; a
  server on another port must be listed as `[host]:port`, as in OpenSSH.
- **SSH identities.** Private identity files must not be readable by other
  principals: on Unix, the file must be owned by the current user, opened
  without following links, and have no group or other permission bits; on
  Windows, the owner and every read grant must be the current user, SYSTEM, or
  Administrators, and allow entries of other types are rejected. Agent keys are
  offered in the agent's order, then `id_ed25519` unless the agent holds the
  same key, at most five in all, one below OpenSSH's default `MaxAuthTries`.
  Each key is first offered without a signature, and only a key the server
  answers with `PK_OK` for that exact key and algorithm is signed, so rejected
  keys never reach the agent's signing path. A key that fails to sign is
  skipped for the next one. Authentication stops early when the server
  disconnects, reports partial success (other methods are not supported), or
  stops listing `publickey`. Every authentication error lists the SHA-256
  fingerprint and outcome of each key offered, the server's disconnect
  reason, and why the agent or identity file was skipped. An agent that
  cannot be listed is skipped in favor of the identity file. Agent sockets
  and named pipes have a 30-second I/O timeout; a timed-out Windows pipe
  operation is cancelled and waited for before its buffers are released, and
  an agent I/O error closes the connection for the remaining keys.

Malformed data and limit violations return `InvalidData`; filesystem and
transport errors keep their original error kinds.

## Resource Limits

Each limit belongs to the module that owns the input, and file and transport
readers share one bounded-read helper.

| Input | Limit |
| --- | --- |
| HTTP, Git TCP, or SSH response | 128 MiB (registry fetches choose their own bound, at most 256 MiB) |
| HTTP response headers / each chunk header / trailer section | 64 KiB each |
| Local pack or compressed loose object | 128 MiB |
| Decoded object or delta result | 64 MiB per object |
| Objects in a pack | 1,000,000 |
| Pack decoded objects, delta instructions, and reconstructed results combined | 512 MiB |
| Delta dependency depth | 64 |
| Streamed packet-line advertisement, including flush | 16 MiB |
| Loose reference / packed references | 4 KiB / 16 MiB |
| Symbolic reference / annotated tag dereferences | 8 / 8 |
| SSH identity / known hosts | 1 MiB / 16 MiB |
| SSH agent identities | 1,024 |
| SSH channel incoming payloads, including stderr and control messages | 128 MiB per session |
| SSH channel receive window / maximum data packet | 2 MiB / 32 KiB |
| SSH plaintext key exchange | 1,024 messages / 1 MiB |
| SSH encrypted IGNORE/DEBUG (and banners, during authentication) before a service, auth, or exec response | 1,024 messages / 1 MiB per wait, combined |
| Credential helper output / run time | 64 KiB / 30 s |
| Checkout directory depth / entries | 64 / 1,000,000 |
| Checkout decoded tree and blob bytes, counting repeated objects | 512 MiB |

Byte budgets count decoded payloads. Container and allocator overhead,
compressed input buffers, and transient decoder buffers are additional memory,
so the limits bound accounting rather than exact process memory.

## Known Limits

- Callers must open the checkout destination safely; package fetches open
  cache and staging components without following links. The checks do not
  stop another process from changing a local source repository during
  checkout.
- The SSH client supports only X25519 key exchange, Ed25519 host keys,
  AES-128-CTR, HMAC-SHA2-256, public-key authentication, and one upload-pack
  channel. It does not implement rekeying, host certificates, encrypted
  identity files, or the rest of the SSH connection protocol. Unsupported
  messages and algorithms fail closed. Interoperability is tested against a
  local protocol fixture, not a third-party server.
- Agent and credential-helper behavior depends on local processes and
  operating-system facilities outside Nia's control. At most five identities
  are offered per connection; a sixth agent key is never tried.
- HTTP redirects, proxy authentication, and interim responses are not
  supported.

## Assurance

Parser boundaries are covered by deterministic regressions in their owning
modules, stable property tests, and coverage-guided fuzz targets with minimized
corpora in [`fuzz/corpus`](fuzz/corpus).
[Testing](../../docs/dev/testing.md#untrusted-parser-properties) describes how to run them.
None of these is a proof of absence of defects or external assurance. A change
to parser boundaries, transport state, filesystem output, cryptography, or
resource accounting must extend the regression suite and update this document.
