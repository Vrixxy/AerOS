//! Host-side harness for the kernel's TLS code: the same source files, run
//! against OpenSSL and published test vectors.
#![allow(dead_code)]

pub mod random {
    pub fn fill(_: &mut [u8]) -> bool {
        false
    }
    pub fn next_u64() -> u64 {
        0
    }
}

#[path = "../../../kernel/src/auth.rs"]
pub mod auth;
#[path = "../../../kernel/src/ed25519.rs"]
pub mod ed25519;
#[path = "../../../kernel/src/tls/mod.rs"]
pub mod tls;
