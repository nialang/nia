# PR #1: Add Option::okOr and Fix Hash Iteration

> Historical proposal. Option conversion APIs are already implemented in
> `lib/std/option.nia`; use current source and tests as the source of truth.

**Status**: Historical proposal; the APIs are implemented in the current tree
**Estimated effort**: 2-3 hours
**Impact**: Unblocks all other refactorings

## Changes

### 1. Add `okOr` to `lib/std/option.nia`

```diff
--- a/lib/std/option.nia
+++ b/lib/std/option.nia
@@ -30,4 +30,16 @@ extend[Value] ?Value {
             null
         }
     }
+
+    pub fun okOr[Error](self, error: Error) Error!Value {
+        if self is ?value {
+            !value
+        } else {
+            error!
+        }
+    }
+
+    pub fun okOrElse[Error](self, supplier: &Fun() Error) Error!Value {
+        if self is ?value { !value } else { supplier()! }
+    }
 }
```

**Test coverage** in `crates/nia-cli/tests/std_option.rs`:

```rust
#[test]
fn option_ok_or_converts_to_error_union() {
    let root = temp_dir("option_ok_or");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::mem;
using std::option;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let some: ?i32 = ?42;
    let none: ?i32 = null;

    if some.okOr(.Invalid) is !value and value == 42 {
        // ok
    } else {
        return process::ExitCode(1)!;
    }

    if none.okOr(.Invalid) is mem::Error::Invalid! {
        // ok
    } else {
        return process::ExitCode(2)!;
    }

    // okOrElse with lazy evaluation
    let noneWithLazy: ?i32 = null;
    if noneWithLazy.okOrElse(\-> .OutOfMemory) is mem::Error::OutOfMemory! {
        // ok
    } else {
        return process::ExitCode(3)!;
    }

    !()
}
"#,
    )
    .unwrap();

    support::build_exe(&main, &exe);
    assert!(Command::new(&exe).assert_success().stdout().is_empty());
}
```

---

### 2. Fix FNV Hash Iteration

```diff
--- a/lib/std/hash/fnv.nia
+++ b/lib/std/hash/fnv.nia
@@ -22,8 +22,8 @@ extend Fnv1a32 {
     }

     pub fun update(&mut self, bytes: &[u8]) () {
-        for index in 0..bytes.len() {
-            self.state = (((self.state ^ bytes[index] as u32) as u64) * 0x01000193u64) as u32;
+        for &byte in bytes {
+            self.state = (((self.state ^ byte as u32) as u64) * 0x01000193u64) as u32;
         }
     }

@@ -54,8 +54,8 @@ extend Fnv1a64 {
     }

     pub fun update(&mut self, bytes: &[u8]) () {
-        for index in 0..bytes.len() {
-            let product = ((self.state ^ bytes[index] as u64) as u128) * 0x100000001b3u128;
+        for &byte in bytes {
+            let product = ((self.state ^ byte as u64) as u128) * 0x100000001b3u128;
             self.state = product as u64;
         }
     }
```

---

### 3. Fix CRC Update Iteration

```diff
--- a/lib/std/checksum/crc.nia
+++ b/lib/std/checksum/crc.nia
@@ -149,8 +149,8 @@ pub struct Crc32C {
 }

 fun updateState(state: &mut u32, bytes: &[u8], table: &[u32]) () {
-    for index in 0..bytes.len() {
-        let slot = ((state.* ^ bytes[index] as u32) & 0xffu32) as usize;
+    for &byte in bytes {
+        let slot = ((state.* ^ byte as u32) & 0xffu32) as usize;
         state.* = (state.* >> 8) ^ table[slot];
     }
 }
```

---

### 4. Update Documentation

```diff
--- a/lib/std/hash/fnv.nia
+++ b/lib/std/hash/fnv.nia
@@ -1,6 +1,11 @@
-// Fowler-Noll-Vo 1a hashers.
+// Fowler-Noll-Vo 1a (FNV-1a) non-cryptographic hash.
 //
-// FNV-1a is a small, deterministic non-cryptographic hash. It is useful for
-// stable file formats and compact indexes; callers should prefer a keyed
-// hasher such as SipHash when the input is attacker-controlled.
+// FNV-1a is a simple hash function suitable for checksums and stable file
+// format identifiers. It has poor avalanche properties and is vulnerable to
+// hash-flooding attacks, so DO NOT use it for hash tables with untrusted input.
+//
+// For hash tables:
+//   - Use SipHash (keyed, DoS-resistant) for untrusted input
+//   - Use XXHash (faster, better distribution) for trusted input
+//
+// FNV is appropriate for protocol magic numbers and non-adversarial checksums.
```

```diff
--- a/lib/std/checksum/crc.nia
+++ b/lib/std/checksum/crc.nia
@@ -1,7 +1,12 @@
-// Reflected CRC-32/ISO-HDLC and CRC-32C/Castagnoli checksums.
+// CRC-32 (ISO-HDLC) and CRC-32C (Castagnoli) checksums.
 //
-// Both variants use the standard reflected polynomials, an all-ones initial
-// state, and a final all-ones xor. A 256-entry lookup table keeps the portable
-// implementation to one table access per byte, with no allocation or hardware
-// dispatch. These are error-detection checksums, not cryptographic hashes.
+// Both are error-detecting checksums suitable for detecting accidental data
+// corruption but NOT for cryptographic authentication. An attacker can construct
+// colliding inputs. Use a keyed hash (SipHash) or HMAC for tamper resistance.
+//
+// CRC-32 (0xedb88320): Standard Ethernet/ZIP/PNG polynomial
+// CRC-32C (0x82f63b78): Better burst-error detection, hardware support on x86
+//
+// This implementation uses a 256-entry lookup table for portable performance.
+// Hardware-accelerated CRC32C (via SSE4.2/ARMv8) may be added in the future.
```

```diff
--- a/lib/std/checksum/adler32.nia
+++ b/lib/std/checksum/adler32.nia
@@ -1 +1,6 @@
-// Adler-32 streaming checksum from RFC 1950.
+// Adler-32 rolling checksum as specified in RFC 1950 (ZLIB compression format).
+//
+// Adler-32 is faster than CRC-32 but has weaker error detection. It is primarily
+// used within ZLIB and other compression formats where speed matters more than
+// detection quality. For standalone checksums, prefer CRC-32 or CRC-32C.
+//
+// Not cryptographically secure—use for error detection only, never authentication.
```

```diff
--- a/lib/std/crypto/hash/sha256.nia
+++ b/lib/std/crypto/hash/sha256.nia
@@ -1,5 +1,11 @@
-// Streaming SHA-256 as specified by FIPS 180-4.
+// SHA-256 cryptographic hash function as specified by FIPS 180-4.
 //
-// The digest is a stable 32-byte value in network byte order. Finalization is
-// non-mutating, so callers may inspect a digest and continue updating state.
+// SHA-256 produces a deterministic 256-bit (32-byte) digest in big-endian order.
+// Digests are collision-resistant, making SHA-256 suitable for:
+//   - File integrity verification
+//   - Digital signatures (with RSA/ECDSA)
+//   - Password derivation (with PBKDF2/scrypt/Argon2)
+//   - Merkle trees and blockchains
+//
+// Finalization is non-mutating: you can inspect an intermediate digest and
+// continue hashing more data with the same state (useful for prefix checking).
```

---

## Commit Message

```
feat(std): add Option::okOr and fix hash iteration

Add functional combinators:
- Option::okOr[Error](error) converts ?T to E!T
- Option::okOrElse[Error](supplier) with lazy evaluation

These enable concise error propagation without match boilerplate:
  value.okOr(.OutOfMemory).?  // vs 5-line match

Fix non-idiomatic patterns in hash/checksum implementations:
- Use `for &byte in bytes` instead of `for index in 0..bytes.len()`
- Follows existing stdlib convention (string.nia, slice.nia)

Improve documentation:
- Clarify security properties (not cryptographic, DoS risks)
- Add usage guidance (when to use each algorithm)
- Remove redundant language ("deterministic", "stable")

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Verification Steps

1. **Run existing tests**: `cargo test --workspace`
2. **Run new test**: `cargo test -p nia-cli --test std_option -- ok_or`
3. **Run hash tests**: `cargo test -p nia-cli --test std_hash_algorithms`
4. **Clippy**: `cargo clippy --workspace --all-targets -- -D warnings`
5. **Check examples**: All examples still compile and run

---

## Next PRs

After this lands:

- **PR #2**: Extract `BlockBuffer[N]` and refactor SHA-256
- **PR #3**: Refactor pool allocator with `okOr`
- **PR #4**: Deduplicate SipHash with const generics
