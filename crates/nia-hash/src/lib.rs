// SPDX-License-Identifier: GPL-3.0-or-later
//! Fast deterministic hash building blocks for compiler identity maps.

use std::hash::{BuildHasherDefault, Hasher};

const FAST_HASH_MULTIPLIER: u64 = 0x517c_c1b7_2722_0a95;

#[derive(Debug, Clone, Copy)]
/// Compact non-cryptographic hasher used for internal map/set keys.
pub struct FastHasher {
    hash: u64,
}

impl Default for FastHasher {
    fn default() -> Self {
        Self {
            hash: 0xcbf2_9ce4_8422_2325,
        }
    }
}

impl FastHasher {
    #[inline]
    fn write_u64_value(&mut self, value: u64) {
        self.hash ^= value;
        self.hash = self.hash.rotate_left(5).wrapping_mul(FAST_HASH_MULTIPLIER);
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut value = 0u64;
            for (index, byte) in chunk.iter().enumerate() {
                value |= u64::from(*byte) << (index * 8);
            }
            self.write_u64_value(value);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.write_u64_value(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.write_u64_value(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.write_u64_value(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.write_u64_value(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.write_u64_value(i as u64);
    }
}

/// BuildHasher adapter for [`FastHasher`].
pub type FastBuildHasher = BuildHasherDefault<FastHasher>;
/// Hash map using [`FastBuildHasher`].
pub type FastHashMap<K, V> = std::collections::HashMap<K, V, FastBuildHasher>;
/// Hash set using [`FastBuildHasher`].
pub type FastHashSet<T> = std::collections::HashSet<T, FastBuildHasher>;
