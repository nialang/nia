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

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{Hash, Hasher};

    #[test]
    fn hasher_is_deterministic_for_bytes_and_scalars() {
        let mut bytes = FastHasher::default();
        bytes.write(b"nia-hash");
        let mut scalars = FastHasher::default();
        scalars.write_u64(u64::from_le_bytes(*b"nia-hash"));
        assert_eq!(bytes.finish(), scalars.finish());

        let mut first = FastHasher::default();
        42u32.hash(&mut first);
        let mut second = FastHasher::default();
        42u32.hash(&mut second);
        assert_eq!(first.finish(), second.finish());
    }

    #[test]
    fn map_and_set_use_the_fast_build_hasher() {
        let mut map = FastHashMap::default();
        map.insert("key", 7u32);
        assert_eq!(map.get("key"), Some(&7));

        let mut set = FastHashSet::default();
        assert!(set.insert("key"));
        assert!(!set.insert("key"));
    }
}
