//! Battery, mains adapter, lid and thermal-zone readings through the AML
//! interpreter (`_BIF`/`_BIX`/`_BST`, `_PSR`, `_LID`, `_TMP`). Everything here
//! works on a borrowed interpreter, so it is tested on the host against
//! hand-built tables.

use crate::aml::{Aml, NodeKind, Resource, Value};

pub const MAX_BATTERIES: usize = 4;
pub const MAX_ADAPTERS: usize = 2;
pub const MAX_ZONES: usize = 4;
const UNKNOWN: u64 = 0xffff_ffff;

#[derive(Clone, Copy, Default)]
pub struct Inventory {
    pub batteries: [u16; MAX_BATTERIES],
    pub battery_count: usize,
    pub adapters: [u16; MAX_ADAPTERS],
    pub adapter_count: usize,
    pub lid: Option<u16>,
    pub zones: [u16; MAX_ZONES],
    pub zone_count: usize,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Battery {
    pub present: bool,
    pub discharging: bool,
    pub charging: bool,
    pub critical: bool,
    /// Capacities and rate are in milliamp-hours (and milliamps) when set,
    /// milliwatt-hours (and milliwatts) otherwise.
    pub current_units: bool,
    pub design_capacity: u32,
    pub full_capacity: u32,
    pub remaining: u32,
    pub rate: u32,
    pub voltage_mv: u32,
    pub cycles: Option<u32>,
    pub model: [u8; 16],
    pub model_length: usize,
}

impl Battery {
    pub fn percent(&self) -> Option<u8> {
        let full = if self.full_capacity != 0 {
            self.full_capacity
        } else {
            self.design_capacity
        };
        if !self.present || full == 0 || self.remaining as u64 == UNKNOWN {
            return None;
        }
        Some((u64::from(self.remaining) * 100 / u64::from(full)).min(100) as u8)
    }

    /// Minutes until empty (discharging) or full (charging), when the rate is
    /// known and non-zero.
    pub fn minutes(&self) -> Option<u32> {
        let full = if self.full_capacity != 0 {
            self.full_capacity
        } else {
            self.design_capacity
        };
        if self.rate == 0 || u64::from(self.rate) == UNKNOWN || u64::from(self.remaining) == UNKNOWN
        {
            return None;
        }
        let units = if self.discharging {
            self.remaining
        } else if self.charging {
            full.saturating_sub(self.remaining)
        } else {
            return None;
        };
        Some((u64::from(units) * 60 / u64::from(self.rate)).min(u64::from(u32::MAX)) as u32)
    }

    pub fn model_text(&self) -> &[u8] {
        &self.model[..self.model_length]
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Zone {
    /// Tenths of a degree Celsius.
    pub temperature: i32,
    pub critical: Option<i32>,
    pub passive: Option<i32>,
    pub hot: Option<i32>,
}

fn integer_of(aml: &Aml, value: Value) -> Option<u64> {
    aml.integer(value)
}

fn call(aml: &mut Aml, node: u16, name: &[u8; 4]) -> Option<Value> {
    let child = aml.child_node(node, name)?;
    aml.evaluate_node(child, &[]).ok()
}

fn call_integer(aml: &mut Aml, node: u16, name: &[u8; 4]) -> Option<u64> {
    let value = call(aml, node, name)?;
    integer_of(aml, value)
}

/// Finds the batteries, adapters, lid and thermal zones in the namespace.
pub fn scan(aml: &mut Aml) -> Inventory {
    let mut inventory = Inventory::default();
    for index in 0..aml.node_count() {
        let node = index as u16;
        match aml.node_kind(node) {
            NodeKind::ThermalZone => {
                if inventory.zone_count < MAX_ZONES {
                    inventory.zones[inventory.zone_count] = node;
                    inventory.zone_count += 1;
                }
            }
            NodeKind::Device => {
                let mut id = [0u8; 8];
                let Some(length) = aml.device_id(node, b"_HID", &mut id) else {
                    continue;
                };
                match &id[..length] {
                    b"PNP0C0A" if inventory.battery_count < MAX_BATTERIES => {
                        inventory.batteries[inventory.battery_count] = node;
                        inventory.battery_count += 1;
                    }
                    b"ACPI0003" if inventory.adapter_count < MAX_ADAPTERS => {
                        inventory.adapters[inventory.adapter_count] = node;
                        inventory.adapter_count += 1;
                    }
                    b"PNP0C0D" if inventory.lid.is_none() => inventory.lid = Some(node),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    inventory
}

fn element_integer(aml: &Aml, package: Value, index: usize) -> Option<u64> {
    aml.package_element(package, index)
        .and_then(|element| integer_of(aml, element))
}

fn element_u32(aml: &Aml, package: Value, index: usize) -> u32 {
    element_integer(aml, package, index).unwrap_or(UNKNOWN) as u32
}

fn copy_model(aml: &Aml, package: Value, index: usize, battery: &mut Battery) {
    let Some(element) = aml.package_element(package, index) else {
        return;
    };
    if let Some(bytes) = aml.buffer(element) {
        let text = bytes.split(|byte| *byte == 0).next().unwrap_or(&[]);
        let length = text.len().min(battery.model.len());
        battery.model[..length].copy_from_slice(&text[..length]);
        battery.model_length = length;
    }
}

pub fn read_battery(aml: &mut Aml, node: u16) -> Battery {
    let mut battery = Battery::default();
    let Some(status) = call_integer(aml, node, b"_STA") else {
        return read_battery_present(aml, node, battery);
    };
    battery.present = status & 0b1001 == 0b1001 && status & 0b10000 != 0;
    if !battery.present {
        return battery;
    }
    read_battery_present(aml, node, battery)
}

fn read_battery_present(aml: &mut Aml, node: u16, mut battery: Battery) -> Battery {
    battery.present = true;
    if let Some(package) = call(aml, node, b"_BIX") {
        battery.current_units = element_integer(aml, package, 1) == Some(1);
        battery.design_capacity = element_u32(aml, package, 2);
        battery.full_capacity = element_u32(aml, package, 3);
        battery.cycles = element_integer(aml, package, 8)
            .filter(|cycles| *cycles != UNKNOWN)
            .map(|cycles| cycles as u32);
        copy_model(aml, package, 16, &mut battery);
    } else if let Some(package) = call(aml, node, b"_BIF") {
        battery.current_units = element_integer(aml, package, 0) == Some(1);
        battery.design_capacity = element_u32(aml, package, 1);
        battery.full_capacity = element_u32(aml, package, 2);
        copy_model(aml, package, 9, &mut battery);
    }
    if let Some(package) = call(aml, node, b"_BST") {
        let state = element_integer(aml, package, 0).unwrap_or(0);
        battery.discharging = state & 1 != 0;
        battery.charging = state & 2 != 0;
        battery.critical = state & 4 != 0;
        battery.rate = element_u32(aml, package, 1);
        battery.remaining = element_u32(aml, package, 2);
        battery.voltage_mv = element_u32(aml, package, 3);
    }
    battery
}

/// Whether any mains adapter reports itself online; `None` with none found.
pub fn on_mains(aml: &mut Aml, inventory: &Inventory) -> Option<bool> {
    let mut seen = false;
    for node in &inventory.adapters[..inventory.adapter_count] {
        if let Some(online) = call_integer(aml, *node, b"_PSR") {
            seen = true;
            if online != 0 {
                return Some(true);
            }
        }
    }
    seen.then_some(false)
}

/// `Some(true)` for an open lid.
pub fn lid_open(aml: &mut Aml, inventory: &Inventory) -> Option<bool> {
    let node = inventory.lid?;
    call_integer(aml, node, b"_LID").map(|state| state != 0)
}

fn decikelvin_to_decicelsius(value: u64) -> Option<i32> {
    // Zero and absurd readings mean "no sensor".
    if value == 0 || value > 6000 {
        return None;
    }
    Some(value as i32 - 2732)
}

pub fn read_zone(aml: &mut Aml, node: u16) -> Option<Zone> {
    let temperature = decikelvin_to_decicelsius(call_integer(aml, node, b"_TMP")?)?;
    let mut limit =
        |name: &[u8; 4]| call_integer(aml, node, name).and_then(decikelvin_to_decicelsius);
    Some(Zone {
        temperature,
        critical: limit(b"_CRT"),
        passive: limit(b"_PSV"),
        hot: limit(b"_HOT"),
    })
}

/// A general-purpose-event handler method, `\_GPE._Exx` (edge) or
/// `\_GPE._Lxx` (level).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct GpeHandler {
    pub number: u8,
    pub level: bool,
    pub node: u16,
    /// The embedded controller's event line, not a `_Exx`/`_Lxx` method.
    pub ec: bool,
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Fills `out` with the handlers under `\_GPE`; returns how many were found.
pub fn scan_gpes(aml: &mut Aml, out: &mut [GpeHandler]) -> usize {
    let mut count = 0;
    for index in 0..aml.node_count() {
        if count == out.len() {
            break;
        }
        let node = index as u16;
        if aml.node_kind(node) != NodeKind::Method {
            continue;
        }
        let mut buffer = [0u8; 32];
        let length = aml.node_path(node, &mut buffer);
        let path = &buffer[..length];
        if length != 10 || !path.starts_with(b"\\_GPE._") {
            continue;
        }
        let level = match path[7] {
            b'L' => true,
            b'E' => false,
            _ => continue,
        };
        let (Some(high), Some(low)) = (hex_digit(path[8]), hex_digit(path[9])) else {
            continue;
        };
        out[count] = GpeHandler {
            number: high << 4 | low,
            level,
            node,
            ec: false,
        };
        count += 1;
    }
    count
}

/// The embedded controller: its two I/O ports and the event line it raises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EcInfo {
    pub node: u16,
    pub data: u16,
    pub command: u16,
    pub gpe: Option<u8>,
}

/// Finds the embedded-controller device (`PNP0C09`): data port first, then
/// command/status port, as its `_CRS` lists them.
pub fn find_ec(aml: &mut Aml) -> Option<EcInfo> {
    for index in 0..aml.node_count() {
        let node = index as u16;
        if aml.node_kind(node) != NodeKind::Device {
            continue;
        }
        let mut id = [0u8; 8];
        if aml.device_id(node, b"_HID", &mut id) != Some(7) || &id[..7] != b"PNP0C09" {
            continue;
        }
        let mut template = [0u8; 128];
        let length = {
            let value = call(aml, node, b"_CRS")?;
            let bytes = aml.buffer(value)?;
            let length = bytes.len().min(template.len());
            template[..length].copy_from_slice(&bytes[..length]);
            length
        };
        let mut ports = [0u16; 2];
        let mut found = 0;
        crate::aml::resources(&template[..length], |resource| {
            let base = match resource {
                Resource::Io { minimum, .. } => minimum,
                Resource::FixedIo { base, .. } => base as u32,
                _ => return,
            };
            if found < 2 {
                ports[found] = base as u16;
                found += 1;
            }
        });
        if found < 2 || ports[0] == 0 || ports[1] == 0 {
            continue;
        }
        let gpe = call_integer(aml, node, b"_GPE")
            .filter(|number| *number < 256)
            .map(|number| number as u8);
        return Some(EcInfo {
            node,
            data: ports[0],
            command: ports[1],
            gpe,
        });
    }
    None
}

/// Tells the firmware the embedded-controller region now has a handler
/// (`_REG(EmbeddedControl, 1)`), as most laptop tables wait for before they
/// touch it.
pub fn announce_ec_region(aml: &mut Aml, ec: &EcInfo) {
    if let Some(method) = aml.child_node(ec.node, b"_REG") {
        let _ = aml.evaluate_node(method, &[3, 1]);
    }
}

/// Runs the `_Qxx` method the controller's query value names.
pub fn run_ec_query(aml: &mut Aml, ec: &EcInfo, code: u8) -> bool {
    let hex = b"0123456789ABCDEF";
    let name = [
        b'_',
        b'Q',
        hex[(code >> 4) as usize],
        hex[(code & 15) as usize],
    ];
    match aml.child_node(ec.node, &name) {
        Some(method) => aml.evaluate_node(method, &[]).is_ok(),
        None => false,
    }
}
