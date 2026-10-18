//! The processor performance states the firmware lists (`_PSS`, limited by
//! `_PPC`) and how to move between them: `_PCT` names the control register
//! (an I/O port, memory, or "functional fixed hardware", which on x86 means a
//! model-specific register that depends on the vendor), and `plan` turns a
//! state and that register into the write to make. The write itself cannot be
//! tried without real hardware; the planning is tested on the host.

use crate::aml::{Aml, NodeKind};

pub const MAX_PSTATES: usize = 16;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct PState {
    pub mhz: u32,
    pub milliwatts: u32,
    /// Transition latency in microseconds.
    pub latency: u32,
    pub control: u64,
    pub status: u64,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Table {
    pub states: [PState; MAX_PSTATES],
    pub count: usize,
    /// The first state the firmware currently allows (`_PPC`), 0 when absent.
    pub first_allowed: usize,
}

impl Table {
    pub fn states(&self) -> &[PState] {
        &self.states[..self.count]
    }
}

fn integer(aml: &Aml, package: crate::aml::Value, index: usize) -> Option<u64> {
    aml.package_element(package, index)
        .and_then(|element| aml.integer(element))
}

/// Reads the performance states of the processor object `node`.
pub fn read(aml: &mut Aml, node: u16) -> Option<Table> {
    let method = aml.child_node(node, b"_PSS")?;
    let list = aml.evaluate_node(method, &[]).ok()?;
    let count = aml.package_len(list)?;
    let mut table = Table::default();
    for index in 0..count.min(MAX_PSTATES) {
        let state = aml.package_element(list, index)?;
        let (Some(mhz), Some(power), Some(latency), Some(control), Some(status)) = (
            integer(aml, state, 0),
            integer(aml, state, 1),
            integer(aml, state, 2),
            integer(aml, state, 4),
            integer(aml, state, 5),
        ) else {
            continue;
        };
        table.states[table.count] = PState {
            mhz: mhz as u32,
            milliwatts: power as u32,
            latency: latency as u32,
            control,
            status,
        };
        table.count += 1;
    }
    if table.count == 0 {
        return None;
    }
    if let Some(limit) = aml.child_node(node, b"_PPC")
        && let Ok(value) = aml.evaluate_node(limit, &[])
        && let Some(first) = aml.integer(value)
    {
        table.first_allowed = (first as usize).min(table.count - 1);
    }
    Some(table)
}

/// The first processor object that lists performance states.
pub fn find(aml: &mut Aml) -> Option<(u16, Table)> {
    for index in 0..aml.node_count() {
        let node = index as u16;
        if matches!(aml.node_kind(node), NodeKind::Processor | NodeKind::Device)
            && let Some(table) = read(aml, node)
        {
            return Some((node, table));
        }
    }
    None
}

/// Where `_PCT` says the control value goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Register {
    Memory { address: u64, width: u8 },
    Io { port: u16, width: u8 },
    FixedHardware,
}

/// One write that moves a processor to a state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Switch {
    Memory { address: u64, width: u8, value: u64 },
    Io { port: u16, width: u8, value: u32 },
    Msr { register: u32, value: u64 },
}

pub const MSR_INTEL_PERF_CTL: u32 = 0x199;
pub const MSR_AMD_PSTATE_CONTROL: u32 = 0xc001_0062;

fn generic_register(bytes: &[u8]) -> Option<Register> {
    if bytes.len() < 15 || bytes[0] != 0x82 {
        return None;
    }
    let width = bytes[4];
    let address = u64::from_le_bytes(bytes[7..15].try_into().ok()?);
    match bytes[3] {
        0 => Some(Register::Memory { address, width }),
        1 => Some(Register::Io {
            port: u16::try_from(address).ok()?,
            width,
        }),
        0x7f => Some(Register::FixedHardware),
        _ => None,
    }
}

/// The register `_PCT` gives for writing a new state.
pub fn control_register(aml: &mut Aml, node: u16) -> Option<Register> {
    let method = aml.child_node(node, b"_PCT")?;
    let pair = aml.evaluate_node(method, &[]).ok()?;
    let control = aml.package_element(pair, 0)?;
    generic_register(aml.buffer(control)?)
}

/// The write that selects `state` through `register`; `intel` picks the
/// model-specific register for functional fixed hardware.
pub fn plan(register: Register, state: &PState, intel: bool) -> Switch {
    match register {
        Register::Io { port, width } => Switch::Io {
            port,
            width,
            value: state.control as u32,
        },
        Register::Memory { address, width } => Switch::Memory {
            address,
            width,
            value: state.control,
        },
        Register::FixedHardware if intel => Switch::Msr {
            register: MSR_INTEL_PERF_CTL,
            value: state.control & 0xffff,
        },
        Register::FixedHardware => Switch::Msr {
            register: MSR_AMD_PSTATE_CONTROL,
            value: state.control & 0x7,
        },
    }
}
