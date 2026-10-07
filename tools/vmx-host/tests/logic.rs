use std::collections::{HashMap, HashSet};

use vmx_host::vmx_logic::*;

/// Every `pub const NAME: u32 = 0x...;` inside `pub mod field`.
fn fields() -> Vec<(String, u32)> {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../kernel/src/vmx_logic.rs"
    ))
    .unwrap();
    let start = source.find("pub mod field {").unwrap();
    let end = source[start..].find("\n}\n").unwrap() + start;
    source[start..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("pub const ")?;
            let (name, value) = rest.split_once(": u32 = 0x")?;
            let value = u32::from_str_radix(value.trim_end_matches(';'), 16).ok()?;
            Some((name.to_string(), value))
        })
        .collect()
}

#[test]
fn vmcs_encodings_follow_the_sdm_layout() {
    let all = fields();
    assert!(all.len() > 100, "parsed only {} fields", all.len());
    let read_only = [
        "EXIT_REASON",
        "EXIT_INTERRUPTION_INFO",
        "EXIT_INSTRUCTION_LENGTH",
        "EXIT_QUALIFICATION",
        "VM_INSTRUCTION_ERROR",
        "GUEST_PHYSICAL_ADDRESS",
    ];
    let sixty_four = [
        "IO_BITMAP_A",
        "IO_BITMAP_B",
        "MSR_BITMAPS",
        "EPT_POINTER",
        "GUEST_VMCS_LINK_POINTER",
        "GUEST_DEBUGCTL",
        "GUEST_PAT",
        "GUEST_EFER",
        "HOST_PAT",
        "HOST_EFER",
        "GUEST_PHYSICAL_ADDRESS",
    ];
    let natural_suffixes = [
        "_CR0", "_CR3", "_CR4", "_BASE", "_DR7", "_RSP", "_RIP", "_RFLAGS", "_SYSENTER_ESP",
        "_SYSENTER_EIP",
    ];
    let natural = [
        "CR0_GUEST_HOST_MASK",
        "CR4_GUEST_HOST_MASK",
        "CR0_READ_SHADOW",
        "CR4_READ_SHADOW",
        "EXIT_QUALIFICATION",
        "GUEST_PENDING_DEBUG",
    ];

    let mut seen = HashSet::new();
    for (name, value) in &all {
        assert!(seen.insert(*value), "{name} repeats encoding {value:#x}");
        assert_eq!(value & 1, 0, "{name} is a high-half encoding");
        let kind = (value >> 10) & 3;
        let width = (value >> 13) & 3;
        let expected_kind = if read_only.contains(&name.as_str()) {
            1
        } else if name.starts_with("GUEST_") {
            2
        } else if name.starts_with("HOST_") {
            3
        } else {
            0
        };
        let expected_width = if name.ends_with("_SELECTOR") || name == "VPID" {
            0
        } else if sixty_four.contains(&name.as_str()) {
            1
        } else if natural.contains(&name.as_str())
            || natural_suffixes.iter().any(|suffix| name.ends_with(suffix))
        {
            3
        } else {
            2
        };
        assert_eq!(kind, expected_kind, "{name} ({value:#x}) has the wrong field type");
        assert_eq!(width, expected_width, "{name} ({value:#x}) has the wrong width");
    }
}

#[test]
fn guest_and_host_selectors_pair_up() {
    let all: HashMap<String, u32> = fields().into_iter().collect();
    let mut pairs = 0;
    for (name, value) in &all {
        if let Some(rest) = name.strip_prefix("HOST_")
            && rest.ends_with("_SELECTOR")
        {
            let guest = all[&format!("GUEST_{rest}")];
            // The host area has no LDTR selector, so host TR sits one slot
            // lower than the guest's.
            let expected = if rest == "TR_SELECTOR" { guest - 2 } else { guest } + 0x0400;
            assert_eq!(*value, expected, "{name} is not where the SDM puts it");
            pairs += 1;
        }
    }
    assert_eq!(pairs, 7);
}

#[test]
fn control_bits_are_adjusted_against_the_capability_msrs() {
    // Low half: must be one. High half: may be one.
    let capability = (0x0000_00ffu64 << 32) | 0x0000_0016;
    assert_eq!(adjust_controls(0x1, capability), Some(0x17));
    assert_eq!(adjust_controls(0x0, capability), Some(0x16));
    assert_eq!(adjust_controls(0x100, capability), None);
    assert_eq!(adjust_controls(0xff, capability), Some(0xff));
    assert_eq!(control_msr(1 << 55, msr::PINBASED, msr::TRUE_PINBASED), msr::TRUE_PINBASED);
    assert_eq!(control_msr(0, msr::PINBASED, msr::TRUE_PINBASED), msr::PINBASED);
    assert_eq!(apply_fixed_bits(0x8000_0011, 0x8000_0021, 0xffff_ffff), 0x8000_0031);
    assert_eq!(apply_fixed_bits(0xffff_ffff, 0, 0x7fff_ffff), 0x7fff_ffff);
}

#[test]
fn segment_attributes_convert_to_access_rights() {
    assert_eq!(access_rights_from_attributes(0x029b), 0x209b); // 64-bit code
    assert_eq!(access_rights_from_attributes(0x0093), 0x0093); // data
    assert_eq!(access_rights_from_attributes(0x009b), 0x009b); // real-mode code
    assert_eq!(access_rights_from_attributes(0x008b), 0x008b); // busy TSS
    assert_eq!(access_rights_from_attributes(0x0c9b), 0xc09b); // 32-bit code, 4 KiB limit
    assert_eq!(access_rights_from_attributes(0), 0);
    assert_eq!(UNUSABLE_SEGMENT, 0x10000);
}

fn mapped(bytes: u64, base: u64) -> (HashMap<u64, u64>, u64) {
    let (pml4, pdpt, pd) = (0x1000u64, 0x2000u64, 0x3000u64);
    let mut memory = HashMap::new();
    assert!(build_ept(pml4, pdpt, pd, base, bytes, |at, value| {
        memory.insert(at, value);
    }));
    (memory, pml4)
}

#[test]
fn ept_maps_guest_ram_at_an_offset_and_nothing_else() {
    let bytes = 32 * 1024 * 1024;
    let base = 0x4000_0000;
    let (memory, pml4) = mapped(bytes, base);
    let read = |at: u64| *memory.get(&at).unwrap_or(&0);
    for gpa in [0u64, 0x1000, 0x9004, 0x2_0000, 0x1f_ffff, 0x20_0000, 0x140_0000, bytes - 1] {
        let (hpa, permissions) = translate(read, pml4, gpa).unwrap();
        assert_eq!(hpa, base + gpa, "gpa {gpa:#x}");
        assert_eq!(permissions, EPT_ALL);
    }
    for gpa in [bytes, bytes + 0x1000, 1 << 30, 1 << 39, u64::MAX >> 16] {
        assert_eq!(translate(read, pml4, gpa), None, "gpa {gpa:#x} must be unmapped");
    }
}

#[test]
fn ept_every_page_translates_exactly() {
    let bytes = 8 * PAGE_2M;
    let base = 0x1_0000_0000;
    let (memory, pml4) = mapped(bytes, base);
    let read = |at: u64| *memory.get(&at).unwrap_or(&0);
    let mut gpa = 0;
    while gpa < bytes {
        assert_eq!(translate(read, pml4, gpa).map(|r| r.0), Some(base + gpa));
        gpa += 0x1000 - 1 + 1;
    }
}

#[test]
fn ept_entries_have_the_right_flags() {
    let (memory, pml4) = mapped(2 * PAGE_2M, 0x8000_0000);
    // Interior entries: read, write, execute and the address.
    assert_eq!(memory[&pml4], 0x2000 | 7);
    assert_eq!(memory[&0x2000], 0x3000 | 7);
    // Leaves: large page, write-back, rwx.
    assert_eq!(memory[&0x3000], 0x8000_0000 | (1 << 7) | (6 << 3) | 7);
    assert_eq!(memory[&0x3008], 0x8020_0000 | (1 << 7) | (6 << 3) | 7);
    assert_eq!(memory.len(), 4);
    assert_eq!(ept_pointer(0x1000), 0x1000 | (3 << 3) | 6);
}

#[test]
fn ept_refuses_unrepresentable_requests() {
    let mut sink = |_: u64, _: u64| {};
    assert!(!build_ept(0x1000, 0x2000, 0x3000, 0x8000_0000, 0, &mut sink));
    assert!(!build_ept(0x1000, 0x2000, 0x3000, 0x8000_1000, PAGE_2M, &mut sink));
    assert!(!build_ept(0x1000, 0x2000, 0x3000, 0x8000_0000, 513 * PAGE_2M, &mut sink));
    assert!(build_ept(0x1000, 0x2000, 0x3000, 0x8000_0000, 512 * PAGE_2M, &mut sink));
}

#[test]
fn walker_handles_four_kilobyte_pages_and_missing_levels() {
    // pml4 -> pdpt -> pd -> pt -> page, with permissions reduced at the leaf.
    let mut memory = HashMap::new();
    memory.insert(0x1000u64, 0x2000 | 7);
    memory.insert(0x2000, 0x3000 | 7);
    memory.insert(0x3000, 0x4000 | 7);
    memory.insert(0x4000 + 5 * 8, 0xabc000 | EPT_READ | EPT_EXECUTE);
    let read = |at: u64| *memory.get(&at).unwrap_or(&0);
    assert_eq!(
        translate(read, 0x1000, (5 << 12) | 0x123),
        Some((0xabc123, EPT_READ | EPT_EXECUTE))
    );
    assert_eq!(translate(read, 0x1000, 6 << 12), None);
    assert_eq!(translate(read, 0x1000, 1 << 21), None);
}

#[test]
fn io_exit_qualification() {
    let out_byte = (0x3f8u64 << 16) | 0;
    assert_eq!(
        decode_io(out_byte),
        IoExit { port: 0x3f8, size: 1, is_in: false, string: false, rep: false }
    );
    let in_dword = (0xcf8u64 << 16) | (1 << 3) | 3;
    assert_eq!(
        decode_io(in_dword),
        IoExit { port: 0xcf8, size: 4, is_in: true, string: false, rep: false }
    );
    let rep_outs = (0x3f8u64 << 16) | (1 << 4) | (1 << 5);
    let io = decode_io(rep_outs);
    assert!(io.string && io.rep && !io.is_in);
    assert_eq!(decode_io(1).size, 2);
}

#[test]
fn register_merging_and_rip() {
    assert_eq!(merge_register(0x1122_3344_5566_7788, 0xaa, 1), 0x1122_3344_5566_77aa);
    assert_eq!(merge_register(0x1122_3344_5566_7788, 0xbbcc, 2), 0x1122_3344_5566_bbcc);
    assert_eq!(merge_register(0x1122_3344_5566_7788, 0xddee_ff00, 4), 0xddee_ff00);
    assert_eq!(advance_rip(0x1000, 2), 0x1002);
    assert_eq!(advance_rip(u64::MAX, 1), 0);
    assert_eq!(ept_violation_access(0b101), (true, false, true));
}
