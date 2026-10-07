use aml_host::acpi_devices::{self, GpeHandler};
use std::sync::Mutex;

use aml_host::aml::{Aml, Hooks};

static EC_SPACE: Mutex<[u8; 256]> = Mutex::new([0; 256]);

fn io_read(_port: u16, _bytes: u8) -> u64 {
    0
}
fn io_write(_port: u16, _bytes: u8, _value: u64) {}
fn mem_read(_address: u64, _bytes: u8) -> Option<u64> {
    Some(0)
}
fn mem_write(_address: u64, _bytes: u8, _value: u64) -> bool {
    true
}
fn pci_read(_b: u8, _s: u8, _f: u8, _o: u16, _n: u8) -> u64 {
    0
}
fn pci_write(_b: u8, _s: u8, _f: u8, _o: u16, _n: u8, _v: u64) {}
fn ec_read(offset: u8) -> Option<u8> {
    Some(EC_SPACE.lock().unwrap()[offset as usize])
}
fn ec_write(offset: u8, value: u8) -> bool {
    EC_SPACE.lock().unwrap()[offset as usize] = value;
    true
}
fn now() -> u64 {
    0
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

fn pkg(content: &[u8]) -> Vec<u8> {
    let n = content.len();
    let mut out = Vec::new();
    if n + 1 < 0x40 {
        out.push((n + 1) as u8);
    } else if n + 2 < 0x1000 {
        let total = n + 2;
        out.push(0x40 | (total & 0xf) as u8);
        out.push((total >> 4) as u8);
    } else {
        let total = n + 3;
        out.push(0x80 | (total & 0xf) as u8);
        out.push((total >> 4) as u8);
        out.push((total >> 12) as u8);
    }
    out.extend_from_slice(content);
    out
}

fn int(value: u64) -> Vec<u8> {
    match value {
        0 => vec![0x00],
        1 => vec![0x01],
        2..=0xff => vec![0x0a, value as u8],
        0x100..=0xffff => {
            let mut out = vec![0x0b];
            out.extend_from_slice(&(value as u16).to_le_bytes());
            out
        }
        _ => {
            let mut out = vec![0x0c];
            out.extend_from_slice(&(value as u32).to_le_bytes());
            out
        }
    }
}

fn text(value: &str) -> Vec<u8> {
    let mut out = vec![0x0d];
    out.extend_from_slice(value.as_bytes());
    out.push(0);
    out
}

fn path(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let rest = match value.strip_prefix('\\') {
        Some(rest) => {
            out.push(0x5c);
            rest
        }
        None => value,
    };
    let parts: Vec<&str> = rest.split('.').collect();
    match parts.len() {
        1 => {}
        2 => out.push(0x2e),
        count => {
            out.push(0x2f);
            out.push(count as u8);
        }
    }
    for part in parts {
        assert_eq!(part.len(), 4);
        out.extend_from_slice(part.as_bytes());
    }
    out
}

fn named(name: &str, value: &[u8]) -> Vec<u8> {
    let mut out = vec![0x08];
    out.extend(path(name));
    out.extend_from_slice(value);
    out
}

fn package(elements: &[Vec<u8>]) -> Vec<u8> {
    let mut content = vec![elements.len() as u8];
    for element in elements {
        content.extend_from_slice(element);
    }
    let mut out = vec![0x12];
    out.extend(pkg(&content));
    out
}

fn method(name: &str, body: &[u8]) -> Vec<u8> {
    let mut content = path(name);
    content.push(0);
    content.extend_from_slice(body);
    let mut out = vec![0x14];
    out.extend(pkg(&content));
    out
}

fn returning(value: &[u8]) -> Vec<u8> {
    let mut out = vec![0xa4];
    out.extend_from_slice(value);
    out
}

fn device(name: &str, body: &[u8]) -> Vec<u8> {
    let mut content = path(name);
    content.extend_from_slice(body);
    let mut out = vec![0x5b, 0x82];
    out.extend(pkg(&content));
    out
}

fn zone(name: &str, body: &[u8]) -> Vec<u8> {
    let mut content = path(name);
    content.extend_from_slice(body);
    let mut out = vec![0x5b, 0x85];
    out.extend(pkg(&content));
    out
}

fn scope(name: &str, body: &[u8]) -> Vec<u8> {
    let mut content = path(name);
    content.extend_from_slice(body);
    let mut out = vec![0x10];
    out.extend(pkg(&content));
    out
}

fn eisa(id: &str) -> Vec<u8> {
    let b = id.as_bytes();
    let vendor =
        (((b[0] - 0x40) as u16) << 10) | (((b[1] - 0x40) as u16) << 5) | (b[2] - 0x40) as u16;
    let hex = |c: u8| (c as char).to_digit(16).unwrap() as u8;
    let mut out = vec![0x0c, (vendor >> 8) as u8, vendor as u8];
    out.push(hex(b[3]) << 4 | hex(b[4]));
    out.push(hex(b[5]) << 4 | hex(b[6]));
    out
}

fn table(body: &[u8]) -> &'static [u8] {
    let mut out = Vec::new();
    out.extend_from_slice(b"DSDT");
    out.extend_from_slice(&((36 + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&[2, 0]);
    out.extend_from_slice(b"AEROS ");
    out.extend_from_slice(b"TESTTBL ");
    out.extend_from_slice(&[1, 0, 0, 0]);
    out.extend_from_slice(b"INTL");
    out.extend_from_slice(&[0x20, 0x20, 0x20, 0x20]);
    assert_eq!(out.len(), 36);
    out.extend_from_slice(body);
    Box::leak(out.into_boxed_slice())
}

fn with_table(body: Vec<u8>, check: impl FnOnce(&mut Aml) + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let mut aml = Box::new(Aml::new(hooks()));
            aml.reset();
            let code = table(&body);
            aml.load(code.as_ptr() as u64, code.len()).unwrap();
            assert_eq!(aml.load_errors, 0);
            check(&mut aml);
        })
        .unwrap()
        .join()
        .unwrap();
}

fn battery(name: &str, status: u64, info: Vec<u8>, info_name: &str, state: [u64; 4]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(named("_HID", &eisa("PNP0C0A")));
    body.extend(method("_STA", &returning(&int(status))));
    body.extend(method(info_name, &returning(&info)));
    let elements: Vec<Vec<u8>> = state.iter().map(|value| int(*value)).collect();
    body.extend(method("_BST", &returning(&package(&elements))));
    device(name, &body)
}

fn bix(unit: u64, design: u64, full: u64, cycles: u64, model: &str) -> Vec<u8> {
    let mut elements = vec![
        int(0),
        int(unit),
        int(design),
        int(full),
        int(1),
        int(11400),
        int(4000),
        int(2000),
        int(cycles),
    ];
    for _ in 9..16 {
        elements.push(int(0));
    }
    elements.push(text(model));
    elements.push(text("SERIAL"));
    elements.push(text("LION"));
    elements.push(text("OEM"));
    package(&elements)
}

fn bif(unit: u64, design: u64, full: u64, model: &str) -> Vec<u8> {
    let elements = vec![
        int(unit),
        int(design),
        int(full),
        int(1),
        int(7400),
        int(3000),
        int(1500),
        int(10),
        int(10),
        text(model),
        text("SERIAL"),
        text("LION"),
        text("OEM"),
    ];
    package(&elements)
}

#[test]
fn battery_discharging_through_bix() {
    let body = scope(
        "\\_SB_",
        &battery(
            "BAT0",
            0x1f,
            bix(0, 50000, 40000, 123, "TESTBAT"),
            "_BIX",
            [1, 5000, 20000, 11800],
        ),
    );
    with_table(body, |aml| {
        let inventory = acpi_devices::scan(aml);
        assert_eq!(inventory.battery_count, 1);
        let battery = acpi_devices::read_battery(aml, inventory.batteries[0]);
        assert!(battery.present && battery.discharging && !battery.charging && !battery.critical);
        assert!(!battery.current_units);
        assert_eq!(battery.design_capacity, 50000);
        assert_eq!(battery.full_capacity, 40000);
        assert_eq!(battery.remaining, 20000);
        assert_eq!(battery.rate, 5000);
        assert_eq!(battery.voltage_mv, 11800);
        assert_eq!(battery.cycles, Some(123));
        assert_eq!(battery.model_text(), b"TESTBAT");
        assert_eq!(battery.percent(), Some(50));
        assert_eq!(battery.minutes(), Some(240));
    });
}

#[test]
fn battery_charging_through_bif() {
    let body = scope(
        "\\_SB_",
        &battery(
            "BAT1",
            0x1f,
            bif(1, 3000, 2500, "OLDBAT"),
            "_BIF",
            [2, 1000, 500, 7600],
        ),
    );
    with_table(body, |aml| {
        let inventory = acpi_devices::scan(aml);
        let battery = acpi_devices::read_battery(aml, inventory.batteries[0]);
        assert!(battery.charging && !battery.discharging);
        assert!(battery.current_units);
        assert_eq!(battery.cycles, None);
        assert_eq!(battery.model_text(), b"OLDBAT");
        assert_eq!(battery.percent(), Some(20));
        // 2000 units to go at 1000 per hour.
        assert_eq!(battery.minutes(), Some(120));
    });
}

#[test]
fn absent_battery_and_unknown_values() {
    let mut body = battery("BAT0", 0x0f, bif(0, 1, 1, "X"), "_BIF", [0, 0, 0, 0]);
    body.extend(battery(
        "BAT1",
        0x1f,
        bif(0, 0, 0, "Y"),
        "_BIF",
        [1, 0xffff_ffff, 0xffff_ffff, 0xffff_ffff],
    ));
    with_table(scope("\\_SB_", &body), |aml| {
        let inventory = acpi_devices::scan(aml);
        assert_eq!(inventory.battery_count, 2);
        let absent = acpi_devices::read_battery(aml, inventory.batteries[0]);
        assert!(!absent.present);
        assert_eq!(absent.percent(), None);
        let unknown = acpi_devices::read_battery(aml, inventory.batteries[1]);
        assert!(unknown.present);
        assert_eq!(unknown.percent(), None);
        assert_eq!(unknown.minutes(), None);
    });
}

#[test]
fn mains_lid_and_thermal() {
    let mut body = Vec::new();
    let mut adapter = named("_HID", &text("ACPI0003"));
    adapter.extend(named("PWRS", &int(1)));
    adapter.extend(method("_PSR", &returning(&path("PWRS"))));
    body.extend(device("ADP1", &adapter));
    let mut lid = named("_HID", &eisa("PNP0C0D"));
    lid.extend(method("_LID", &returning(&int(0))));
    body.extend(device("LID0", &lid));
    let mut thermal = Vec::new();
    thermal.extend(method("_TMP", &returning(&int(3282))));
    thermal.extend(method("_CRT", &returning(&int(3732))));
    thermal.extend(method("_PSV", &returning(&int(3532))));
    body.extend(zone("TZ00", &thermal));
    let mut dead = Vec::new();
    dead.extend(method("_TMP", &returning(&int(0))));
    body.extend(zone("TZ01", &dead));
    with_table(scope("\\_SB_", &body), |aml| {
        let inventory = acpi_devices::scan(aml);
        assert_eq!(inventory.adapter_count, 1);
        assert_eq!(acpi_devices::on_mains(aml, &inventory), Some(true));
        assert_eq!(acpi_devices::lid_open(aml, &inventory), Some(false));
        assert_eq!(inventory.zone_count, 2);
        let reading = acpi_devices::read_zone(aml, inventory.zones[0]).unwrap();
        assert_eq!(reading.temperature, 550);
        assert_eq!(reading.critical, Some(1000));
        assert_eq!(reading.passive, Some(800));
        assert_eq!(reading.hot, None);
        assert!(acpi_devices::read_zone(aml, inventory.zones[1]).is_none());
    });
}

#[test]
fn no_devices_means_nothing_to_report() {
    with_table(scope("\\_SB_", &named("FOO0", &int(1))), |aml| {
        let inventory = acpi_devices::scan(aml);
        assert_eq!(
            inventory.battery_count + inventory.adapter_count + inventory.zone_count,
            0
        );
        assert_eq!(acpi_devices::on_mains(aml, &inventory), None);
        assert_eq!(acpi_devices::lid_open(aml, &inventory), None);
    });
}

#[test]
fn gpe_handlers_are_found_and_notify_is_queued() {
    let mut body = scope(
        "\\_SB_",
        &battery("BAT0", 0x1f, bif(0, 1, 1, "X"), "_BIF", [1, 1, 1, 1]),
    );
    let mut notify = vec![0x86];
    notify.extend(path("\\_SB_.BAT0"));
    notify.extend(int(0x80));
    let mut handlers = method("_E1C", &notify);
    handlers.extend(method("_L05", &[]));
    handlers.extend(method("_Q05", &[]));
    body.extend(scope("\\_GPE", &handlers));
    with_table(body, |aml| {
        let mut found = [GpeHandler::default(); 8];
        let count = acpi_devices::scan_gpes(aml, &mut found);
        assert_eq!(count, 2);
        let edge = found[..count].iter().find(|handler| !handler.level).unwrap();
        let level = found[..count].iter().find(|handler| handler.level).unwrap();
        assert_eq!(edge.number, 0x1c);
        assert_eq!(level.number, 0x05);
        aml.evaluate_node(edge.node, &[]).unwrap();
        let inventory = acpi_devices::scan(aml);
        let (node, value) = aml.take_notification().unwrap();
        assert_eq!(value, 0x80);
        assert_eq!(node, inventory.batteries[0]);
        assert_eq!(aml.take_notification(), None);
    });
}

fn method_with(name: &str, arguments: u8, body: &[u8]) -> Vec<u8> {
    let mut content = path(name);
    content.push(arguments);
    content.extend_from_slice(body);
    let mut out = vec![0x14];
    out.extend(pkg(&content));
    out
}

fn buffer(bytes: &[u8]) -> Vec<u8> {
    let mut content = int(bytes.len() as u64);
    content.extend_from_slice(bytes);
    let mut out = vec![0x11];
    out.extend(pkg(&content));
    out
}

fn embedded_controller() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(named("_HID", &eisa("PNP0C09")));
    body.extend(named(
        "_CRS",
        &buffer(&[
            0x47, 0x01, 0x62, 0x00, 0x62, 0x00, 0x00, 0x01, 0x47, 0x01, 0x66, 0x00, 0x66, 0x00,
            0x00, 0x01, 0x79, 0x00,
        ]),
    ));
    body.extend(named("_GPE", &int(0x17)));
    body.extend(named("ECAV", &int(0)));
    // OperationRegion (ECF2, EmbeddedControl, 0, 0x40)
    let mut region = vec![0x5b, 0x80];
    region.extend(path("ECF2"));
    region.push(0x03);
    region.extend(int(0));
    region.extend(int(0x40));
    body.extend(region);
    // Field (ECF2, ByteAcc, NoLock, Preserve) { BRAT, 8; BREM, 16; BVLT, 16 }
    let mut fields = path("ECF2");
    fields.push(0x01);
    for (name, bits) in [("BRAT", 8u8), ("BREM", 16), ("BVLT", 16)] {
        fields.extend_from_slice(name.as_bytes());
        fields.push(bits);
    }
    let mut field = vec![0x5b, 0x81];
    field.extend(pkg(&fields));
    body.extend(field);
    // Method (_REG, 2) { ECAV = Arg1 }
    let mut store = vec![0x70, 0x69];
    store.extend(path("ECAV"));
    body.extend(method_with("_REG", 2, &store));
    // Method (_Q17, 0) { BRAT = 0x55 }
    let mut query = vec![0x70];
    query.extend(int(0x55));
    query.extend(path("BRAT"));
    body.extend(method("_Q17", &query));
    device("EC0_", &body)
}

#[test]
fn embedded_controller_region_is_read_and_written_through_the_hooks() {
    {
        let mut space = EC_SPACE.lock().unwrap();
        *space = [0; 256];
        space[0x01] = 0x34;
        space[0x02] = 0x12;
    }
    with_table(scope("\\_SB_", &embedded_controller()), |aml| {
        let ec = acpi_devices::find_ec(aml).unwrap();
        assert_eq!((ec.data, ec.command, ec.gpe), (0x62, 0x66, Some(0x17)));

        let value = aml.evaluate("\\_SB_.EC0_.BREM", &[]).unwrap();
        assert_eq!(aml.integer(value), Some(0x1234));

        acpi_devices::announce_ec_region(aml, &ec);
        let available = aml.evaluate("\\_SB_.EC0_.ECAV", &[]).unwrap();
        assert_eq!(aml.integer(available), Some(1));

        assert!(acpi_devices::run_ec_query(aml, &ec, 0x17));
        assert_eq!(EC_SPACE.lock().unwrap()[0x00], 0x55);
        assert!(!acpi_devices::run_ec_query(aml, &ec, 0x18));
    });
}
