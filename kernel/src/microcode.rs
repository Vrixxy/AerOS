//! CPU microcode updates. A blob holds Intel update files (a 48-byte header,
//! the data and an optional extended signature table, one after another) or
//! an AMD container (`DMA\0`, an equivalence table and patch sections).
//! `find` picks the newest update for a processor; `apply_here` hands it to
//! the processor and returns the revision it reports afterwards.

#![allow(dead_code)]

use core::arch::asm;

const INTEL_HEADER: usize = 48;
const INTEL_DEFAULT_DATA: usize = 2000;
const INTEL_DEFAULT_TOTAL: usize = 2048;
const AMD_MAGIC: u32 = 0x0041_4d44;
const AMD_PATCH_HEADER: usize = 64;
const MSR_INTEL_TRIGGER: u32 = 0x79;
const MSR_AMD_TRIGGER: u32 = 0xc001_0020;
const MSR_REVISION: u32 = 0x8b;
const MSR_PLATFORM_ID: u32 = 0x17;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vendor {
    Intel,
    Amd,
}

/// What `find` needs to know about a processor.
#[derive(Clone, Copy, Debug)]
pub struct Cpu {
    pub vendor: Vendor,
    /// CPUID leaf 1, `eax`: family, model and stepping.
    pub signature: u32,
    /// One bit for the platform id (Intel; 0 elsewhere).
    pub platform: u32,
    /// The microcode revision the processor runs now.
    pub revision: u32,
}

/// An update inside a blob.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Patch {
    /// Where the bytes the processor is pointed at start.
    pub offset: usize,
    pub revision: u32,
    pub length: usize,
}

fn word(blob: &[u8], offset: usize) -> Option<u32> {
    let bytes = blob.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn half(blob: &[u8], offset: usize) -> Option<u16> {
    let bytes = blob.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn sum(blob: &[u8], start: usize, length: usize) -> Option<u32> {
    let end = start.checked_add(length)?;
    let bytes = blob.get(start..end)?;
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    Some(bytes.chunks_exact(4).fold(0u32, |total, chunk| {
        total.wrapping_add(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
    }))
}

fn platform_matches(flags: u32, platform: u32) -> bool {
    flags & platform != 0 || (flags == 0 && platform == 0)
}

fn newer(vendor: Vendor, candidate: u32, current: u32) -> bool {
    match vendor {
        Vendor::Intel => (candidate as i32) > (current as i32),
        Vendor::Amd => candidate > current,
    }
}

/// The update for `cpu` with the highest revision above the one it runs, if
/// the blob has one. Updates that fail their checksums are skipped.
pub fn find(blob: &[u8], cpu: &Cpu) -> Option<Patch> {
    match cpu.vendor {
        Vendor::Intel => find_intel(blob, cpu),
        Vendor::Amd => find_amd(blob, cpu),
    }
}

fn find_intel(blob: &[u8], cpu: &Cpu) -> Option<Patch> {
    let mut best: Option<Patch> = None;
    let mut offset = 0;
    while offset + INTEL_HEADER <= blob.len() {
        if word(blob, offset)? != 1 || word(blob, offset + 20)? != 1 {
            break;
        }
        let data = match word(blob, offset + 28)? as usize {
            0 => INTEL_DEFAULT_DATA,
            size => size,
        };
        let total = match word(blob, offset + 32)? as usize {
            0 => INTEL_DEFAULT_TOTAL,
            size => size,
        };
        if total < INTEL_HEADER + data || !total.is_multiple_of(4) || offset + total > blob.len() {
            break;
        }
        if let Some(revision) = intel_candidate(blob, offset, data, total, cpu)
            && newer(Vendor::Intel, revision, cpu.revision)
            && best.is_none_or(|found| newer(Vendor::Intel, revision, found.revision))
        {
            best = Some(Patch {
                offset: offset + INTEL_HEADER,
                revision,
                length: data,
            });
        }
        offset += total;
    }
    best
}

fn intel_candidate(
    blob: &[u8],
    offset: usize,
    data: usize,
    total: usize,
    cpu: &Cpu,
) -> Option<u32> {
    let revision = word(blob, offset + 4)?;
    let signature = word(blob, offset + 12)?;
    let checksum = word(blob, offset + 16)?;
    let flags = word(blob, offset + 24)?;
    if sum(blob, offset, INTEL_HEADER + data)? != 0 {
        return None;
    }
    if signature == cpu.signature && platform_matches(flags, cpu.platform) {
        return Some(revision);
    }
    let extended = offset + INTEL_HEADER + data;
    if total <= INTEL_HEADER + data {
        return None;
    }
    let count = word(blob, extended)? as usize;
    let table_length = 20usize.checked_add(count.checked_mul(12)?)?;
    if extended + table_length > offset + total || sum(blob, extended, table_length)? != 0 {
        return None;
    }
    let original = signature.wrapping_add(flags).wrapping_add(checksum);
    for index in 0..count {
        let at = extended + 20 + index * 12;
        let (sig, pf, check) = (word(blob, at)?, word(blob, at + 4)?, word(blob, at + 8)?);
        if sig.wrapping_add(pf).wrapping_add(check) != original {
            return None;
        }
        if sig == cpu.signature && platform_matches(pf, cpu.platform) {
            return Some(revision);
        }
    }
    None
}

fn find_amd(blob: &[u8], cpu: &Cpu) -> Option<Patch> {
    let mut best: Option<Patch> = None;
    let mut offset = 0;
    while offset + 12 <= blob.len() {
        if word(blob, offset)? != AMD_MAGIC {
            break;
        }
        let kind = word(blob, offset + 4)?;
        let length = word(blob, offset + 8)? as usize;
        if kind != 0 || !length.is_multiple_of(16) {
            break;
        }
        let table = offset + 12;
        if table + length > blob.len() {
            break;
        }
        let mut equivalent = 0u16;
        for entry in (table..table + length).step_by(16) {
            let installed = word(blob, entry)?;
            if installed == 0 {
                break;
            }
            if installed == cpu.signature {
                equivalent = half(blob, entry + 12)?;
                break;
            }
        }
        offset = table + length;
        while offset + 8 <= blob.len() && word(blob, offset)? == 1 {
            let size = word(blob, offset + 4)? as usize;
            let patch = offset + 8;
            if size < AMD_PATCH_HEADER || patch + size > blob.len() {
                return best;
            }
            let revision = word(blob, patch + 4)?;
            let target = half(blob, patch + 24)?;
            if equivalent != 0
                && target == equivalent
                && newer(Vendor::Amd, revision, cpu.revision)
                && best.is_none_or(|found| newer(Vendor::Amd, revision, found.revision))
            {
                best = Some(Patch {
                    offset: patch,
                    revision,
                    length: size,
                });
            }
            offset = patch + size;
        }
    }
    best
}

fn cpuid(leaf: u32) -> [u32; 4] {
    let (eax, ebx, ecx, edx);
    unsafe {
        asm!(
            "mov {saved:r}, rbx",
            "cpuid",
            "xchg {saved:r}, rbx",
            saved = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nomem, nostack, preserves_flags),
        );
    }
    [eax, ebx, ecx, edx]
}

/// True when a hypervisor reports itself: the host owns the microcode then.
pub fn virtualised() -> bool {
    cpuid(1)[2] & (1 << 31) != 0
}

pub fn vendor() -> Option<Vendor> {
    let id = cpuid(0);
    match (id[1], id[3], id[2]) {
        (0x756e_6547, 0x4965_6e69, 0x6c65_746e) => Some(Vendor::Intel),
        (0x6874_7541, 0x6974_6e65, 0x444d_4163) => Some(Vendor::Amd),
        _ => None,
    }
}

fn read_msr(register: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") register, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    }
    (u64::from(high) << 32) | u64::from(low)
}

unsafe fn write_msr(register: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") register, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nomem, nostack, preserves_flags));
    }
}

fn revision_here(vendor: Vendor) -> u32 {
    match vendor {
        Vendor::Intel => {
            unsafe { write_msr(MSR_REVISION, 0) };
            let _ = cpuid(1);
            (read_msr(MSR_REVISION) >> 32) as u32
        }
        Vendor::Amd => read_msr(MSR_REVISION) as u32,
    }
}

/// The running processor, or `None` under a hypervisor or for a vendor this
/// does not know (the model-specific registers are not safe to read there).
pub fn identify() -> Option<Cpu> {
    if virtualised() {
        return None;
    }
    let vendor = vendor()?;
    let platform = match vendor {
        Vendor::Intel => 1u32 << ((read_msr(MSR_PLATFORM_ID) >> 50) & 7),
        Vendor::Amd => 0,
    };
    Some(Cpu {
        vendor,
        signature: cpuid(1)[0],
        platform,
        revision: revision_here(vendor),
    })
}

/// Loads the update at `address` on this processor; returns the revision the
/// processor reports afterwards.
///
/// # Safety
/// `address` must be the start of a validated update (`Patch::offset` of a
/// blob that `find` accepted), mapped for the processor.
pub unsafe fn apply_here(cpu: &Cpu, address: u64) -> u32 {
    let trigger = match cpu.vendor {
        Vendor::Intel => MSR_INTEL_TRIGGER,
        Vendor::Amd => MSR_AMD_TRIGGER,
    };
    unsafe { write_msr(trigger, address) };
    revision_here(cpu.vendor)
}
