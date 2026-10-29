# Standard Library Code Review — October 28, 2026

> Historical first-pass review. Recommendations below are exploratory and
> should be checked against current Nia semantics and stdlib implementations.

This review covers the recent standard library additions:
- Hash algorithms (FNV-1a, SipHash, XXHash)
- Checksum algorithms (CRC-32, Adler-32)
- Cryptographic hash (SHA-256)
- Pool allocator

## Executive Summary

The implementations are functionally correct and well-tested, but they exhibit several **non-idiomatic patterns** that undermine their value as reference code. The standard library should exemplify Nia's unique features—particularly its iterator system, for-in loops, and explicit error handling—but these implementations read like **direct C ports** rather than native Nia code.

**Key Issues:**
1. **C-style manual indexing** instead of iterators and for-in loops
2. **Repetitive boilerplate** across similar implementations
3. **Mixed abstraction levels** (manual wrapping arithmetic scattered throughout)
4. **Documentation inconsistencies** and awkward phrasing

---

## Critical Issue: C-Style Manual Indexing

### Problem

The hash/checksum implementations use manual index loops extensively:

```nia
// From fnv.nia
for index in 0..bytes.len() {
    self.state = (((self.state ^ bytes[index] as u32) as u64) * 0x01000193u64) as u32;
}
```

```nia
// From sha256.nia
let mut index: usize = 0;
while index < 16 {
    schedule[index] = readBig32(&block[(index * 4)..]);
    index += 1;
}
```

```nia
// From crc.nia
for index in 0..bytes.len() {
    let slot = ((state.* ^ bytes[index] as u32) & 0xffu32) as usize;
    state.* = (state.* >> 8) ^ table[slot];
}
```

### Why This Is Wrong

Nia provides **direct iteration over slice elements**:

```nia
for &byte in bytes {
    self.state = (((self.state ^ byte as u32) as u64) * 0x01000193u64) as u32;
}
```

This pattern is:
- **More idiomatic**: Follows Nia's iterator-first design
- **Clearer intent**: "for each byte" vs "for each index, then access byte"
- **Less error-prone**: No chance of off-by-one errors or wrong array access
- **Better matches existing stdlib**: See `slice.nia`, `string.nia`

### Evidence from Existing Code

The standard library **already uses** for-element iteration correctly:

```nia
// From string.nia:70
for scalar in view {
    out.text.appendAssumeCapacity(scalar);
}
```

```nia
// From iter.nia:100
while true {
    if iter.next() is ?value {
        callback(value);
    } else {
        break;
    }
}
```

The hash implementations should follow this established pattern, not introduce C idioms.

---

## Issue: Unnecessary Wrapping Arithmetic Helpers

### Problem

```nia
// From sha256.nia
fun addWrap32(left: u32, right: u32) u32 {
    ((left as u64) + (right as u64)) as u32
}

// Used everywhere:
a = addWrap32(t1 as u32, t2 as u32);
```

```nia
// From xxhash.nia
fun addWrap64(left: u64, right: u64) u64 {
    ((left as u128) + (right as u128)) as u64
}

fun subWrap64(left: u64, right: u64) u64 {
    ((left as u128) + (1u128 << 64) - (right as u128)) as u64
}

fun mulWrap64(left: u64, right: u64) u64 {
    ((left as u128) * (right as u128)) as u64
}
```

### Why This Is Wrong

1. **Nia has explicit wrapping semantics**: Integer overflow traps at runtime. If wrapping is needed for the algorithm (which it is), the code should **document this once** rather than wrap every operation individually.

2. **Performance overhead**: Every arithmetic operation pays for the cast→widen→operate→narrow→cast chain. For hot loops processing megabytes of data, this adds up.

3. **Obscures algorithm structure**: Reading `addWrap32(addWrap32(a, b), c)` is harder than `a + b + c` with documented wrap semantics.

### Better Approach

**Option 1**: Use checked arithmetic with explicit comment:

```nia
// SHA-256 uses wrapping 32-bit addition per FIPS 180-4 §6.2.2
fun compress(hashState: &mut [u32], block: &[u8]) () {
    // All additions in this function wrap modulo 2^32
    let t1 = (h as u64 + bigSigma1(e) as u64 + ...) as u32;
    let t2 = (bigSigma0(a) as u64 + majority(a, b, c) as u64) as u32;
    // ...
}
```

**Option 2**: If Nia gains wrapping operators later (`+%`, `*%`), migrate to those.

**Option 3**: For algorithms like XXHash where wrapping is pervasive, add a `// wrapping arithmetic` directive at the top and inline the casts.

---

## Issue: Duplicate Structures (SipHash13 / SipHash24)

### Problem

`SipHash13` and `SipHash24` are **structurally identical**:

```nia
pub struct SipHash13 {
    v0: u64, v1: u64, v2: u64, v3: u64,
    totalLen: usize, tail: [u8; 8], tailLen: usize,
}

pub struct SipHash24 {
    v0: u64, v1: u64, v2: u64, v3: u64,
    totalLen: usize, tail: [u8; 8], tailLen: usize,
}
```

Their `update` methods are **nearly identical** (100+ lines duplicated), differing only in calling `compress13` vs `compress24`.

### Why This Is Wrong

1. **Violates DRY**: Bug fixes must be applied twice
2. **Maintenance burden**: Changes to buffering logic need manual sync
3. **Missed abstraction**: The round count is compile-time data, not a runtime choice

### Better Approach

**Generic SipHash implementation**:

```nia
pub struct SipHash[const COMPRESS_ROUNDS: usize, const FINAL_ROUNDS: usize] {
    v0: u64, v1: u64, v2: u64, v3: u64,
    totalLen: usize, tail: [u8; 8], tailLen: usize,
}

pub type SipHash13 = SipHash[1, 3];
pub type SipHash24 = SipHash[2, 4];

extend[const C: usize, const D: usize] SipHash[C, D] {
    fun compress(&mut self, word: u64) () {
        self.v3 ^= word;
        for _ in 0..C { self.round(); }  // Unrolled at compile time
        self.v0 ^= word;
    }

    pub fun update(&mut self, bytes: &[u8]) () {
        // Single implementation, parameterized by round counts
    }
}
```

**Current workaround** (if const generics aren't ready):
- Extract common `SipHashState` struct
- Move round count to a stored field
- Single `update` implementation

---

## Issue: Inconsistent Naming Conventions

### Problem

Method naming is inconsistent across implementations:

```nia
// fnv.nia, siphash.nia, xxhash.nia
pub fun finishValue(&self) u64 { ... }

// crc.nia, adler32.nia
pub fun finish(&self) u32 { ... }

// sha256.nia
pub fun finish(&self) [u8; 32] { ... }
```

All three provide a `Hasher` trait impl that calls `finish()` → `u64`, but the "native" finish method has different names and return types.

### Why This Is Wrong

1. **Confusing for users**: Why does `Crc32::finish()` return `u32` but `Fnv1a32::finishValue()` return `u32` while its trait `finish()` returns `u64`?

2. **No clear pattern**: When should I call `finish()` vs `finishValue()` vs `digest()`?

### Better Approach

**Establish clear conventions**:

```nia
pub trait Hasher {
    fun write(&mut self, bytes: &[u8]) ();
    fun finish(&self) u64;  // Trait always returns u64
}

// Concrete hashers:
pub fun finalize(&self) <NativeType> { ... }  // Returns native width/format
pub fun hash(bytes: &[u8]) <NativeType> { ... }  // One-shot static method
```

Or:

```nia
pub fun result(&self) <NativeType> { ... }
pub fun digest(bytes: &[u8]) <NativeType> { ... }  // For crypto (SHA-256)
```

Pick **one** and apply it consistently.

---

## Issue: Manual Buffer Management in SHA-256

### Problem

SHA-256's `update` method has 26 lines of manual buffer management:

```nia
pub fun update(&mut self, bytes: &[u8]) () {
    self.totalLen = ((self.totalLen as u128) + (bytes.len() as u128)) as u64;
    let mut index: usize = 0;

    if self.bufferLen > 0 {
        let needed = 64 - self.bufferLen;
        if bytes.len() < needed {
            _ = (&mut self.buffer[self.bufferLen..(self.bufferLen + bytes.len())])
                .copyFrom(bytes);
            self.bufferLen += bytes.len();
            return;
        }
        _ = (&mut self.buffer[self.bufferLen..64]).copyFrom(&bytes[0..needed]);
        compress(&mut self.state[..], &self.buffer[..]);
        self.bufferLen = 0;
        index = needed;
    }

    while bytes.len() - index >= 64 {
        compress(&mut self.state[..], &bytes[index..]);
        index += 64;
    }

    let remaining = &bytes[index..];
    _ = (&mut self.buffer[0..remaining.len()]).copyFrom(remaining);
    self.bufferLen = remaining.len();
}
```

### Why This Is Wrong

1. **Repeated pattern**: XXHash has identical buffering logic (32-byte blocks), SipHash has it (8-byte blocks)
2. **Hard to audit**: Security-critical code should be simple and obvious
3. **Error-prone**: Easy to get slice bounds wrong

### Better Approach

**Extract streaming buffer abstraction**:

```nia
struct BlockBuffer[const BLOCK_SIZE: usize] {
    buffer: [u8; BLOCK_SIZE],
    len: usize,
}

extend[const N: usize] BlockBuffer[N] {
    pub fun update(
        &mut self,
        bytes: &[u8],
        process: &Fun(&[u8]) (),
    ) () {
        if self.len > 0 {
            let needed = N - self.len;
            if bytes.len() < needed {
                self.appendToBuffer(bytes);
                return;
            }
            self.appendToBuffer(&bytes[0..needed]);
            process(&self.buffer);
            self.len = 0;
            self.update(&bytes[needed..], process);
            return;
        }

        let mut pos = 0;
        while pos + N <= bytes.len() {
            process(&bytes[pos..(pos + N)]);
            pos += N;
        }

        if pos < bytes.len() {
            self.appendToBuffer(&bytes[pos..]);
        }
    }
}
```

Then SHA-256's update becomes:

```nia
pub fun update(&mut self, bytes: &[u8]) () {
    self.totalLen += bytes.len() as u64;
    self.blockBuffer.update(bytes, &|block| {
        compress(&mut self.state, block);
    });
}
```

---

## Issue: Pool Allocator Complexity

### Problem

`pool_allocator.nia` is **598 lines** with deeply nested validation logic:

```nia
let metadataAddress = match base.checkedAdd(self.slotMetadataOffset) {
    ?value => value,
    null => {
        self.releaseUnpublished(backing);
        return .OutOfMemory!;
    },
};
let metadataEnd = match metadataAddress.checkedAdd(self.slotMetadataSize) {
    ?value => value,
    null => {
        self.releaseUnpublished(backing);
        return .OutOfMemory!;
    },
};
// ... 10 more of these
```

Every validation returns the same error and releases the same block. This is **40+ lines** of repetitive error handling.

### Why This Is Wrong

1. **Hard to read**: The happy path is buried under error handling
2. **Repetitive**: `match ... { ?v => v, null => { release; return err } }` appears 10+ times
3. **Fragile**: Easy to forget one release or use wrong error

### Better Approach

**Helper macro or early return pattern**:

```nia
fun allocatePage(&mut self) Error!() {
    let layout = Layout::init(self.pageSize, self.pageAlign).?;
    let backing = self.child.alloc(layout).?;

    defer if err! {
        self.releaseUnpublished(backing);
    }

    let base = backing.ptr() as usize;
    let metadataAddress = base.checkedAdd(self.slotMetadataOffset) or return .OutOfMemory!;
    let metadataEnd = metadataAddress.checkedAdd(self.slotMetadataSize) or return .OutOfMemory!;
    let slotsAddress = base.checkedAdd(self.slotsOffset) or return .OutOfMemory!;
    // ... clean linear flow
}
```

Or introduce a `?` operator for `Option[T]` → `Error!T`:

```nia
let metadataAddress = base.checkedAdd(self.slotMetadataOffset).okOr(.OutOfMemory)?;
```

---

## Documentation Issues

### Adler-32

**Current**:
```nia
// Adler-32 streaming checksum from RFC 1950.
```

**Issue**: RFC 1950 specifies the ZLIB format, which *uses* Adler-32, but Adler-32 itself is from RFC 1951 (DEFLATE) Appendix or just "the Adler-32 checksum." Being pedantic, but accuracy matters.

**Suggestion**:
```nia
// Adler-32 rolling checksum as specified in RFC 1950 (ZLIB compression format).
// Adler-32 is a simple checksum algorithm that provides better error detection
// than Fletcher but remains fast for compression applications.
```

### CRC-32

**Current**:
```nia
// Reflected CRC-32/ISO-HDLC and CRC-32C/Castagnoli checksums.
// Both variants use the standard reflected polynomials...
```

**Issue**: "Reflected" is implementation detail (bit order), not user-facing. Also, calling them "error-detection checksums, not cryptographic hashes" is good but should explain *why* users care.

**Suggestion**:
```nia
// CRC-32 (ISO-HDLC) and CRC-32C (Castagnoli) checksums.
//
// Both are error-detecting checksums suitable for detecting accidental data
// corruption but NOT for cryptographic authentication. An attacker can construct
// colliding inputs. Use a keyed hash (SipHash) or HMAC for tamper resistance.
//
// CRC-32 is the standard Ethernet/ZIP/PNG polynomial.
// CRC-32C has better burst-error detection and hardware support on modern CPUs.
```

### FNV-1a

**Current**:
```nia
// FNV-1a is a small, deterministic non-cryptographic hash. It is useful for
// stable file formats and compact indexes; callers should prefer a keyed
// hasher such as SipHash when the input is attacker-controlled.
```

**Issue**: "deterministic" is redundant (all hash functions are deterministic). Also, FNV is *deprecated* for hash tables due to poor avalanche and DoS risk—this should be explicit.

**Suggestion**:
```nia
// Fowler-Noll-Vo 1a (FNV-1a) non-cryptographic hash.
//
// FNV-1a is a simple hash function suitable for checksums and stable file
// format identifiers. It has poor avalanche properties and is vulnerable to
// hash-flooding attacks, so DO NOT use it for hash tables with untrusted input.
// Prefer SipHash (keyed) or XXHash (faster and better distribution) instead.
```

### SHA-256

**Current**:
```nia
// Streaming SHA-256 as specified by FIPS 180-4.
// The digest is a stable 32-byte value in network byte order.
```

**Issue**: "stable" is ambiguous—does it mean "won't change across versions"? Also, should mention streaming vs one-shot usage.

**Suggestion**:
```nia
// SHA-256 cryptographic hash function as specified by FIPS 180-4.
//
// SHA-256 produces a 256-bit (32-byte) digest in big-endian byte order.
// Digests are deterministic and collision-resistant, making SHA-256 suitable
// for integrity checking, digital signatures, and password derivation (with KDF).
//
// Finalization is non-mutating: you can inspect an intermediate digest and
// continue hashing more data with the same state.
```

### Pool Allocator

**Current**:
```nia
// Fixed-size object pool over a child allocator.
// Each backing page stores initialized slot metadata and a free-slot chain...
```

**Issue**: Doesn't explain *when* to use a pool allocator vs general-purpose.

**Suggestion**:
```nia
// Fixed-size object pool allocator for repeated allocations of the same type.
//
// Pools are efficient when you allocate and free many objects of type T:
// - Game entities, AST nodes, event objects
// - Network packet buffers, per-connection state
//
// Pools are NOT efficient for:
// - Mixed sizes (use a slab allocator)
// - Long-lived objects (pool pages remain until deinit)
// - Single large allocations (use the child allocator directly)
//
// Each page holds `slotsPerPage` T-sized slots. Allocation is O(1) from the
// free list. Empty pages remain available for reuse until deinit.
```

---

## Recommendations

### Immediate (Next PR)

1. **Convert manual indexing to for-element loops** in all hash/checksum code
2. **Unify finalization naming**: Pick `finalize()` or `result()` consistently
3. **Fix documentation** in adler32, crc, fnv, sha256, siphash

### Short-Term (Next Week)

4. **Extract shared buffering logic** into `BlockBuffer[N]` helper
5. **Consolidate SipHash variants** with const generics or shared state struct
6. **Simplify pool allocator validation** with error propagation helper

### Long-Term (Future)

7. **Design wrapping arithmetic convention** for the language (RFC/proposal)
8. **Audit all stdlib for C-isms**: Run `grep "for index in 0\\.\\..*\\.len()"` and convert

---

## Testing Note

The test coverage (`std_hash_algorithms.rs`) is **excellent**: reference vectors, streaming behavior, and edge cases are all covered. The *functional correctness* is not in question—this review is about *code quality and idiomaticity*.

---

---

## Addendum: The Functional Composition Problem

After deeper analysis, the issues run **much deeper** than C-style loops. The code systematically **avoids functional composition** despite Nia providing the necessary abstractions.

### Issue: Ignoring Existing Combinators

Nia **already has** functional combinators for `?T` and `E!T`:

```nia
// From lib/std/option.nia
extend[Value] ?Value {
    pub fun map[Mapped, Mapper](self, mapper: Mapper) ?Mapped
    pub fun andThen[Mapped, Next](self, next: Next) ?Mapped
}

// From lib/std/result.nia
extend[Value, Source] Source!Value {
    pub fun map[Mapped, Mapper](self, mapper: Mapper) Source!Mapped
    pub fun andThen[Mapped, Next](self, next: Next) Source!Mapped
    pub fun mapError(self, mapper: &Fun(Source) Target) Target!Value
    pub fun orElse(self, fallback: &Fun(Source) Target!Value) Target!Value
}
```

**These are used correctly elsewhere in the stdlib**:

```nia
// From lib/std/os.nia:290
!status.map(\value: platform::WaitStatus -> WaitStatus::fromPlatform(value))

// From lib/std/os.nia:724
self.platform().stdin()
    .map[FileHandle, _](\handle: platform::FileDescriptor ->
        FileHandle::fromPlatform(handle))
```

### But The New Code Ignores Them Completely

**Pool allocator — 28 identical match blocks**:

```nia
// Current: Lines 78-120 of pool_allocator.nia
slotStride = match objectLayout.size().alignForward(objectLayout.align()) {
    ?value => value,
    null => { return .OutOfMemory!; },
};
slotMetadataOffset = match root::builtin::size[PoolPage]().alignForward(slotAlign) {
    ?value => value,
    null => { return .OutOfMemory!; },
};
slotMetadataSize = match slotSize.checkedMul(slotsPerPage) {
    ?value => value,
    null => { return .OutOfMemory!; },
};
// ... 10 more identical patterns
```

**This should be**:

```nia
// Option 1: Helper function
fun okOrOom[T](opt: ?T) Error!T {
    opt.map(\v -> !v).orElse(\.OutOfMemory!)
}

slotStride = okOrOom(objectLayout.size().alignForward(objectLayout.align())).?;
slotMetadataOffset = okOrOom(root::builtin::size[PoolPage]().alignForward(slotAlign)).?;
slotMetadataSize = okOrOom(slotSize.checkedMul(slotsPerPage)).?;
```

**Option 2: If Nia supported `?` on `Option`**:

```nia
// Pseudocode - requires language extension
fun init(...) Error!PoolAllocator[T] {
    let slotStride = objectLayout.size()
        .alignForward(objectLayout.align())? or .OutOfMemory!;
    let slotMetadataOffset = root::builtin::size[PoolPage]()
        .alignForward(slotAlign)? or .OutOfMemory!;
    // ...
}
```

### The Match Explosion

**Pool allocator**: 28 match expressions (in one function!)
**SHA-256**: 1 match (reasonable)
**SipHash**: 0 matches (all imperative)

The pool allocator is **40% error handling boilerplate** by line count. This is the opposite of Nia's design.

### Problem: No Error Context Propagation

When the pool allocator fails during `allocatePage()`, it calls:

```nia
self.releaseUnpublished(backing);
return .OutOfMemory!;
```

**10 times in sequence**. This pattern appears in:
- Lines 274, 282, 288, 295, 302, 309, 316, 327

Every validation does the same cleanup, but each match arm repeats it verbatim.

**This cries out for an abstraction**:

```nia
fun allocatePage(&mut self) Error!() {
    let layout = Layout::init(self.pageSize, self.pageAlign).?;
    let backing = self.child.alloc(layout).?;

    // Use a closure-based error context
    let page = self.validatePageLayout(backing).inspectError(\_ -> {
        self.releaseUnpublished(backing);
    }).?;

    self.publishPage(page);
    !()
}

fun validatePageLayout(&self, backing: Block) Error!PoolPageLayout {
    let base = backing.ptr() as usize;
    let metadataAddress = base.checkedAdd(self.slotMetadataOffset)
        .okOr(.OutOfMemory!).?;
    let metadataEnd = metadataAddress.checkedAdd(self.slotMetadataSize)
        .okOr(.OutOfMemory!).?;
    // ... validation continues, cleanup happens once
    !layout
}
```

Or using `mapError`:

```nia
self.validatePageLayout(backing)
    .mapError(\_ -> { self.releaseUnpublished(backing); .OutOfMemory })
    .?
```

### The Missing `okOr` / `okOrElse`

Rust has:

```rust
option.ok_or(error)              // Option<T> -> Result<T, E>
option.ok_or_else(|| error())    // Lazy version
```

Nia needs the equivalent:

```nia
extend[Value] ?Value {
    pub fun okOr[Error](self, error: Error) Error!Value {
        if self is ?value { !value } else { error! }
    }

    pub fun okOrElse[Error](self, supplier: &Fun() Error) Error!Value {
        if self is ?value { !value } else { supplier()! }
    }
}
```

Then the pool allocator becomes:

```nia
let slotStride = objectLayout.size()
    .alignForward(objectLayout.align())
    .okOr(.OutOfMemory).?;
```

**One line. No match. No null arm duplication.**

### Why This Matters Beyond "Style"

1. **Error context is lost**: When `allocatePage` returns `.OutOfMemory!`, the user has no idea *which* validation failed. With proper error combinators, you could write:

   ```nia
   base.checkedAdd(self.slotMetadataOffset)
       .okOrElse(\-> .InvalidAlignment("metadata address overflow")!)
   ```

2. **Cannot compose validations**: Want to validate a chain of computations? Each needs its own match. With combinators:

   ```nia
   let pageLayout = base.checkedAdd(metadataOffset)
       .andThen(\addr -> addr.checkedAdd(metadataSize))
       .andThen(\end -> end.alignForward(slotAlign))
       .okOr(.OutOfMemory).?;
   ```

3. **Testing is harder**: Can't easily inject failure at specific validation points when the logic is scattered across 10 identical match arms.

### The Imperative vs Functional Divide

Compare **existing stdlib** (functional):

```nia
// string.nia:70
for scalar in view {
    out.text.appendAssumeCapacity(scalar);
}

// os.nia:290
!status.map(\value -> WaitStatus::fromPlatform(value))
```

With **new code** (imperative):

```nia
// fnv.nia:25
for index in 0..bytes.len() {
    self.state = (((self.state ^ bytes[index] as u32) as u64) * 0x01000193u64) as u32;
}

// pool_allocator.nia:90-95
slotMetadataSize = match slotSize.checkedMul(slotsPerPage) {
    ?value => value,
    null => { return .OutOfMemory!; },
};
```

The new code reads like **C with pattern matching**, not like Nia.

---

## Architectural Recommendations

### 1. Extend Option Combinators (stdlib addition)

Add to `lib/std/option.nia`:

```nia
extend[Value] ?Value {
    pub fun okOr[Error](self, error: Error) Error!Value {
        if self is ?value { !value } else { error! }
    }

    pub fun okOrElse[Error](self, supplier: &Fun() Error) Error!Value {
        if self is ?value { !value } else { supplier()! }
    }

    pub fun expect(self, message: &[char]) Value {
        if self is ?value { value } else {
            // Calls std::process::abort or similar
            panicWith(message)
        }
    }
}
```

### 2. Add Early-Return Helpers (stdlib addition)

```nia
// For the common "unwrap or return error" pattern
extend[Value, Error] Error!Value {
    pub fun orReturn(self) ?Value {
        match self {
            !value => ?value,
            _! => null,  // Caller propagates via .?
        }
    }
}
```

Usage:

```nia
// Instead of:
let x = match computation() {
    !value => value,
    error! => return error!,
};

// Write:
let x = computation().orReturn().?;
```

### 3. Pattern: Railway-Oriented Programming

For validation chains, adopt the railway pattern:

```nia
struct ValidationResult[T, E] {
    value: E!T,
}

extend[T, E] ValidationResult[T, E] {
    pub fun validate(self, check: &Fun(&T) E!()) ValidationResult[T, E] {
        .{ value: self.value.andThen(\v -> check(&v).map(\_ -> v)) }
    }

    pub fun mapValue[U](self, f: &Fun(T) U) ValidationResult[U, E] {
        .{ value: self.value.map(f) }
    }
}
```

Then pool allocator validations become:

```nia
ValidationResult[PoolPageLayout, Error] { value: !PoolPageLayout::empty() }
    .validate(\layout -> checkMetadataAddress(layout))
    .validate(\layout -> checkSlotsAddress(layout))
    .validate(\layout -> checkAlignment(layout))
    .mapValue(\layout -> self.createPage(layout))
    .value.?
```

### 4. Hash Combinator Pattern

For streaming hash functions, extract the common structure:

```nia
trait StreamingHash {
    type State;
    type Output;

    fun init() Self::State;
    fun compress(&mut Self::State, block: &[u8]) ();
    fun finalize(&Self::State) Self::Output;
}

struct BlockingHasher[H, const BLOCK_SIZE: usize]
where H: StreamingHash
{
    state: H::State,
    buffer: BlockBuffer[BLOCK_SIZE],
}

extend[H, const N: usize] BlockingHasher[H, N]
where H: StreamingHash
{
    pub fun update(&mut self, bytes: &[u8]) () {
        self.buffer.process(bytes, \block -> H::compress(&mut self.state, block));
    }

    pub fun finalize(&self) H::Output {
        H::finalize(&self.state)
    }
}
```

Then SHA-256 becomes:

```nia
pub struct Sha256State { /* fields */ }

extend Sha256State : StreamingHash {
    type State = [u32; 8];
    type Output = [u8; 32];

    fun init() [u32; 8] { [0x6a09e667, ...] }
    fun compress(state: &mut [u32; 8], block: &[u8]) () { /* core logic */ }
    fun finalize(state: &[u32; 8]) [u8; 32] { /* padding and output */ }
}

pub type Sha256 = BlockingHasher[Sha256State, 64];
```

**All buffering logic vanishes into the abstraction.**

---

## Revised Priority List

### Immediate (This Week)

1. **Add `okOr` / `okOrElse` to `option.nia`** — 20 lines, unblocks everything
2. **Refactor pool allocator validation** — Use new combinators, cut 200 lines
3. **Convert hash loops to for-element** — Already covered in main review

### Short-Term (Next Sprint)

4. **Extract `BlockBuffer[N]` helper** — Shared by SHA-256, XXHash, SipHash
5. **Document functional patterns** — Add "Functional Composition" section to stdlib guide
6. **Audit for imperative anti-patterns** — Search for `match.*{.*?value.*null.*return`

### Long-Term (Next Quarter)

7. **Design error context system** — How should validation failures carry context?
8. **Streaming hash trait** — `StreamingHash` + `BlockingHasher` abstraction
9. **Railway validation pattern** — Standard approach for multi-step validation

---

## Conclusion

These implementations work, but they don't **teach** Nia. Users learning from this code will write C-style loops, duplicate boilerplate, and miss the iterator system entirely. The standard library must set a higher bar.

**The deeper issue**: The code avoids Nia's functional features—`map`, `andThen`, `orElse`—despite them being **already implemented and used elsewhere in the stdlib**. This creates a pedagogical disaster: new contributors will see two conflicting styles and not know which to follow.

**The goal**: Code that users can read and think, "Ah, *this* is how you write Nia."

**The reality**: Code that users read and think, "Why does this look like C with pattern matching?"
