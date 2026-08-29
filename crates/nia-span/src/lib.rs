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

#[cfg(test)]
mod tests {
    use super::Span;

    #[test]
    fn half_open_ranges_report_length_and_empty_boundaries() {
        let span = Span::new(3, 8);
        assert_eq!(span.len(), 5);
        assert!(!span.is_empty());

        let empty = Span::new(4, 4);
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
    }

    #[test]
    fn reversed_ranges_have_saturating_zero_length() {
        let span = Span::new(9, 2);
        assert_eq!(span.len(), 0);
        assert!(!span.is_empty());
    }
}
