// SPDX-License-Identifier: GPL-3.0-or-later
//! Fast deterministic hash building blocks for compiler identity maps.

use std::hash::{BuildHasherDefault, Hasher};

const FAST_HASH_SEED: u64 = 0xcbf2_9ce4_8422_2325;
const FAST_HASH_MULTIPLIER: u64 = 0x517c_c1b7_2722_0a95;
const FAST_HASH_WIDTH_MULTIPLIER: u64 = 0x9e37_79b1_85eb_ca87;
const FAST_HASH_BYTES_DOMAIN: u64 = 0xa076_1d64_78bd_642f;
const FAST_HASH_BYTE_CHUNK_DOMAIN: u64 = 0xe703_7ed1_a0b4_28db;
const FAST_HASH_BYTE_TAIL_DOMAIN: u64 = 0x8ebc_6af0_9c88_c6e3;
const FAST_HASH_SCALAR_DOMAIN: u64 = 0x5899_65cc_7537_4cc3;
const FAST_HASH_U128_HIGH_DOMAIN: u64 = 0x1d8e_4e27_c47d_124f;

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
    fn mix(&mut self, value: u64, domain: u64) {
        self.hash = (self.hash.rotate_left(5) ^ value ^ domain).wrapping_mul(FAST_HASH_MULTIPLIER);
    }

    #[inline]
    fn write_scalar(&mut self, value: u64, width: u64) {
        self.mix(
            value,
            FAST_HASH_SCALAR_DOMAIN ^ width.wrapping_mul(FAST_HASH_WIDTH_MULTIPLIER),
        );
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.mix(bytes.len() as u64, FAST_HASH_BYTES_DOMAIN);
        let (chunks, remainder) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.mix(u64::from_le_bytes(*chunk), FAST_HASH_BYTE_CHUNK_DOMAIN);
        }
        if !remainder.is_empty() {
            let mut tail = [0u8; 8];
            tail[..remainder.len()].copy_from_slice(remainder);
            self.mix(
                u64::from_le_bytes(tail),
                FAST_HASH_BYTE_TAIL_DOMAIN
                    ^ (remainder.len() as u64).wrapping_mul(FAST_HASH_WIDTH_MULTIPLIER),
            );
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
    fn write_u128(&mut self, i: u128) {
        self.write_scalar(i as u64, 16);
        self.mix((i >> 64) as u64, FAST_HASH_U128_HIGH_DOMAIN);
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
        assert_ne!(
            hash(&[b"abcdefghijklmnop"]),
            hash(&[b"abcdefgh", b"ijklmnop"])
        );
        assert_ne!(hash(&[]), hash(&[b""]));
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
    fn byte_and_scalar_writes_have_separate_domains() {
        let mut bytes = FastHasher::default();
        bytes.write(&7u64.to_le_bytes());
        let mut scalar = FastHasher::default();
        scalar.write_u64(7);
        assert_ne!(bytes.finish(), scalar.finish());
    }

    #[test]
    fn both_halves_of_u128_contribute_to_the_hash() {
        let mut low = FastHasher::default();
        low.write_u128(7);
        let mut high = FastHasher::default();
        high.write_u128((1u128 << 64) | 7);
        assert_ne!(low.finish(), high.finish());
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
