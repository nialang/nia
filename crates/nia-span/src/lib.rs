// SPDX-License-Identifier: GPL-3.0-or-later
//! Half-open source ranges used by diagnostics and syntax products.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
/// A half-open byte/character source range `[start, end)`.
pub struct Span {
    /// Inclusive range start.
    pub start: usize,
    /// Exclusive range end.
    pub end: usize,
}

impl Span {
    /// Creates a span from inclusive start and exclusive end offsets.
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Returns the saturating length of the half-open range.
    pub const fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Reports whether start and end offsets are equal.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}
