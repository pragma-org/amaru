//! Crate specific constants

/// `suite_string` of `ECVRF-ED25519-SHA512-Elligator2`
pub const SUITE: &[u8] = &[4u8];
/// `ZERO` used as a domain separator
pub const ZERO: &[u8] = &[0u8];
/// `ONE` used as a domain separator
pub const ONE: &[u8] = &[1u8];
/// `TWO` used as a domain separator
pub const TWO: &[u8] = &[2u8];
/// `THREE` used as a domain separator
pub const THREE: &[u8] = &[3u8];
/// Byte size of the secret seed
pub const SEED_SIZE: usize = 32;
/// Byte size of the public key
pub const PUBLIC_KEY_SIZE: usize = 32;
/// Byte size of the output of the VRF function
pub const OUTPUT_SIZE: usize = 64;
