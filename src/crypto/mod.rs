//! Hand-written cryptographic primitives — zero crates.
//!
//! - [`sha256`] SHA-256 (FIPS 180-4), used by the integrity scrubber to fingerprint
//!   files and catch silent corruption (bit rot) that SMART cannot see.

pub mod sha256;
