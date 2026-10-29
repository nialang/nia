# Standard Library Refactoring Examples

> Initial refactoring proposals from the October 2026 review. They are
> exploratory examples rather than normative guidance; check current stdlib
> sources and the language guide before applying a proposal.

Concrete before/after examples demonstrating functional composition patterns.

## Example 1: FNV-1a Hash (Simple)

### Before (Current)

```nia
// From lib/std/hash/fnv.nia:25-27
pub fun update(&mut self, bytes: &[u8]) () {
    for index in 0..bytes.len() {
        self.state = (((self.state ^ bytes[index] as u32) as u64) * 0x01000193u64) as u32;
    }
}
```

### After (Idiomatic)

```nia
pub fun update(&mut self, bytes: &[u8]) () {
    for &byte in bytes {
        self.state = (((self.state ^ byte as u32) as u64) * 0x01000193u64) as u32;
    }
}
```

**Difference**: Direct element iteration, no manual indexing.

---

## Example 2: Pool Allocator Validation (Complex)

### Before (Current — 50 lines)

```nia
// From lib/std/mem/pool_allocator.nia:77-120
if not objectLayout.isEmpty() {
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
    let metadataEnd = match slotMetadataOffset.checkedAdd(slotMetadataSize) {
        ?value => value,
        null => { return .OutOfMemory!; },
    };
    slotsOffset = match metadataEnd.alignForward(objectLayout.align()) {
        ?value => value,
        null => { return .OutOfMemory!; },
    };
    let slotBytes = match slotStride.checkedMul(slotsPerPage) {
        ?value => value,
        null => { return .OutOfMemory!; },
    };
    pageSize = match slotsOffset.checkedAdd(slotBytes) {
        ?value => value,
        null => { return .OutOfMemory!; },
    };
}
```

### After (With `okOr` combinator — 15 lines)

**Step 1**: Add helper to `lib/std/option.nia`:

```nia
extend[Value] ?Value {
    pub fun okOr[Error](self, error: Error) Error!Value {
        if self is ?value { !value } else { error! }
    }
}
```

**Step 2**: Refactor pool allocator:

```nia
if not objectLayout.isEmpty() {
    slotStride = objectLayout.size()
        .alignForward(objectLayout.align())
        .okOr(.OutOfMemory).?;

    slotMetadataOffset = root::builtin::size[PoolPage]()
        .alignForward(slotAlign)
        .okOr(.OutOfMemory).?;

    slotMetadataSize = slotSize.checkedMul(slotsPerPage)
        .okOr(.OutOfMemory).?;

    let metadataEnd = slotMetadataOffset.checkedAdd(slotMetadataSize)
        .okOr(.OutOfMemory).?;

    slotsOffset = metadataEnd.alignForward(objectLayout.align())
        .okOr(.OutOfMemory).?;

    let slotBytes = slotStride.checkedMul(slotsPerPage)
        .okOr(.OutOfMemory).?;

    pageSize = slotsOffset.checkedAdd(slotBytes)
        .okOr(.OutOfMemory).?;
}
```

**Reduction**: 50 lines → 15 lines (70% shorter)
**Readability**: Clear "unwrap or error" pattern throughout

---

## Example 3: Pool Allocator Cleanup (Resource Management)

### Before (Current — 40 lines)

```nia
// From lib/std/mem/pool_allocator.nia:263-327
fun allocatePage(&mut self) Error!() {
    let layout = Layout::init(self.pageSize, self.pageAlign).?;
    let backing = self.child.alloc(layout).?;
    let base = backing.ptr() as usize;
    if backing.size() < self.pageSize {
        self.releaseUnpublished(backing);
        return .Invalid!;
    }
    let pageEnd = match base.checkedAdd(self.pageSize) {
        ?value => value,
        null => {
            self.releaseUnpublished(backing);
            return .OutOfMemory!;
        },
    };
    let slotsAddress = match base.checkedAdd(self.slotsOffset) {
        ?value => value,
        null => {
            self.releaseUnpublished(backing);
            return .OutOfMemory!;
        },
    };
    let metadataAddress = match base.checkedAdd(self.slotMetadataOffset) {
        ?value => value,
        null => {
            self.releaseUnpublished(backing);
            return .OutOfMemory!;
        },
    };
    // ... 6 more identical patterns
}
```

### After (With `inspectError` — 20 lines)

```nia
fun allocatePage(&mut self) Error!() {
    let layout = Layout::init(self.pageSize, self.pageAlign).?;
    let backing = self.child.alloc(layout).?;

    // Any error from here triggers cleanup
    let pageLayout = self.validatePageLayout(backing)
        .inspectError(\_ -> self.releaseUnpublished(backing))
        .?;

    self.publishPage(pageLayout);
    !()
}

fun validatePageLayout(&self, backing: Block) Error!PageLayout {
    let base = backing.ptr() as usize;

    if backing.size() < self.pageSize { return .Invalid!; }

    let pageEnd = base.checkedAdd(self.pageSize).okOr(.OutOfMemory).?;
    let slotsAddress = base.checkedAdd(self.slotsOffset).okOr(.OutOfMemory).?;
    let metadataAddress = base.checkedAdd(self.slotMetadataOffset).okOr(.OutOfMemory).?;
    let metadataEnd = metadataAddress.checkedAdd(self.slotMetadataSize).okOr(.OutOfMemory).?;
    let slotsEnd = slotsAddress.checkedAdd(slotDataSize).okOr(.OutOfMemory).?;

    if base % self.pageAlign != 0
        or metadataAddress < headerEnd
        or metadataEnd > pageEnd
        or slotsAddress < metadataEnd
        or slotsEnd > pageEnd
    {
        return .InvalidAlignment!;
    }

    !PageLayout { backing, metadataAddress, slotsAddress, ... }
}
```

**Benefits**:
1. **Single cleanup point**: `inspectError` handles release once
2. **Separated concerns**: Validation logic is pure, cleanup is isolated
3. **Easier to test**: `validatePageLayout` is a pure function
4. **Better errors**: Can return specific error types per validation

---

## Example 4: SHA-256 Buffering (Abstraction)

### Before (Current — 26 lines)

```nia
// From lib/std/crypto/hash/sha256.nia:201-226
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

### After (With `BlockBuffer` helper — 5 lines)

**Step 1**: Add to `lib/std/mem/block_buffer.nia`:

```nia
pub struct BlockBuffer[const BLOCK_SIZE: usize] {
    buffer: [u8; BLOCK_SIZE],
    len: usize,
}

extend[const N: usize] BlockBuffer[N] {
    pub fun init() BlockBuffer[N] {
        .{ buffer: [0; N], len: 0 }
    }

    pub fun process(&mut self, bytes: &[u8], handler: &Fun(&[u8]) ()) () {
        let mut remaining = bytes;

        // Fill partial block first
        if self.len > 0 {
            let needed = N - self.len;
            if remaining.len() < needed {
                self.append(remaining);
                return;
            }
            self.append(&remaining[0..needed]);
            handler(&self.buffer);
            self.len = 0;
            remaining = &remaining[needed..];
        }

        // Process full blocks directly
        while remaining.len() >= N {
            handler(&remaining[0..N]);
            remaining = &remaining[N..];
        }

        // Buffer remainder
        if remaining.len() > 0 {
            self.append(remaining);
        }
    }

    fun append(&mut self, bytes: &[u8]) () {
        _ = (&mut self.buffer[self.len..(self.len + bytes.len())]).copyFrom(bytes);
        self.len += bytes.len();
    }
}
```

**Step 2**: Refactor SHA-256:

```nia
pub struct Sha256 {
    state: [u32; 8],
    totalLen: u64,
    blockBuffer: BlockBuffer[64],
}

extend Sha256 {
    pub fun init() Sha256 {
        .{
            state: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19],
            totalLen: 0,
            blockBuffer: BlockBuffer[64]::init(),
        }
    }

    pub fun update(&mut self, bytes: &[u8]) () {
        self.totalLen += bytes.len() as u64;
        self.blockBuffer.process(bytes, \block -> {
            compress(&mut self.state[..], block);
        });
    }
}
```

**Benefit**: SHA-256, XXHash, SipHash now share buffering logic. Bug fixes apply once.

---

## Example 5: SipHash Deduplication

### Before (Current — 200+ lines duplicated)

```nia
// lib/std/hash/siphash.nia has TWO identical structs:
pub struct SipHash13 {
    v0: u64, v1: u64, v2: u64, v3: u64,
    totalLen: usize, tail: [u8; 8], tailLen: usize,
}

pub struct SipHash24 {
    v0: u64, v1: u64, v2: u64, v3: u64,
    totalLen: usize, tail: [u8; 8], tailLen: usize,
}

// With 100+ lines of IDENTICAL update() logic each
```

### After (Generic with const parameters)

```nia
pub struct SipHash[const C: usize, const D: usize] {
    v0: u64, v1: u64, v2: u64, v3: u64,
    totalLen: usize,
    blockBuffer: BlockBuffer[8],
}

pub type SipHash13 = SipHash[1, 3];
pub type SipHash24 = SipHash[2, 4];

extend[const C: usize, const D: usize] SipHash[C, D] {
    pub fun init(key0: u64, key1: u64) SipHash[C, D] {
        .{
            v0: key0 ^ 0x736f6d6570736575u64,
            v1: key1 ^ 0x646f72616e646f6du64,
            v2: key0 ^ 0x6c7967656e657261u64,
            v3: key1 ^ 0x7465646279746573u64,
            totalLen: 0,
            blockBuffer: BlockBuffer[8]::init(),
        }
    }

    fun compress(&mut self, word: u64) () {
        self.v3 ^= word;
        for _ in 0..C {  // Unrolled at compile time
            self.round();
        }
        self.v0 ^= word;
    }

    pub fun update(&mut self, bytes: &[u8]) () {
        self.totalLen = (self.totalLen + bytes.len()) % 256;
        self.blockBuffer.process(bytes, \block -> {
            self.compress(read8(block));
        });
    }

    pub fun finalize(&self) u64 {
        let mut state = (self.v0, self.v1, self.v2, self.v3);
        // ... finalization logic
        for _ in 0..D {  // Unrolled at compile time
            round(&mut state.0, &mut state.1, &mut state.2, &mut state.3);
        }
        state.0 ^ state.1 ^ state.2 ^ state.3
    }
}
```

**Result**: One implementation, two type aliases. Zero duplication.

---

## Example 6: CRC Update (Tiny but Important)

### Before (Current)

```nia
// From lib/std/checksum/crc.nia:150-155
fun updateState(state: &mut u32, bytes: &[u8], table: &[u32]) () {
    for index in 0..bytes.len() {
        let slot = ((state.* ^ bytes[index] as u32) & 0xffu32) as usize;
        state.* = (state.* >> 8) ^ table[slot];
    }
}
```

### After (Idiomatic)

```nia
fun updateState(state: &mut u32, bytes: &[u8], table: &[u32]) () {
    for &byte in bytes {
        let slot = ((state.* ^ byte as u32) & 0xffu32) as usize;
        state.* = (state.* >> 8) ^ table[slot];
    }
}
```

**Why**: Teaches users the right pattern from the start.

---

## Example 7: Iterator Chaining (Currently Missing)

### What Could Be (But Isn't)

```nia
// Hash multiple buffers efficiently
let digest = bytes.iter()
    .chain(moreBytes.iter())
    .fold(Sha256::init(), \hasher, &byte -> {
        hasher.update(&[byte]);
        hasher
    })
    .finalize();

// Or with a streaming API:
let hasher = [&buffer1, &buffer2, &buffer3].iter()
    .fold(Sha256::init(), \h, buf -> {
        h.update(buf.*);
        h
    });
```

This pattern exists in Rust/Zig but is absent from Nia's new code.

---

## Summary Table

| Pattern | Before (Lines) | After (Lines) | Savings | Abstraction Used |
|---------|----------------|---------------|---------|------------------|
| FNV loop | 3 | 3 | 0% | for-element |
| Pool validation | 50 | 15 | **70%** | `okOr` |
| Pool cleanup | 40 | 20 | **50%** | `inspectError` |
| SHA-256 buffering | 26 | 5 | **80%** | `BlockBuffer[N]` |
| SipHash duplication | 200+ | 50 | **75%** | const generics |
| CRC update | 3 | 3 | 0% | for-element |

**Total impact**: ~250 lines removed, patterns made reusable.

---

## Implementation Order

### Phase 1: Foundation (Week 1)
1. Add `okOr` / `okOrElse` to `option.nia`
2. Convert hash loops to for-element iteration
3. Document the pattern in stdlib guide

### Phase 2: Abstraction (Week 2)
4. Extract `BlockBuffer[N]` to `mem/block_buffer.nia`
5. Refactor SHA-256, XXHash, SipHash to use it
6. Add `inspectError` usage examples

### Phase 3: Consolidation (Week 3)
7. Deduplicate SipHash with const generics
8. Refactor pool allocator with new combinators
9. Audit entire stdlib for similar patterns

### Phase 4: Documentation (Week 4)
10. Add "Functional Composition" guide
11. Update code review checklist
12. Add lints for imperative anti-patterns
