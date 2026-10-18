//! Host-side harness for the kernel's protocol layers that have no device to
//! run against in the emulator (WPA2 keys, Bluetooth HCI, TPM 2.0): the same
//! source files, run against published vectors.
#![allow(dead_code)]

#[path = "../../../kernel/src/wpa.rs"]
pub mod wpa;

#[path = "../../../kernel/src/bthci.rs"]
pub mod bthci;

#[path = "../../../kernel/src/tpm.rs"]
pub mod tpm;

#[path = "../../../kernel/src/microcode.rs"]
pub mod microcode;
