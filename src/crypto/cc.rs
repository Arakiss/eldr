//! CommonCrypto SHA-256 — the system's hardware-accelerated implementation (the ARMv8
//! SHA extensions), reached by FFI over libSystem. Zero crates: CommonCrypto ships with
//! macOS. The scrubber uses this, where it hashes several times faster than the portable
//! [`super::sha256`]; that hand-rolled one stays as the reference and the test oracle.

use core::ffi::c_void;

// CC_SHA256_CTX from <CommonCrypto/CommonDigest.h>: CC_LONG (u32) count[2], hash[8],
// wbuf[16]. We never read the fields; the layout just has to match for the C calls.
#[repr(C)]
struct CcSha256Ctx {
    count: [u32; 2],
    hash: [u32; 8],
    wbuf: [u32; 16],
}

unsafe extern "C" {
    fn CC_SHA256_Init(c: *mut CcSha256Ctx) -> i32;
    fn CC_SHA256_Update(c: *mut CcSha256Ctx, data: *const c_void, len: u32) -> i32;
    fn CC_SHA256_Final(md: *mut u8, c: *mut CcSha256Ctx) -> i32;
}

/// Streaming SHA-256 over CommonCrypto — same shape as [`super::sha256::Sha256`].
pub struct CcSha256 {
    ctx: CcSha256Ctx,
}

impl Default for CcSha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl CcSha256 {
    pub fn new() -> Self {
        let mut ctx = CcSha256Ctx {
            count: [0; 2],
            hash: [0; 8],
            wbuf: [0; 16],
        };
        unsafe {
            CC_SHA256_Init(&mut ctx);
        }
        CcSha256 { ctx }
    }

    pub fn update(&mut self, data: &[u8]) {
        // CC_SHA256_Update takes a u32 length; feed in bounded chunks so a huge slice
        // can't overflow it. Callers already pass ~64 KiB, so this never actually splits.
        for chunk in data.chunks(1 << 30) {
            unsafe {
                CC_SHA256_Update(
                    &mut self.ctx,
                    chunk.as_ptr() as *const c_void,
                    chunk.len() as u32,
                );
            }
        }
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let mut md = [0u8; 32];
        unsafe {
            CC_SHA256_Final(md.as_mut_ptr(), &mut self.ctx);
        }
        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::sha256;

    #[test]
    fn matches_the_reference_impl() {
        for input in [
            &b""[..],
            b"abc",
            b"The quick brown fox jumps over the lazy dog",
        ] {
            let mut h = CcSha256::new();
            h.update(input);
            assert_eq!(h.finalize(), sha256::hash(input), "mismatch on {input:?}");
        }
    }

    #[test]
    fn streaming_in_odd_chunks_matches_one_shot() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let mut h = CcSha256::new();
        for c in data.chunks(7) {
            h.update(c);
        }
        assert_eq!(h.finalize(), sha256::hash(&data));
    }
}
