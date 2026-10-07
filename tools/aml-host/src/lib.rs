//! Host-side harness for the kernel's AML interpreter: the same source file,
//! run against a DSDT captured from QEMU.
#![allow(dead_code)]

#[path = "../../../kernel/src/aml.rs"]
pub mod aml;

#[path = "../../../kernel/src/acpi_devices.rs"]
pub mod acpi_devices;

#[path = "../../../kernel/src/ec.rs"]
pub mod ec;
