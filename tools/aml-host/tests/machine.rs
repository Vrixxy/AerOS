//! Runs the interpreter against a real machine's tables. Set AEROS_ACPI_DIR to
//! a folder of raw tables (DSDT*.bin and SSD*.bin, e.g. dumped from
//! HKLM\HARDWARE\ACPI on Windows); without it these tests do nothing.

use aml_host::aml::{Aml, Hooks, NodeKind};
use std::sync::Mutex;

static EC_SPACE: Mutex<[u8; 256]> = Mutex::new([0; 256]);

// The root complex's SMN index/data pair (ECAM offsets 0xb8/0xbc) and the
// SMU mailbox behind it: a message write is answered with response 1 (OK).
static SMN: Mutex<(u32, std::collections::BTreeMap<u32, u32>)> =
    Mutex::new((0, std::collections::BTreeMap::new()));
const SMU_MESSAGE: u32 = 0x3b1_0528;
const SMU_RESPONSE: u32 = 0x3b1_0564;

static ACCESSES: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn note(text: String) {
    let mut list = ACCESSES.lock().unwrap();
    if list.len() == 4000 {
        list.drain(..2000);
    }
    list.push(text);
}

fn io_read(port: u16, _bytes: u8) -> u64 {
    note(format!("io r {port:#x}"));
    0
}
fn io_write(_port: u16, _bytes: u8, _value: u64) {}
fn mem_read(address: u64, _bytes: u8) -> Option<u64> {
    note(format!("mem r {address:#x}"));
    let smn = SMN.lock().unwrap();
    Some(match address & 0xfff {
        0xb8 => smn.0 as u64,
        0xbc => *smn.1.get(&smn.0).unwrap_or(&0) as u64,
        _ => 0,
    })
}
fn mem_write(address: u64, _bytes: u8, value: u64) -> bool {
    note(format!("mem w {address:#x}={value:#x}"));
    let mut smn = SMN.lock().unwrap();
    match address & 0xfff {
        0xb8 => smn.0 = value as u32,
        0xbc => {
            let index = smn.0;
            smn.1.insert(index, value as u32);
            if index == SMU_MESSAGE {
                smn.1.insert(SMU_RESPONSE, 1);
            }
        }
        _ => {}
    }
    true
}
fn pci_read(b: u8, s: u8, f: u8, o: u16, _n: u8) -> u64 {
    note(format!("pci r {b}:{s}.{f}+{o:#x}"));
    0xffff_ffff
}
fn pci_write(b: u8, s: u8, f: u8, o: u16, _n: u8, v: u64) {
    note(format!("pci w {b}:{s}.{f}+{o:#x}={v:#x}"));
}
fn now() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_nanos() as u64
}
fn ec_read(offset: u8) -> Option<u8> {
    Some(EC_SPACE.lock().unwrap()[offset as usize])
}
fn ec_write(offset: u8, value: u8) -> bool {
    EC_SPACE.lock().unwrap()[offset as usize] = value;
    true
}

fn hooks() -> Hooks {
    Hooks {
        io_read,
        io_write,
        mem_read,
        mem_write,
        pci_read,
        pci_write,
        ec_read,
        ec_write,
        now_ns: now,
    }
}

fn tables() -> Option<Vec<(String, &'static [u8])>> {
    let dir = std::env::var("AEROS_ACPI_DIR").ok()?;
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("DSDT") || name.starts_with("SSD"))
        .collect();
    names.sort_by_key(|name| (!name.starts_with("DSDT"), name.clone()));
    Some(
        names
            .into_iter()
            .map(|name| {
                let bytes = std::fs::read(format!("{dir}/{name}")).unwrap();
                (name, &*Box::leak(bytes.into_boxed_slice()))
            })
            .collect(),
    )
}

fn with_machine(body: impl FnOnce(&mut Aml) + Send + 'static) {
    let Some(tables) = tables() else {
        println!("AEROS_ACPI_DIR not set; skipped");
        return;
    };
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let mut aml = Box::new(Aml::new(hooks()));
            aml.reset();
            for (name, table) in &tables {
                let before = aml.load_errors;
                let result = aml.load(table.as_ptr() as u64, table.len());
                println!(
                    "{name}: {result:?} nodes={} new_errors={}",
                    aml.node_count(),
                    aml.load_errors - before
                );
            }
            body(&mut aml);
        })
        .unwrap()
        .join()
        .unwrap();
}

fn path(aml: &Aml, node: u16) -> String {
    let mut out = [0u8; 160];
    let n = aml.node_path(node, &mut out);
    String::from_utf8_lossy(&out[..n]).into_owned()
}

#[test]
fn loads_every_table() {
    with_machine(|aml| {
        let count = tables().unwrap().len();
        println!(
            "tables={count} nodes={} errors={}",
            aml.node_count(),
            aml.load_errors
        );
        assert_eq!(aml.load_errors, 0);
    });
}

#[test]
fn identification_methods_run() {
    with_machine(|aml| {
        let mut ok = 0;
        let mut bad = Vec::new();
        for index in 0..aml.node_count() {
            let node = index as u16;
            let name = aml.node_name(node);
            if aml.node_kind(node) == NodeKind::Method
                && [b"_STA", b"_CRS", b"_HID", b"_ADR", b"_UID", b"_PRW"].contains(&&name)
            {
                match aml.evaluate_node(node, &[]) {
                    Ok(_) => ok += 1,
                    Err(error) => bad.push((path(aml, node), error)),
                }
            }
        }
        println!("ok={ok} failed={}", bad.len());
        for (node, error) in &bad {
            println!("  {node}: {error:?}");
        }
        assert!(bad.is_empty());
    });
}

#[test]
fn power_devices() {
    with_machine(|aml| {
        let mut failures = Vec::new();
        for index in 0..aml.node_count() {
            let node = index as u16;
            if aml.node_kind(node) != NodeKind::Device {
                continue;
            }
            let mut id = [0u8; 8];
            let Some(n) = aml.device_id(node, b"_HID", &mut id) else {
                continue;
            };
            let methods: &[&[u8; 4]] = match &id[..n] {
                b"PNP0C0A" => &[b"_STA", b"_BIF", b"_BIX", b"_BST"],
                b"ACPI0003" => &[b"_STA", b"_PSR"],
                b"PNP0C0D" => &[b"_LID"],
                b"PNP0C09" => &[b"_STA", b"_CRS", b"_GPE"],
                b"PNP0C0C" | b"PNP0C0E" => &[b"_STA"],
                _ => continue,
            };
            for method in methods {
                let Some(child) = aml.child_node(node, method) else {
                    continue;
                };
                ACCESSES.lock().unwrap().clear();
                let label = format!(
                    "{} ({}).{}",
                    path(aml, node),
                    String::from_utf8_lossy(&id[..n]),
                    String::from_utf8_lossy(&method[..])
                );
                match aml.evaluate_node(child, &[]) {
                    Ok(value) => println!("{label} = {value:?}"),
                    Err(error) => {
                        println!("{label} FAILED {error:?}");
                        let list = ACCESSES.lock().unwrap();
                        for line in list.iter().rev().take(30).rev() {
                            println!("    {line}");
                        }
                        failures.push(label);
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:?}");
    });
}

#[test]
fn sleep_states_and_routing() {
    with_machine(|aml| {
        for state in [r"\_S0_", r"\_S3_", r"\_S4_", r"\_S5_"] {
            println!(
                "{state}: {:?}",
                aml.evaluate(state, &[]).map(|v| aml.package_len(v))
            );
        }
        assert!(aml.evaluate(r"\_S5_", &[]).is_ok());
        println!(r"\_PIC(1): {:?}", aml.evaluate(r"\_PIC", &[1]));
        let mut routed = 0;
        for index in 0..aml.node_count() {
            let node = index as u16;
            if &aml.node_name(node) == b"_PRT" {
                let result = aml.evaluate_node(node, &[]);
                println!(
                    "{} -> {:?}",
                    path(aml, node),
                    result.map(|v| aml.package_len(v))
                );
                routed += result.is_ok() as usize;
            }
        }
        assert!(routed > 0);
    });
}

#[test]
fn initialisers_run() {
    with_machine(|aml| {
        let mut bad = Vec::new();
        let mut ok = 0;
        for index in 0..aml.node_count() {
            let node = index as u16;
            if &aml.node_name(node) == b"_INI" && aml.node_kind(node) == NodeKind::Method {
                match aml.evaluate_node(node, &[]) {
                    Ok(_) => ok += 1,
                    Err(error) => bad.push((path(aml, node), error)),
                }
            }
        }
        println!("_INI ok={ok} failed={bad:?}");
        assert!(bad.is_empty());
        for method in [
            r"\_SB_.PCI0.SBRG.EC0_._REG",
            r"\_GPE._L08",
            r"\_SB_.PCI0.SBRG.EC0_._Q0A",
        ] {
            if let Some(node) = aml.find(method) {
                let args: &[u64] = if method.ends_with("_REG") {
                    &[3, 1]
                } else {
                    &[]
                };
                println!("{method}: {:?}", aml.evaluate_node(node, args));
            }
        }
    });
}

#[test]
fn event_handlers_run() {
    with_machine(|aml| {
        let mut bad = Vec::new();
        let mut ok = 0;
        for index in 0..aml.node_count() {
            let node = index as u16;
            let name = aml.node_name(node);
            let event = (name[0] == b'_' && matches!(name[1], b'Q' | b'L' | b'E'))
                && name[2].is_ascii_hexdigit()
                && name[3].is_ascii_hexdigit();
            if event && aml.node_kind(node) == NodeKind::Method {
                match aml.evaluate_node(node, &[]) {
                    Ok(_) => ok += 1,
                    Err(error) => bad.push((path(aml, node), error)),
                }
            }
            while aml.take_notification().is_some() {}
        }
        println!("events ok={ok} failed={}", bad.len());
        for (node, error) in &bad {
            println!("  {node}: {error:?}");
        }
        assert!(bad.is_empty());
    });
}

#[test]
fn backlight_methods() {
    with_machine(|aml| {
        for index in 0..aml.node_count() {
            let node = index as u16;
            if &aml.node_name(node) != b"_BCL" {
                continue;
            }
            let device = path(aml, node);
            let levels = aml.evaluate_node(node, &[]).unwrap();
            let count = aml.package_len(levels).unwrap();
            let values: Vec<_> = (0..count)
                .map(|i| aml.package_element(levels, i).and_then(|v| aml.integer(v)))
                .collect();
            println!("{device} = {values:?}");
            let parent = device.trim_end_matches("._BCL");
            for level in [values[count - 1].unwrap_or(100), values[2].unwrap_or(0)] {
                ACCESSES.lock().unwrap().clear();
                let result = aml.evaluate(&format!("{parent}._BCM"), &[level]);
                let list = ACCESSES.lock().unwrap().clone();
                println!("_BCM({level}) -> {result:?}; accesses: {list:?}");
                println!("_BQC -> {:?}", aml.evaluate(&format!("{parent}._BQC"), &[]));
            }
            println!("EC after: {:?}", &EC_SPACE.lock().unwrap()[..]);
        }
    });
}
