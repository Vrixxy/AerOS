use aml_host::aml::{Aml, Hooks, NodeKind, resources, Resource};

fn io_read(_port: u16, _bytes: u8) -> u64 { 0 }
fn io_write(_port: u16, _bytes: u8, _value: u64) {}
fn mem_read(_address: u64, _bytes: u8) -> Option<u64> { Some(0) }
fn mem_write(_address: u64, _bytes: u8, _value: u64) -> bool { true }
fn pci_read(_b: u8, _s: u8, _f: u8, _o: u16, _n: u8) -> u64 { 0 }
fn pci_write(_b: u8, _s: u8, _f: u8, _o: u16, _n: u8, _v: u64) {}
fn now() -> u64 { 0 }
fn ec_read(_offset: u8) -> Option<u8> { None }
fn ec_write(_offset: u8, _value: u8) -> bool { false }

fn hooks() -> Hooks {
    Hooks { io_read, io_write, mem_read, mem_write, pci_read, pci_write, ec_read, ec_write, now_ns: now }
}

fn with_aml(body: impl FnOnce(&mut Aml) + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let mut aml = Box::new(Aml::new(hooks()));
            aml.reset();
            let table: &'static [u8] = Box::leak(std::fs::read("fixtures/qemu-q35-dsdt.bin").unwrap().into_boxed_slice());
            let result = aml.load(table.as_ptr() as u64, table.len());
            println!("load: {result:?} nodes={} errors={}", aml.node_count(), aml.load_errors);
            body(&mut aml);
        })
        .unwrap()
        .join()
        .unwrap();
}

fn path(aml: &Aml, node: u16) -> String {
    let mut out = [0u8; 128];
    let n = aml.node_path(node, &mut out);
    String::from_utf8_lossy(&out[..n]).into_owned()
}

#[test]
fn loads_and_lists() {
    with_aml(|aml| {
        assert_eq!(aml.load_errors, 0);
        let mut devices = 0;
        let mut methods = 0;
        for index in 0..aml.node_count() {
            match aml.node_kind(index as u16) {
                NodeKind::Device => { devices += 1; println!("device {}", path(aml, index as u16)); }
                NodeKind::Method => { methods += 1; println!("method {}", path(aml, index as u16)); }
                _ => {}
            }
        }
        println!("devices={devices} methods={methods}");
        assert!(devices > 10);
    });
}

#[test]
fn s5_package() {
    with_aml(|aml| {
        let value = aml.evaluate(r"\_S5_", &[]).unwrap();
        let len = aml.package_len(value).unwrap();
        let first = aml.package_element(value, 0).unwrap();
        let second = aml.package_element(value, 1).unwrap();
        println!("S5 len={len} {:?} {:?}", aml.integer(first), aml.integer(second));
        assert!(len >= 2);
    });
}

#[test]
fn pci_root() {
    with_aml(|aml| {
        let node = aml.find(r"\_SB_.PCI0").unwrap();
        let mut out = [0u8; 8];
        let n = aml.device_id(node, b"_HID", &mut out).unwrap();
        println!("HID {}", String::from_utf8_lossy(&out[..n]));
        assert_eq!(&out[..n], b"PNP0A08");
        let crs = aml.evaluate(r"\_SB_.PCI0._CRS", &[]).unwrap();
        let bytes = aml.buffer(crs).unwrap().to_vec();
        let mut found_cf8 = false;
        resources(&bytes, |r| { println!("{r:?}"); if let Resource::Io { minimum: 0xcf8, .. } = r { found_cf8 = true; } });
        assert!(found_cf8);
    });
}

#[test]
fn prt() {
    with_aml(|aml| {
        let r = aml.evaluate(r"\_PIC", &[1]);
        println!(r"_PIC -> {r:?}");
        let picf = aml.evaluate(r"\PICF", &[]);
        println!("PICF {picf:?}");
        let prt = aml.evaluate(r"\_SB_.PCI0._PRT", &[]).unwrap();
        let n = aml.package_len(prt).unwrap();
        println!("PRT entries {n}");
        for i in 0..n.min(6) {
            let entry = aml.package_element(prt, i).unwrap();
            let fields: Vec<_> = (0..4).map(|j| aml.package_element(entry, j)).collect();
            println!("{i}: {fields:?}");
        }
        assert!(n > 20);
    });
}

#[test]
fn every_sta_and_crs() {
    with_aml(|aml| {
        let mut ok = 0;
        let mut bad = Vec::new();
        for index in 0..aml.node_count() {
            let node = index as u16;
            let name = aml.node_name(node);
            if aml.node_kind(node) == NodeKind::Method && (&name == b"_STA" || &name == b"_CRS" || &name == b"_HID" || &name == b"_ADR") {
                match aml.evaluate_node(node, &[]) {
                    Ok(v) => { ok += 1; if &name == b"_STA" { println!("{} = {:?}", path(aml, node), v); } }
                    Err(e) => bad.push((path(aml, node), e)),
                }
            }
        }
        println!("ok={ok} bad={bad:?}");
        assert!(bad.is_empty());
    });
}

#[test]
fn prt_follows_interrupt_mode() {
    with_aml(|aml| {
        let source_of = |aml: &mut Aml| -> String {
            let prt = aml.evaluate(r"\_SB_.PCI0._PRT", &[]).unwrap();
            let entry = aml.package_element(prt, 0).unwrap();
            let node = aml.value_node(aml.package_element(entry, 2).unwrap()).unwrap();
            path(aml, node)
        };
        let before = source_of(aml);
        aml.evaluate(r"\_PIC", &[1]).unwrap();
        let after = source_of(aml);
        println!("PIC mode source {before}, APIC mode source {after}");
        assert_ne!(before, after);
    });
}
