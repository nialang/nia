# Modules And Visibility

As a program grows, it is split into modules. Each `.nia` file is one module,
modules form a tree, and `using` brings names from one module into another.
This chapter covers loading modules, importing names, controlling visibility,
and the one rule that surprises newcomers: extension methods need an import.

## Files Are Modules

A file loads a child module with `module name;`. Take this program:

```text
main.nia
geometry.nia
geometry/shapes.nia
text.nia
```

`main.nia` is the root. It declares its children, which sit next to it:

```nia ignore
module geometry;
module text;

using std::io;
using std::process;
using geometry::{Point, origin};
using geometry::shapes::Rect;
using text;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let corner = Point { x: 3, y: 4 };
    let rect = Rect { min: origin(), max: corner };
    let count = "banana".countOf('a');
    io::debugPrint("area={} count={}\n", [rect.area(), count]).?;
    !()
}
```

`geometry.nia` has a child of its own, which lives in a directory named after
it, `geometry/shapes.nia`:

```nia ignore
pub module shapes;

pub struct Point {
    x: i32,
    y: i32,
}

pub fun origin() Point {
    Point { x: 0, y: 0 }
}

// Private: visible only inside this module.
fun clamp(value: i32) i32 {
    if value < 0 { 0 } else { value }
}

// Visible to every module in this module tree.
pub(root) fun span(low: i32, high: i32) i32 {
    clamp(high - low)
}
```

```nia ignore
// geometry/shapes.nia
using super::{Point, span};

pub struct Rect {
    min: Point,
    max: Point,
}

extend Rect {
    pub fun area(&self) i32 {
        span(self.min.x, self.max.x) * span(self.min.y, self.max.y)
    }
}
```

```nia
// text.nia: an extension method on a type this module does not own.
extend [char] {
    pub fun countOf(&self, target: char) usize {
        let mut count: usize = 0;
        for &ch in self {
            if ch == target {
                count += 1;
            }
        }
        count
    }
}
```

```text
area=12 count=3
```

Build it from its root file: `nia emit --exe main.nia -o main`. The rules for
where a child's file lives:

- Children of a tree's root file sit beside it. The root is the file you pass
  to `nia emit`, or a package's `src/main.nia`, `lib/root.nia`, or test file.
- Children of any other module live in a directory named after that module:
  `geometry.nia`'s child `shapes` is `geometry/shapes.nia`.
- Every module is loaded explicitly; `using` never discovers files.

`pub module shapes;` makes the child part of `geometry`'s public surface, so
other modules can name `geometry::shapes`. A plain `module` declaration keeps
the child private to its parent.

## Importing Names

`using` makes names from another module available. A path starts at a module
in scope or at one of these roots:

| Root | Names |
| --- | --- |
| `std` | the standard library |
| a dependency name | that package's library, from `nia.toml` |
| a child module's name | that child, as in `using geometry::Point;` |
| `root` | the root of the current module tree |
| `self`, `super` | the current module, its parent |

```nia ignore
using std::process;                     // a module, used as `process::Init`
using geometry::Point;                  // one item
using geometry::{Point, origin};        // several items
using geometry::origin as zero;         // renamed
using geometry::*;                      // every public item
using geometry::shapes::Rect;           // through a child module
using palette::Color::*;                // every variant of an enum
```

Importing a module and naming items through it, `process::Init`, keeps it
clear where a name comes from; the standard library and these chapters mostly
do that. A `using` inside a block applies only within that block, and imports
names only; extension methods need a `using` at the top of the file.

## Visibility

Items are private to their module unless marked:

| Modifier | Visible to |
| --- | --- |
| none | the declaring module |
| `pub(super)` | the parent module and its descendants |
| `pub(root)` | every module of the same module tree |
| `pub` | every module that can name it |

Visibility applies to functions, types, traits, constants, statics, modules,
and to the members of `extend` blocks, each marked individually. Struct fields
have no visibility of their own: they are visible wherever the struct is. In a
package, the library and each executable are separate trees, so `pub(root)`
in the library stays inside the library.

## Extension Methods

Methods come in two kinds, and they become visible differently.

A *trait implementation*, `extend Type : Trait`, needs no import: its methods,
such as `values.len()`, and operators like `a + b` work wherever the value
does.

A plain `extend` block whose target is a type from somewhere else adds
*extension methods*, like `countOf` on `[char]` above. These are visible only
where their module is imported. Without `using text;`, the call fails:

```text
error[E0201]: unknown method `countOf`
help: check the method name or use `using` to import the module that declares the extension method `countOf`
```

The standard library uses extension methods for many everyday operations, so
it is worth knowing their modules:

| Methods | Import |
| --- | --- |
| slice helpers: `get`, `equals`, `copyFrom`, `startsWith`, `sort`, ... | `using std::slice;` |
| iterator adapters: `map`, `fold`, `count`, `position`, `forEach`, ... | `using std::iter;` |
| optional helpers: `okOr`, `isPresent`, `isNull`, `map`, ... | `using std::option;` |
| error-union helpers: `mapError`, `orElse`, `isError`, `map`, ... | `using std::result;` |
| `cleanupAfter` | `using std::error;` |
| text parsing: `parse[T]`, `parseRadix[T]` on `&[char]` and `&[u8]` | `using std::parse;` |
| writer `print` | `using std::fmt::template;` |

A module's own imports stay private: importing `std::io` does not bring in the
modules `std::io` itself uses. A method declared in the type's own module, such
as `Rect::area` above or `ArrayList::push`, is part of the type and needs no
extra import.

## Re-Exports

`pub using` makes names part of the current module's public surface. A module
that presents a curated API, a *facade*, re-exports what its users need:

```nia ignore
// api.nia
using root::geometry;
using root::text;

pub using geometry::{Point, origin};   // items
pub using text;                        // a module, with its extension methods
```

Users can then write `using api::Point;` without knowing where `Point` is
declared. Re-exporting a module also re-exports its extension methods, so
importing `api` makes `countOf` available. Declaring a module `pub module` is
not enough for that; the facade must `pub using` it.

A package's library entry is such a facade for the whole package. The
[packages guide](../guides/packages.md#how-the-parts-fit-together) shows how
executables and other packages reach it.

## Summary

- A file is a module; `module name;` loads a child, beside a root file or in a
  directory named after its parent.
- `using` imports modules and items from `std`, dependencies, children, `root`,
  `self`, and `super`; nothing is imported implicitly.
- Items are private unless marked `pub`, `pub(super)`, or `pub(root)`.
- Trait methods are visible with their trait; extension methods need a `using`
  of their module or of a module that re-exports it.
- `pub using` builds facades, including for extension methods.

The reference covers [modules](../reference/language.md#modules-and-visibility)
in full. Next, [Testing](testing.md).
