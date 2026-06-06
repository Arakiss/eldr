//! Hand-written cryptographic primitives — zero crates.
//!
//! - [`sha256`] SHA-256 (FIPS 180-4), the portable reference + test oracle.
//! - [`cc`]     CommonCrypto SHA-256 (hardware-accelerated) — what the scrubber runs.

pub mod cc;
pub mod sha256;
