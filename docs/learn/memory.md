# Memory And Allocators

Nia never allocates behind the program's back. There is no garbage collector,
no destructor, and no hidden allocator: memory comes from an allocator value
that the program creates, passes to every operation that needs it, and tears
down when it is done. This chapter shows how the standard library's allocators,
containers, and owning types fit together. The library conventions are
summarized in the [standard library overview](../../lib/README.md#conventions).

Every program here is complete; build one with `nia emit --exe` and run it to
see the output shown after it.

## Allocators And Owners

An allocator implements the `mem::Allocator` trait. Functions that allocate
take `&mut mem::Allocator`, a trait object, so any allocator can be passed; a
`let mut` allocator is passed by name, and Nia takes its address.
Containers such as `std::ArrayList`, `std::HashMap`, `std::TreeMap`, and
`std::String` do not store the allocator; they take it on every call that may
allocate, grow, or free, and the caller passes the same allocator each time.

A program usually builds a small chain: a `PageAllocator` that gets memory from
the operating system, and a `GeneralPurposeAllocator` on top of it that serves
individual allocations. Owners are released in the reverse order of their
creation, which is exactly the order in which `defer` runs:

```nia
using std::io;
using std::mem;
using std::process;

fun collectSquares(allocator: &mut mem::Allocator, limit: i32) mem::Error!std::ArrayList[i32] {
    let mut squares: std::ArrayList[i32] = .init();
    for value in 1..=limit {
        squares.push(allocator, value * value).?;
    }
    !squares
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut page: mem::PageAllocator = .init();
    let mut allocator: mem::GeneralPurposeAllocator = .init(page);
    defer allocator.deinit().?;          // runs last

    let mut squares = collectSquares(allocator, 4).?;
    defer squares.deinit(allocator).?;

    let mut names: std::HashMap[i32, i32] = .init();
    defer names.deinit(allocator).?;
    for &square in squares {
        _ = names.insert(allocator, square, square * 10).?;
    }

    let live = not allocator.isEmpty();
    io::debugPrint("squares={} names={} live={}\n", [squares.len(), names.len(), live]).?;
    !()
}
```

```text
squares=4 names=4 live=true
```

`collectSquares` returns the list it filled. The list now belongs to `main`,
which releases it with the allocator that produced it. A function that
returns an owner documents that the caller must release it; a function that
only uses a value takes a pointer or a view and leaves responsibility where it
was. These are conventions of the library: the compiler tracks neither owners
nor lifetimes, so keeping a pointer valid is the program's job.

## Working With Collections

The standard library's containers all follow the allocator-argument pattern.
`ArrayList[T]` is a growable array, `HashMap[K, V]` a hash table,
`TreeMap[K, V]` an ordered map, and `String` growable text:

```nia
using std::io;
using std::mem;
using std::option;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page: mem::PageAllocator = .init();
    let mut allocator: mem::GeneralPurposeAllocator = .init(page);
    defer allocator.deinit().?;

    // A growable list.
    let mut words: std::ArrayList[&[char]] = .init();
    defer words.deinit(allocator).?;
    words.push(allocator, "apple").?;
    words.push(allocator, "pear").?;
    words.push(allocator, "fig").?;
    let removed = words.pop();            // ?&[char]: the last element, if any
    let longest = {
        let mut longest: usize = 0;
        for &word in words {
            if word.len() > longest {
                longest = word.len();
            }
        }
        longest
    };

    // Counting with a hash map.
    let mut counts: std::HashMap[i32, i32] = .init();
    defer counts.deinit(allocator).?;
    for &n in &[3, 1, 3, 2, 3][..] {
        let mut slot = counts.getOrInsert(allocator, n, 0).?;
        slot.value().* += 1;
    }
    let threes = if counts.get(3) is ?count { count.* } else { 0 };
    _ = counts.remove(1);

    // Building text.
    let mut text = std::String::init();
    defer text.deinit(allocator).?;
    text.append(allocator, "nia").?;
    text.push(allocator, '!').?;

    io::debugPrint("words={} longest={} popped={} threes={} keys={} text={}\n", [
        words.len(), longest, removed.isPresent(), threes, counts.len(), text.text(),
    ]).?;
    !()
}
```

```text
words=2 longest=5 popped=true threes=3 keys=2 text=nia!
```

A few patterns repeat across all containers:

- `.init()` creates an empty container without allocating, so it cannot fail.
  The first insertion allocates.
- Every call that may allocate takes the allocator and returns an error union,
  because memory can run out.
- Lookups return optional pointers into the container: `get` gives `?&V`, and
  `getMut` gives `?&mut V`. Those pointers are valid only until the container
  changes; see [Views Do Not Own](#views-do-not-own).
- `pop` and `remove` return the removed value, as an optional, because the
  container may have been empty or missing the key.
- `for item in container` iterates in place; `TreeMap` iterates in key order.

`isPresent` and the other optional helpers come from `std::option`, which the
program imports; methods like these are available only where their module is
imported, as [Modules](modules.md#extension-methods) explains.

## Choosing An Allocator

`std::mem` provides these allocators. Each one except `PageAllocator` takes a
child allocator, or a buffer, to get its own memory from.

| Allocator | Use it for | Released by |
| --- | --- | --- |
| `PageAllocator` | whole pages from the operating system; the usual root | nothing to release |
| `GeneralPurposeAllocator` | general allocations of any size, with leak reporting | `deinit` |
| `ArenaAllocator` | many allocations that end together | `deinit`, or `reset` to reuse |
| `FixedBufferAllocator` | allocation from a caller-provided buffer, with no system calls | the buffer's owner |
| `PoolAllocator[T]` | many values of one type that are created and freed often | `deinit` |

An arena reclaims space only when its most recent allocation is freed; the rest
is released together, either by `reset`, which keeps the memory for the next
round, or by `deinit`, which returns it to the child allocator. It suits work
with a clear end, such as handling one request or one build step:

```nia
using std::io;
using std::mem;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut page: mem::PageAllocator = .init();
    let mut arena: mem::ArenaAllocator = .init(page);
    defer arena.deinit().?;

    let mut total: usize = 0;
    for round in 0..3 {
        // Lists allocated from the arena are released by `reset`, not one by one.
        let mut values: std::ArrayList[i32] = .init();
        for value in 0..100 {
            values.push(arena, value + round).?;
        }
        total += values.len();
        arena.reset().?;
    }

    io::debugPrint("total={} used={}\n", [total, arena.used()]).?;
    !()
}
```

```text
total=300 used=0
```

A fixed buffer allocator works without an operating system, which makes it the
allocator for early startup, embedded targets, and tests of out-of-memory
handling. Like an arena, it reclaims only its most recent allocation. When the
buffer is full, allocation fails with
`mem::Error::OutOfMemory`:

```nia
using std::io;
using std::mem;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut buffer: [u8; 64] = [0; 64];
    let mut fixed: mem::FixedBufferAllocator = .init(buffer);

    let mut first = fixed.allocSlice[u8](48).?;
    let second = fixed.allocSlice[u8](32);
    let exhausted = if second is mem::Error::OutOfMemory! { true } else { false };
    let remaining = fixed.remaining();
    first.deinit(fixed).?;

    io::debugPrint("exhausted={} remaining={}\n", [exhausted, remaining]).?;
    !()
}
```

```text
exhausted=true remaining=16
```

## Single Values And Raw Storage

A container is not always needed. `mem::allocValue` stores one value and
returns an `Allocated[T]` owner; `allocSlice[T]` on an allocator returns a
`SliceAllocation[T]` owner for a run of uninitialized elements. Both are
released with `deinit`, and both expose the storage through pointers and
slices that are valid until then:

```nia
using std::io;
using std::mem;
using std::process;

struct Point {
    x: i32,
    y: i32,
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut page: mem::PageAllocator = .init();
    let mut allocator: mem::GeneralPurposeAllocator = .init(page);
    defer allocator.deinit().?;

    let mut point = mem::allocValue(allocator, Point { x: 3, y: 4 }).?;
    defer point.deinit(allocator).?;
    point.valueMut().*.x += 1;

    let mut storage = allocator.allocSlice[u32](4).?;
    defer storage.deinit(allocator).?;
    let slots = storage.asMutSlice();
    for index in 0..slots.len() {
        slots[index] = index as u32 * 2;
    }

    let view = storage.asSlice();
    io::debugPrint("x={} slots={} last={}\n", [point.value().x, view.len(), view[3]]).?;
    !()
}
```

```text
x=4 slots=4 last=6
```

## Leaks And Failed Cleanup

`GeneralPurposeAllocator::deinit` releases everything it still holds and then
reports `mem::Error::Leak` if user allocations were still live. The memory is
gone at that point; the error tells you which allocator was left with owners
that were never released. `deinitWithoutLeakCheck` performs the same cleanup
without the report:

```nia
using std::io;
using std::mem;
using std::process;

fun leakOne() bool {
    let mut page: mem::PageAllocator = .init();
    let mut allocator: mem::GeneralPurposeAllocator = .init(page);
    let forgotten = mem::allocValue(allocator, 42u64);
    _ = forgotten;
    match allocator.deinit() {
        mem::Error::Leak! => true,
        _ => false,
    }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    io::debugPrint("leak reported={}\n", [leakOne()]).?;
    !()
}
```

```text
leak reported=true
```

Every `deinit` and `close` in the standard library can fail, because releasing
memory or a handle can fail. A failed `deinit` attempts every resource the
value owns and keeps the ones it could not release; the value is then
cleanup-only, and calling `deinit` again with the same allocator retries the
rest. [Optionals And Errors](errors.md#cleaning-up) shows how to
write owners with the same behavior.

## Views Do Not Own

A slice, a `PathView`, or a `CStringView` borrows storage that someone else
owns. A view never extends that storage's life, and nothing checks it: a view
into a list becomes invalid when the list grows and moves its elements, and a
view into a released owner points at freed memory. Take views for the duration
of an operation, and take them again after anything that may reallocate:

```nia
using std::io;
using std::mem;
using std::process;

fun sum(values: &[i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut page: mem::PageAllocator = .init();
    let mut allocator: mem::GeneralPurposeAllocator = .init(page);
    defer allocator.deinit().?;

    let mut values: std::ArrayList[i32] = .init();
    defer values.deinit(allocator).?;
    values.push(allocator, 1).?;
    let before = sum(values.asSlice());
    for value in 2..=100 {
        values.push(allocator, value).?;   // may move the elements
    }
    let after = sum(values.asSlice());          // a fresh view

    io::debugPrint("before={} after={}\n", [before, after]).?;
    !()
}
```

```text
before=1 after=5050
```

## Summary

- Memory comes from an allocator that the program creates and passes to every
  call that allocates or frees; containers never store it.
- Register `deinit` with `defer` right after creating an owner, so owners are
  released before the allocator they came from.
- Choose the allocator by how memory is used: general-purpose, arena for work
  with a clear end, fixed buffer without an operating system, pool for many
  values of one type.
- Cleanup can fail and is retried; `GeneralPurposeAllocator` reports leaks.
- Pointers into a container are valid only until it changes.

Next, [Generics And Traits](generics-and-traits.md).
