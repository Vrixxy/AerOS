use aslr_host::aslr::*;

fn le(bytes: &mut [u8], at: usize, value: u64, width: usize) {
    bytes[at..at + width].copy_from_slice(&value.to_le_bytes()[..width]);
}

const POINTERS: [(usize, u64); 3] = [(0x1008, 0x1100), (0x1010, 0x1200), (0x1ff8, 0x2f00)];

/// A tiny PE32+ image: headers, one section with three pointers, and a
/// relocation table naming them.
fn synthetic(base: u64) -> (Vec<u8>, Pe) {
    let mut image = vec![0u8; 0x3000];
    image[..2].copy_from_slice(b"MZ");
    le(&mut image, 0x3c, 0x80, 4);
    let pe = 0x80;
    image[pe..pe + 4].copy_from_slice(b"PE\0\0");
    le(&mut image, pe + 6, 2, 2);
    le(&mut image, pe + 20, 240, 2);
    let optional = pe + 24;
    le(&mut image, optional, 0x20b, 2);
    le(&mut image, optional + 16, 0x1000, 4);
    le(&mut image, optional + 24, base, 8);
    le(&mut image, optional + 56, 0x3000, 4);
    le(&mut image, optional + 60, 0x400, 4);
    le(&mut image, optional + 108, 16, 4);
    le(&mut image, optional + 112 + 8 * 5, 0x2000, 4);
    le(&mut image, optional + 112 + 8 * 5 + 4, 0x10, 4);
    let table = optional + 240;
    for (index, (rva, virtual_size, raw)) in
        [(0x1000u64, 0x1000u64, 0x1000u64), (0x2000, 0x10, 0x200)].iter().enumerate()
    {
        let entry = table + 40 * index;
        le(&mut image, entry + 8, *virtual_size, 4);
        le(&mut image, entry + 12, *rva, 4);
        le(&mut image, entry + 16, *raw, 4);
    }
    for (at, target) in POINTERS {
        le(&mut image, at, base + target, 8);
    }
    // One block for page 0x1000: three DIR64 entries and a padding entry.
    le(&mut image, 0x2000, 0x1000, 4);
    le(&mut image, 0x2004, 0x10, 4);
    for (index, entry) in [0xa008u64, 0xa010, 0xaff8, 0x0000].iter().enumerate() {
        le(&mut image, 0x2008 + 2 * index, *entry, 2);
    }
    let parsed = parse(&image).unwrap();
    (image, parsed)
}

fn apply(image: &mut [u8], pe: &Pe, delta: u64) -> Result<u32, Error> {
    unsafe { apply_relocations(image.as_mut_ptr(), image.len(), pe.reloc_rva, pe.reloc_size, delta) }
}

#[test]
fn headers_are_read() {
    let (_, pe) = synthetic(0x1_4000_0000);
    assert_eq!(pe.size_of_image, 0x3000);
    assert_eq!(pe.size_of_headers, 0x400);
    assert_eq!(pe.entry_rva, 0x1000);
    assert_eq!((pe.reloc_rva, pe.reloc_size), (0x2000, 0x10));
    assert_eq!(pe.section_count, 2);
    assert_eq!(pe.sections[0], Section { rva: 0x1000, virtual_size: 0x1000, raw_size: 0x1000 });
    assert!(parse(b"not a pe").is_none());
    assert!(parse(&[0u8; 4096]).is_none());
}

#[test]
fn relocations_move_exactly_the_named_pointers() {
    let (mut image, pe) = synthetic(0x1_4000_0000);
    let before = image.clone();
    let delta = 0x2345_0000u64;
    assert_eq!(apply(&mut image, &pe, delta), Ok(3));
    for (at, target) in POINTERS {
        let value = u64::from_le_bytes(image[at..at + 8].try_into().unwrap());
        assert_eq!(value, 0x1_4000_0000 + target + delta);
    }
    let mut changed = 0;
    for (index, (old, new)) in before.iter().zip(&image).enumerate() {
        if old != new {
            assert!(POINTERS.iter().any(|(at, _)| (*at..*at + 8).contains(&index)));
            changed += 1;
        }
    }
    assert!(changed > 0);
    apply(&mut image, &pe, delta.wrapping_neg()).unwrap();
    assert_eq!(image, before);
}

#[test]
fn malformed_tables_are_refused_not_followed() {
    let (image, pe) = synthetic(0x1_4000_0000);
    let mut copy = image.clone();
    assert_eq!(
        unsafe { apply_relocations(copy.as_mut_ptr(), copy.len(), 0, 0, 1) },
        Err(Error::NoRelocations)
    );
    assert_eq!(
        unsafe { apply_relocations(copy.as_mut_ptr(), copy.len(), 0x2f00, 0x400, 1) },
        Err(Error::OutOfRange)
    );
    let mut bad = image.clone();
    le(&mut bad, 0x2004, 4, 4);
    assert_eq!(apply(&mut bad, &pe, 1), Err(Error::Malformed));
    let mut long = image.clone();
    le(&mut long, 0x2004, 0x100, 4);
    assert_eq!(apply(&mut long, &pe, 1), Err(Error::Malformed));
    let mut kind = image.clone();
    le(&mut kind, 0x2008, 0x3008, 2);
    assert_eq!(apply(&mut kind, &pe, 1), Err(Error::UnsupportedType));
    // A pointer that would straddle the end of the image.
    let mut edge = image.clone();
    le(&mut edge, 0x2000, 0x2000, 4);
    le(&mut edge, 0x2008, 0xaff9, 2);
    assert_eq!(apply(&mut edge, &pe, 1), Err(Error::OutOfRange));
}

#[test]
fn slots_are_aligned_inside_regions_and_uniform() {
    let size = 10 * 1024 * 1024;
    let regions = vec![(0x10_0000u64, 0x1000_0000u64), (0x1_0000_0000, 0x1_0000_0000 + 0x1000_0000)];
    let mut counts = std::collections::HashMap::new();
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut slots = 0;
    for _ in 0..20_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let (start, total) = pick_slot(regions.iter().copied(), size, state).unwrap();
        slots = total;
        assert_eq!(start % ALIGNMENT, 0);
        assert!(regions.iter().any(|(s, e)| start >= *s && start + size <= *e), "{start:#x}");
        *counts.entry(start).or_insert(0u32) += 1;
    }
    assert_eq!(counts.len() as u64, slots, "every slot should be reachable");
    let mean = 20_000.0 / slots as f64;
    let max = *counts.values().max().unwrap() as f64;
    assert!(max < mean * 3.0 + 10.0, "max {max} mean {mean}");
    // A bigger region gets proportionally more of the picks.
    let in_second: u32 = counts.iter().filter(|(start, _)| **start >= 0x1_0000_0000).map(|(_, n)| *n).sum();
    assert!(in_second > 20_000 / 3 && in_second < 20_000 * 2 / 3 + 2000, "{in_second}");
}

#[test]
fn slot_edge_cases() {
    let size = 4 * 1024 * 1024;
    assert_eq!(pick_slot([(0x10_0000u64, 0x10_0000 + size - 1)].into_iter(), size, 7), None);
    assert_eq!(pick_slot([(0x10_1000u64, 0x30_0000)].into_iter(), size, 7), None);
    assert_eq!(
        pick_slot([(0x20_0000u64, 0x20_0000 + size)].into_iter(), size, 123),
        Some((0x20_0000, 1))
    );
    // Start not aligned: the first slot is rounded up (only 0x400000 fits).
    let (start, total) = pick_slot([(0x20_1000u64, 0x80_0000)].into_iter(), size, 0).unwrap();
    assert_eq!((start, total), (0x40_0000, 1));
    // A wider region has two: 0x400000 and 0x600000.
    let (start, total) = pick_slot([(0x20_1000u64, 0xa0_0000)].into_iter(), size, 1).unwrap();
    assert_eq!((start, total), (0x60_0000, 2));
    assert_eq!(pick_slot(std::iter::empty(), size, 1), None);
}

fn relocation_sites(image: &[u8], rva: usize, size: usize) -> Vec<usize> {
    let table = &image[rva..rva + size];
    let mut sites = Vec::new();
    let mut at = 0;
    while at + 8 <= table.len() {
        let page = u32::from_le_bytes(table[at..at + 4].try_into().unwrap()) as usize;
        let block = u32::from_le_bytes(table[at + 4..at + 8].try_into().unwrap()) as usize;
        for entry in (at + 8..at + block).step_by(2) {
            let value = u16::from_le_bytes(table[entry..entry + 2].try_into().unwrap());
            if value >> 12 == 10 {
                sites.push(page + usize::from(value & 0xfff));
            }
        }
        at += block;
    }
    sites
}

/// The real boot image, laid out the way firmware does and moved twice.
#[test]
fn the_real_boot_image_relocates_consistently() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../build/esp/EFI/BOOT/BOOTX64.EFI");
    let Ok(file) = std::fs::read(path) else {
        eprintln!("no build/esp/EFI/BOOT/BOOTX64.EFI; build the kernel to run this test");
        return;
    };
    let pe = parse(&file).expect("headers");
    assert!(pe.reloc_rva != 0 && pe.size_of_image > 1 << 20);
    let header = u32::from_le_bytes(file[0x3c..0x40].try_into().unwrap()) as usize;
    let optional = header + 24;
    let preferred = u64::from_le_bytes(file[optional + 24..optional + 32].try_into().unwrap());
    let optional_size = u16::from_le_bytes(file[header + 20..header + 22].try_into().unwrap()) as usize;
    let section_table = optional + optional_size;
    let place = |image: &mut Vec<u8>| {
        image[..pe.size_of_headers].copy_from_slice(&file[..pe.size_of_headers]);
        for index in 0..pe.section_count {
            let entry = section_table + 40 * index;
            let raw_ptr = u32::from_le_bytes(file[entry + 20..entry + 24].try_into().unwrap()) as usize;
            let section = pe.sections[index];
            let count = section.raw_size.min(section.virtual_size);
            image[section.rva..section.rva + count].copy_from_slice(&file[raw_ptr..raw_ptr + count]);
        }
    };
    let mut first = vec![0u8; pe.size_of_image];
    place(&mut first);
    let base0 = 0x0480_0000u64;
    let applied = apply(&mut first, &pe, base0.wrapping_sub(preferred)).unwrap();
    assert!(applied > 4000, "{applied}");

    // Every relocated site now holds an address inside the image at base0.
    let sites = relocation_sites(&first, pe.reloc_rva, pe.reloc_size);
    assert_eq!(sites.len() as u32, applied);
    for site in &sites {
        let value = u64::from_le_bytes(first[*site..*site + 8].try_into().unwrap());
        assert!(
            value >= base0 && value <= base0 + pe.size_of_image as u64,
            "site {site:#x} holds {value:#x}, outside the image"
        );
    }

    // The kernel's move: copy the loaded image (headers and raw data only,
    // the rest stays zero) and add the new delta.
    let mut second = vec![0u8; pe.size_of_image];
    second[..pe.size_of_headers].copy_from_slice(&first[..pe.size_of_headers]);
    for index in 0..pe.section_count {
        let section = pe.sections[index];
        let count = section.raw_size.min(section.virtual_size);
        second[section.rva..section.rva + count]
            .copy_from_slice(&first[section.rva..section.rva + count]);
    }
    let base1 = 0x1ac0_0000u64;
    let moved = apply(&mut second, &pe, base1.wrapping_sub(base0)).unwrap();
    assert_eq!(moved, applied);
    for site in &sites {
        let old = u64::from_le_bytes(first[*site..*site + 8].try_into().unwrap());
        let new = u64::from_le_bytes(second[*site..*site + 8].try_into().unwrap());
        assert_eq!(new, old - base0 + base1);
        assert!(new >= base1 && new <= base1 + pe.size_of_image as u64);
    }
    // The code bytes are identical: only data pointers move.
    let entry = pe.entry_rva;
    assert_eq!(first[entry..entry + 64], second[entry..entry + 64]);
}
