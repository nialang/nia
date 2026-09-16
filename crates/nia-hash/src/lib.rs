// SPDX-License-Identifier: GPL-3.0-or-later
//! Fast deterministic hash building blocks for compiler identity maps.

use std::hash::{BuildHasherDefault, Hasher};

const FAST_HASH_SEED: u64 = 0xcbf2_9ce4_8422_2325;
const FAST_HASH_MULTIPLIER: u64 = 0x517c_c1b7_2722_0a95;
const FAST_HASH_WIDTH_MULTIPLIER: u64 = 0x9e37_79b1_85eb_ca87;

#[derive(Debug, Clone, Copy)]
/// Compact non-cryptographic hasher used for internal map/set keys.
pub struct FastHasher {
    hash: u64,
}

impl Default for FastHasher {
    fn default() -> Self {
        Self {
            hash: FAST_HASH_SEED,
        }
    }
}

impl FastHasher {
    #[inline]
    fn write_scalar(&mut self, value: u64, width: u64) {
        self.hash =
            (self.hash.rotate_left(5) ^ value ^ width.wrapping_mul(FAST_HASH_WIDTH_MULTIPLIER))
                .wrapping_mul(FAST_HASH_MULTIPLIER);
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.write_scalar(
                u64::from_le_bytes(chunk.try_into().expect("exact hash chunk width")),
                8,
            );
        }
        let remainder = chunks.remainder();
        if !remainder.is_empty() {
            let mut tail = [0u8; 8];
            tail[..remainder.len()].copy_from_slice(remainder);
            self.write_scalar(u64::from_le_bytes(tail), remainder.len() as u64);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.write_scalar(u64::from(i), 1);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.write_scalar(u64::from(i), 2);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.write_scalar(u64::from(i), 4);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.write_scalar(i, 8);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.write_scalar(i as u64, std::mem::size_of::<usize>() as u64);
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
        let mut first = FastHasher::default();
        42u32.hash(&mut first);
        let mut second = FastHasher::default();
        42u32.hash(&mut second);
        assert_eq!(first.finish(), second.finish());
    }

    #[test]
    fn byte_length_and_boundaries_contribute_to_the_hash() {
        fn hash(writes: &[&[u8]]) -> u64 {
            let mut hasher = FastHasher::default();
            for bytes in writes {
                hasher.write(bytes);
            }
            hasher.finish()
        }

        assert_ne!(hash(&[b"a"]), hash(&[b"a\0"]));
        assert_ne!(hash(&[b"ab"]), hash(&[b"a", b"b"]));
        assert_ne!(hash(&[b"abcdefgh"]), hash(&[b"abcdefgh\0"]));
    }

    #[test]
    fn scalar_width_contributes_to_the_hash() {
        let mut narrow = FastHasher::default();
        narrow.write_u32(7);
        let mut wide = FastHasher::default();
        wide.write_u64(7);
        assert_ne!(narrow.finish(), wide.finish());
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
