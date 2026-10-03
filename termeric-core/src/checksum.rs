//! A fast, deterministic, non-cryptographic hash.

/// 64-bit FNV-1a. It detects corruption and changes, never tampering: the
/// graphics relay checksum and Termeric's persisted scrollback names and
/// plugin fingerprints depend on its exact output.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn matches_the_published_fnv1a64_vectors() {
        assert_eq!(super::fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(super::fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(super::fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
