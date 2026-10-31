# Memory and Resource Management

This guide demonstrates idiomatic memory and resource management patterns in Nia, based on standard library practices.

## Core Principles

Nia has no garbage collector, no borrow checker, and no RAII. Instead:
- Allocation is explicit and parameterized by allocator
- Cleanup is explicit with `defer`
- Ownership is documented through API contracts, not enforced by the compiler

## Pattern 1: Allocator as Parameter

Containers take an allocator argument for every operation that may allocate or free:

```nia
fun buildList(allocator: &mut mem::Allocator) mem::Error!ArrayList[i32] {
    let mut list: ArrayList[i32] = .init();
    list.push(allocator, 10).?;
    list.push(allocator, 20).?;
    !list
}
```

Never store an allocator in a container. Operations receive `&mut mem::Allocator` each time.

## Pattern 2: `defer` for Cleanup

Register cleanup immediately after acquisition, in reverse dependency order:

```nia
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    
    let mut page: mem::PageAllocator = .init();
    let mut allocator: mem::GeneralPurposeAllocator = .init(&mut page);
    defer allocator.deinit().?;  // deallocate all memory
    
    let mut values: ArrayList[i32] = .init();
    defer values.deinit(&mut allocator).?;  // release container storage
    
    values.push(&mut allocator, 42).?;
    // ...
    !()
}  // defer runs in reverse: values, then allocator
```

This ensures cleanup happens on any exit path: normal return, `return` statement, or `.?` propagation.

## Pattern 3: Zero-Initialization

Containers initialize with zero capacity and no allocation:

```nia
// From lib/std/collections/array_list/list.nia
pub fun init() ArrayList[T] {
    .{
        items: raw::danglingSlice[T](0),
        capacity: 0,
        storageBlock: null,
        replacementBlock: null,
        aliasBlock: null,
    }
}
```

This allows cheap initialization. First push/insert allocates.

## Pattern 4: Pre-allocation for Known Capacity

When capacity is known upfront, pre-allocate to avoid multiple reallocations:

```nia
let mut list = ArrayList[i32]::initCapacity(&mut allocator, 100).?;
// Now 100 pushes won't reallocate
```

## Pattern 5: Owned vs Borrowed Views

Containers own their storage but provide borrowed views:

```nia
extend[T] ArrayList[T]
where T: Sized
{
    pub fun asSlice(&self) &[T] {
        &self.items[0..self.len()]
    }
    
    pub fun asMutSlice(&mut self) &mut [T] {
        &mut self.items[0..self.len()]
    }
}
```

Callers receive slices, never raw pointers to internal storage.

## Pattern 6: Ownership Transfer

APIs that transfer ownership consume by value and document it:

```nia
// Takes ownership of allocation
pub fun fromOwnedAllocation(allocation: mem::SliceAllocation[T]) ArrayList[T] {
    let len = allocation.len();
    let block = allocation.intoBlock();
    // ... uses block, now owns it
}
```

Methods like `intoOwnedSlice` consume the container and return owned storage:

```nia
let owned = list.intoOwnedSlice(&mut allocator).?;  // list is now empty
defer owned.deinit(&mut allocator).?;
```

## Pattern 7: Assume-Capacity Operations

After ensuring capacity, use assume-capacity variants to skip checks:

```nia
// From lib/std/string.nia
fun fromUtf8View(allocator: &mut mem::Allocator, view: unicode::Utf8View) mem::Error!String {
    let mut out = String::initCapacity(allocator, view.scalarCount()).?;
    for scalar in view {
        out.text.appendAssumeCapacity(scalar);  // capacity guaranteed
    }
    !out
}
```

This is safe when the invariant is maintained by the caller.

## Pattern 8: Storage Block Ownership

Containers retain the storage block separately from logical capacity:

```nia
pub struct ArrayList[T] {
    items: &mut [T],
    capacity: usize,
    storageBlock: ?mem::Block,  // actual allocation owner
    // ...
}
```

This allows empty lists to be reallocated without reconstructing the block from capacity.

## Pattern 9: Multiple Cleanup with Accumulator

When multiple resources need cleanup and each can fail, use `CleanupAccumulator`:

```nia
pub fun deinit(&mut self, allocator: &mut mem::Allocator) mem::Error!() {
    let mut cleanup: error::CleanupAccumulator[mem::Error] = .init();
    cleanup.attempt(self.freeItems(allocator));
    cleanup.attempt(self.freeReplacementItems(allocator));
    cleanup.attempt(self.freeAliasItems(allocator));
    if not cleanup.isClean() {
        return cleanup.finish();
    }
    self.items = raw::danglingSlice[T](0);
    self.capacity = 0;
    self.storageBlock = null;
    !()
}
```

This attempts all cleanup operations and reports the first error, ensuring partial cleanup on failure.

## Pattern 10: Transactional Growth

On reallocation failure, preserve the old allocation rather than leaving the container in an invalid state:

```nia
// From lib/std/collections/array_list/list.nia
fun grow(&mut self, allocator: &mut mem::Allocator, newCapacity: usize) mem::Error!() {
    let newBlock = allocator.alloc(layout).?;
    let mut newItems = raw::blockItems[T](newBlock, newCapacity);
    _ = newItems.copyFrom(self.asSlice());
    
    // Only after successful copy, replace old block
    self.releaseReplacedBlock(allocator, oldBlock, newBlock).?;
    self.items = newItems;
    self.capacity = newCapacity;
    self.storageBlock = ?newBlock;
    !()
}
```

If allocation fails, the original container remains valid.

## Pattern 11: Allocator Hierarchy

Standard library provides a hierarchy of allocators:

```nia
// Low-level: OS memory pages
let mut page: mem::PageAllocator = .init();

// General-purpose: suitable for most containers
let mut general: mem::GeneralPurposeAllocator = .init(&mut page);

// Specialized: arena for batch allocation/free
let mut arena: mem::ArenaAllocator = .init(&mut page);

// Specialized: pool for fixed-size objects
let mut pool: mem::PoolAllocator[Point] = .init(&mut page);
```

Choose based on allocation patterns, not performance guesses.

## Pattern 12: ZST Optimization

Zero-sized types need no allocation:

```nia
if root::builtin::size[T]() == 0 {
    return !.fromBlock(mem::Block::empty(layout), len);
}
```

Standard library checks ZSTs explicitly in allocation paths.

## Pattern 13: Layout Queries

Never hard-code sizes or alignments. Query them:

```nia
let layout = mem::Layout::array[T](len).?;
let block = allocator.alloc(layout).?;
```

`Layout` encapsulates size, alignment, and overflow checking.

## Common Mistakes

**Don't forget `defer`:**
```nia
// Bad: leaks if early return happens
let mut list: ArrayList[i32] = .init();
list.push(&mut allocator, 10).?;
list.deinit(&mut allocator).?;  // unreachable if push fails

// Good: defer ensures cleanup
let mut list: ArrayList[i32] = .init();
defer list.deinit(&mut allocator).?;
list.push(&mut allocator, 10).?;
```

**Don't use slices to recreate ownership:**
```nia
// Bad: slice doesn't carry allocation info
let slice = list.asSlice();
// ... how do you free this later?

// Good: transfer ownership explicitly
let owned = list.intoOwnedSlice(&mut allocator).?;
defer owned.deinit(&mut allocator).?;
```

**Don't allocate implicitly:**
```nia
// There's no: vec![1, 2, 3]
// Write:
let mut list: ArrayList[i32] = .init();
defer list.deinit(&mut allocator).?;
list.push(&mut allocator, 1).?;
list.push(&mut allocator, 2).?;
```

## Summary

Nia's memory management is:
- **Explicit**: every allocation names an allocator
- **Flexible**: choose allocators per use case
- **Predictable**: cleanup order is source order, reversed
- **Compositional**: `defer` works with all control flow

Trust the programmer. Document ownership. Use `defer`.
