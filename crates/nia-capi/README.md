# nia-capi

`nia-capi` is the C ABI facade for embedding the Nia compiler from non-Rust
tools. The Rust-native compiler API remains `nia-driver`; this crate translates
that API into opaque handles, integer status codes, and explicit ownership
rules.

The current surface is intentionally small:

- query C API ABI version, compiler version, and status names;
- create/free a `NiaSession`;
- inject in-memory source text into a session;
- create/free a `NiaCheckRequest`;
- add module-map entries to a check request;
- set runtime and optimization level;
- check a source root;
- emit a single object file or an object directory;
- emit an executable, optionally with linker options;
- read and free result messages.

The C API is an embedding boundary, not the package-build interface. Nia-owned
build flows should use the Rust toolchain/build crates so compiler API, LLVM,
C runtime, unwind runtime, and dynamic linker details stay inside the toolchain
link plan.

All string inputs are passed as `const uint8_t *` plus byte length and must be
valid UTF-8. Returned `NiaString` values are owned by the caller and must be
released with `nia_string_free`. `NiaResult`, `NiaSession`,
`NiaCheckRequest` and `NiaLinkOptions` are opaque handles and must be released
with their matching free functions.

Every exported function catches Rust panics and Nia internal compiler errors at
the ABI boundary. Functions returning handles return null on boundary failures
that cannot be represented as a `NiaResult`; functions returning `NiaResult`
return `NIA_STATUS_INTERNAL_ERROR` with a rendered message when possible.

The public C declarations live in `include/nia.h`.
