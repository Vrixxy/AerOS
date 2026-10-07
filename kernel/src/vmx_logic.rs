//! The parts of the Intel VT-x backend that need no hardware: VMCS field
//! encodings, control-bit adjustment against the capability MSRs, segment
//! access-rights conversion, the EPT table builder and a software EPT walker,
//! and exit-qualification decoding. `core` only, so `tools/vmx-host` can test
//! it on the host.
#![allow(dead_code)]

/// VMCS field encodings (Intel SDM volume 3, appendix B).
pub mod field {
    pub const VPID: u32 = 0x0000;

    pub const GUEST_ES_SELECTOR: u32 = 0x0800;
    pub const GUEST_CS_SELECTOR: u32 = 0x0802;
    pub const GUEST_SS_SELECTOR: u32 = 0x0804;
    pub const GUEST_DS_SELECTOR: u32 = 0x0806;
    pub const GUEST_FS_SELECTOR: u32 = 0x0808;
    pub const GUEST_GS_SELECTOR: u32 = 0x080a;
    pub const GUEST_LDTR_SELECTOR: u32 = 0x080c;
    pub const GUEST_TR_SELECTOR: u32 = 0x080e;

    pub const HOST_ES_SELECTOR: u32 = 0x0c00;
    pub const HOST_CS_SELECTOR: u32 = 0x0c02;
    pub const HOST_SS_SELECTOR: u32 = 0x0c04;
    pub const HOST_DS_SELECTOR: u32 = 0x0c06;
    pub const HOST_FS_SELECTOR: u32 = 0x0c08;
    pub const HOST_GS_SELECTOR: u32 = 0x0c0a;
    pub const HOST_TR_SELECTOR: u32 = 0x0c0c;

    pub const IO_BITMAP_A: u32 = 0x2000;
    pub const IO_BITMAP_B: u32 = 0x2002;
    pub const MSR_BITMAPS: u32 = 0x2004;
    pub const EPT_POINTER: u32 = 0x201a;

    pub const GUEST_VMCS_LINK_POINTER: u32 = 0x2800;
    pub const GUEST_DEBUGCTL: u32 = 0x2802;
    pub const GUEST_PAT: u32 = 0x2804;
    pub const GUEST_EFER: u32 = 0x2806;

    pub const HOST_PAT: u32 = 0x2c00;
    pub const HOST_EFER: u32 = 0x2c02;

    pub const PIN_BASED_CONTROLS: u32 = 0x4000;
    pub const PROC_BASED_CONTROLS: u32 = 0x4002;
    pub const EXCEPTION_BITMAP: u32 = 0x4004;
    pub const PAGE_FAULT_ERROR_MASK: u32 = 0x4006;
    pub const PAGE_FAULT_ERROR_MATCH: u32 = 0x4008;
    pub const CR3_TARGET_COUNT: u32 = 0x400a;
    pub const EXIT_CONTROLS: u32 = 0x400c;
    pub const EXIT_MSR_STORE_COUNT: u32 = 0x400e;
    pub const EXIT_MSR_LOAD_COUNT: u32 = 0x4010;
    pub const ENTRY_CONTROLS: u32 = 0x4012;
    pub const ENTRY_MSR_LOAD_COUNT: u32 = 0x4014;
    pub const ENTRY_INTERRUPTION_INFO: u32 = 0x4016;
    pub const ENTRY_EXCEPTION_ERROR_CODE: u32 = 0x4018;
    pub const ENTRY_INSTRUCTION_LENGTH: u32 = 0x401a;
    pub const SECONDARY_CONTROLS: u32 = 0x401e;

    pub const VM_INSTRUCTION_ERROR: u32 = 0x4400;
    pub const EXIT_REASON: u32 = 0x4402;
    pub const EXIT_INTERRUPTION_INFO: u32 = 0x4404;
    pub const EXIT_INSTRUCTION_LENGTH: u32 = 0x440c;

    pub const GUEST_ES_LIMIT: u32 = 0x4800;
    pub const GUEST_CS_LIMIT: u32 = 0x4802;
    pub const GUEST_SS_LIMIT: u32 = 0x4804;
    pub const GUEST_DS_LIMIT: u32 = 0x4806;
    pub const GUEST_FS_LIMIT: u32 = 0x4808;
    pub const GUEST_GS_LIMIT: u32 = 0x480a;
    pub const GUEST_LDTR_LIMIT: u32 = 0x480c;
    pub const GUEST_TR_LIMIT: u32 = 0x480e;
    pub const GUEST_GDTR_LIMIT: u32 = 0x4810;
    pub const GUEST_IDTR_LIMIT: u32 = 0x4812;
    pub const GUEST_ES_ACCESS: u32 = 0x4814;
    pub const GUEST_CS_ACCESS: u32 = 0x4816;
    pub const GUEST_SS_ACCESS: u32 = 0x4818;
    pub const GUEST_DS_ACCESS: u32 = 0x481a;
    pub const GUEST_FS_ACCESS: u32 = 0x481c;
    pub const GUEST_GS_ACCESS: u32 = 0x481e;
    pub const GUEST_LDTR_ACCESS: u32 = 0x4820;
    pub const GUEST_TR_ACCESS: u32 = 0x4822;
    pub const GUEST_INTERRUPTIBILITY: u32 = 0x4824;
    pub const GUEST_ACTIVITY_STATE: u32 = 0x4826;
    pub const GUEST_SYSENTER_CS: u32 = 0x482a;

    pub const HOST_SYSENTER_CS: u32 = 0x4c00;

    pub const CR0_GUEST_HOST_MASK: u32 = 0x6000;
    pub const CR4_GUEST_HOST_MASK: u32 = 0x6002;
    pub const CR0_READ_SHADOW: u32 = 0x6004;
    pub const CR4_READ_SHADOW: u32 = 0x6006;

    pub const EXIT_QUALIFICATION: u32 = 0x6400;
    pub const GUEST_PHYSICAL_ADDRESS: u32 = 0x2400;

    pub const GUEST_CR0: u32 = 0x6800;
    pub const GUEST_CR3: u32 = 0x6802;
    pub const GUEST_CR4: u32 = 0x6804;
    pub const GUEST_ES_BASE: u32 = 0x6806;
    pub const GUEST_CS_BASE: u32 = 0x6808;
    pub const GUEST_SS_BASE: u32 = 0x680a;
    pub const GUEST_DS_BASE: u32 = 0x680c;
    pub const GUEST_FS_BASE: u32 = 0x680e;
    pub const GUEST_GS_BASE: u32 = 0x6810;
    pub const GUEST_LDTR_BASE: u32 = 0x6812;
    pub const GUEST_TR_BASE: u32 = 0x6814;
    pub const GUEST_GDTR_BASE: u32 = 0x6816;
    pub const GUEST_IDTR_BASE: u32 = 0x6818;
    pub const GUEST_DR7: u32 = 0x681a;
    pub const GUEST_RSP: u32 = 0x681c;
    pub const GUEST_RIP: u32 = 0x681e;
    pub const GUEST_RFLAGS: u32 = 0x6820;
    pub const GUEST_PENDING_DEBUG: u32 = 0x6822;
    pub const GUEST_SYSENTER_ESP: u32 = 0x6824;
    pub const GUEST_SYSENTER_EIP: u32 = 0x6826;

    pub const HOST_CR0: u32 = 0x6c00;
    pub const HOST_CR3: u32 = 0x6c02;
    pub const HOST_CR4: u32 = 0x6c04;
    pub const HOST_FS_BASE: u32 = 0x6c06;
    pub const HOST_GS_BASE: u32 = 0x6c08;
    pub const HOST_TR_BASE: u32 = 0x6c0a;
    pub const HOST_GDTR_BASE: u32 = 0x6c0c;
    pub const HOST_IDTR_BASE: u32 = 0x6c0e;
    pub const HOST_SYSENTER_ESP: u32 = 0x6c10;
    pub const HOST_SYSENTER_EIP: u32 = 0x6c12;
    pub const HOST_RSP: u32 = 0x6c14;
    pub const HOST_RIP: u32 = 0x6c16;
}

/// Capability MSRs.
pub mod msr {
    pub const FEATURE_CONTROL: u32 = 0x3a;
    pub const VMX_BASIC: u32 = 0x480;
    pub const PINBASED: u32 = 0x481;
    pub const PROCBASED: u32 = 0x482;
    pub const EXIT: u32 = 0x483;
    pub const ENTRY: u32 = 0x484;
    pub const CR0_FIXED0: u32 = 0x486;
    pub const CR0_FIXED1: u32 = 0x487;
    pub const CR4_FIXED0: u32 = 0x488;
    pub const CR4_FIXED1: u32 = 0x489;
    pub const PROCBASED2: u32 = 0x48b;
    pub const EPT_VPID_CAP: u32 = 0x48c;
    pub const TRUE_PINBASED: u32 = 0x48d;
    pub const TRUE_PROCBASED: u32 = 0x48e;
    pub const TRUE_EXIT: u32 = 0x48f;
    pub const TRUE_ENTRY: u32 = 0x490;
}

pub const FEATURE_CONTROL_LOCK: u64 = 1;
pub const FEATURE_CONTROL_VMXON_OUTSIDE_SMX: u64 = 1 << 2;
pub const CR4_VMXE: u64 = 1 << 13;

pub mod pin {
    pub const EXTERNAL_INTERRUPT_EXITING: u32 = 1 << 0;
    pub const NMI_EXITING: u32 = 1 << 3;
}

pub mod proc {
    pub const HLT_EXITING: u32 = 1 << 7;
    pub const UNCONDITIONAL_IO_EXITING: u32 = 1 << 24;
    pub const SECONDARY_CONTROLS: u32 = 1 << 31;
}

pub mod secondary {
    pub const ENABLE_EPT: u32 = 1 << 1;
    pub const ENABLE_RDTSCP: u32 = 1 << 3;
    pub const UNRESTRICTED_GUEST: u32 = 1 << 7;
}

pub mod exit_control {
    pub const HOST_ADDRESS_SPACE_SIZE: u32 = 1 << 9;
    pub const SAVE_EFER: u32 = 1 << 20;
    pub const LOAD_EFER: u32 = 1 << 21;
}

pub mod entry_control {
    pub const IA32E_MODE_GUEST: u32 = 1 << 9;
    pub const LOAD_EFER: u32 = 1 << 15;
}

pub mod exit_reason {
    pub const EXCEPTION_OR_NMI: u32 = 0;
    pub const EXTERNAL_INTERRUPT: u32 = 1;
    pub const TRIPLE_FAULT: u32 = 2;
    pub const CPUID: u32 = 10;
    pub const HLT: u32 = 12;
    pub const VMCALL: u32 = 18;
    pub const IO_INSTRUCTION: u32 = 30;
    pub const RDMSR: u32 = 31;
    pub const WRMSR: u32 = 32;
    pub const INVALID_GUEST_STATE: u32 = 33;
    pub const MSR_LOADING_FAILURE: u32 = 34;
    pub const EPT_VIOLATION: u32 = 48;
    pub const EPT_MISCONFIGURATION: u32 = 49;
    pub const XSETBV: u32 = 55;
    /// Set in the exit-reason field when the VM entry itself failed.
    pub const ENTRY_FAILURE: u32 = 1 << 31;
}

/// Picks control bits for a control field. `capability` is the low/high pair
/// of a VMX capability MSR: the low half holds the bits that must be 1, the
/// high half the bits that may be 1. `None` when `wanted` asks for a bit the
/// processor cannot set.
pub const fn adjust_controls(wanted: u32, capability: u64) -> Option<u32> {
    let must_be_one = capability as u32;
    let may_be_one = (capability >> 32) as u32;
    if wanted & !may_be_one != 0 {
        return None;
    }
    Some(wanted | must_be_one)
}

/// The control MSR to read: the `TRUE_*` variants exist when bit 55 of
/// IA32_VMX_BASIC is set and are the ones that report what is really
/// required.
pub const fn control_msr(basic: u64, plain: u32, true_variant: u32) -> u32 {
    if basic & (1 << 55) != 0 {
        true_variant
    } else {
        plain
    }
}

/// CR0 or CR4 value that satisfies the fixed-bit MSRs: bits set in `fixed0`
/// must be 1, bits clear in `fixed1` must be 0.
pub const fn apply_fixed_bits(value: u64, fixed0: u64, fixed1: u64) -> u64 {
    (value | fixed0) & fixed1
}

/// A segment's attribute word as the AMD VMCB stores it (type, S, DPL, P in
/// bits 0-7; AVL, L, D/B, G in bits 8-11) converted to the VMX access-rights
/// layout (AVL, L, D/B, G in bits 12-15).
pub const fn access_rights_from_attributes(attributes: u16) -> u32 {
    ((attributes & 0xff) as u32) | (((attributes & 0xf00) as u32) << 4)
}

pub const UNUSABLE_SEGMENT: u32 = 1 << 16;

/// Page-table entry bits for EPT.
pub const EPT_READ: u64 = 1;
pub const EPT_WRITE: u64 = 2;
pub const EPT_EXECUTE: u64 = 4;
pub const EPT_ALL: u64 = 7;
pub const EPT_LARGE: u64 = 1 << 7;
pub const EPT_MEMORY_TYPE_WRITE_BACK: u64 = 6 << 3;

pub const PAGE_2M: u64 = 0x20_0000;

/// The EPT pointer for a PML4 table: write-back memory type, 4-level walk.
pub const fn ept_pointer(pml4: u64) -> u64 {
    pml4 | (3 << 3) | 6
}

/// Builds an identity-offset EPT: guest-physical `[0, bytes)` maps to
/// `host_base + gpa` with 2 MiB pages. `pml4`, `pdpt` and `pd` are the host
/// addresses of three zeroed tables; `write` stores one 64-bit entry. At most
/// 1 GiB (one page directory) can be mapped.
pub fn build_ept(
    pml4: u64,
    pdpt: u64,
    pd: u64,
    host_base: u64,
    bytes: u64,
    mut write: impl FnMut(u64, u64),
) -> bool {
    let entries = bytes.div_ceil(PAGE_2M);
    if entries == 0 || entries > 512 || !host_base.is_multiple_of(PAGE_2M) {
        return false;
    }
    write(pml4, pdpt | EPT_ALL);
    write(pdpt, pd | EPT_ALL);
    for index in 0..entries {
        write(
            pd + index * 8,
            (host_base + index * PAGE_2M) | EPT_LARGE | EPT_MEMORY_TYPE_WRITE_BACK | EPT_ALL,
        );
    }
    true
}

/// Walks an EPT in software the way the processor would: returns the host
/// physical address and the permission bits, or `None` if any level is not
/// present.
pub fn translate(read: impl Fn(u64) -> u64, pml4: u64, gpa: u64) -> Option<(u64, u64)> {
    let mut table = pml4;
    let mut permissions = EPT_ALL;
    for level in (1..=4u32).rev() {
        let shift = 12 + 9 * (level - 1);
        let index = (gpa >> shift) & 0x1ff;
        let entry = read(table + index * 8);
        if entry & EPT_ALL == 0 {
            return None;
        }
        permissions &= entry;
        let leaf = level == 1 || (entry & EPT_LARGE != 0 && level <= 3);
        if leaf {
            let mask = (1u64 << shift) - 1;
            let base = entry & 0x000f_ffff_ffff_f000 & !mask;
            return Some((base | (gpa & mask), permissions & EPT_ALL));
        }
        table = entry & 0x000f_ffff_ffff_f000;
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IoExit {
    pub port: u16,
    pub size: u8,
    pub is_in: bool,
    pub string: bool,
    pub rep: bool,
}

/// Decodes the exit qualification of an I/O-instruction exit.
pub const fn decode_io(qualification: u64) -> IoExit {
    IoExit {
        port: (qualification >> 16) as u16,
        size: ((qualification & 7) + 1) as u8,
        is_in: qualification & (1 << 3) != 0,
        string: qualification & (1 << 4) != 0,
        rep: qualification & (1 << 5) != 0,
    }
}

/// Bits of the EPT-violation exit qualification.
pub const fn ept_violation_access(qualification: u64) -> (bool, bool, bool) {
    (
        qualification & 1 != 0,
        qualification & 2 != 0,
        qualification & 4 != 0,
    )
}

/// New value of the low part of a general register after an operation of
/// `size` bytes: 8 and 16-bit writes keep the upper bits, 32-bit writes
/// clear them (as the hardware does for `in eax, dx`).
pub const fn merge_register(old: u64, value: u64, size: u8) -> u64 {
    match size {
        1 => (old & !0xff) | (value & 0xff),
        2 => (old & !0xffff) | (value & 0xffff),
        _ => value & 0xffff_ffff,
    }
}

/// Where the guest's execution continues after an instruction of `length`
/// bytes that the host emulated.
pub const fn advance_rip(rip: u64, length: u32) -> u64 {
    rip.wrapping_add(length as u64)
}
