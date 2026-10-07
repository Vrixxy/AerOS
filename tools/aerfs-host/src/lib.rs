//! Host-side harness for AerFS: the kernel's source file with a RAM device
//! that can lose power in the middle of a write.
#![allow(dead_code)]

#[path = "../../../kernel/src/aerfs.rs"]
pub mod aerfs;
