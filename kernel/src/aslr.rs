//! Kernel image randomisation: the pure parts. At boot the kernel copies
//! itself to a random 2 MiB-aligned place in free memory, re-applies its own
//! base relocations for the new address and continues there, so the kernel's
//! code and data sit somewhere an attacker cannot guess. `uefi.rs` does the
//! firmware calls; everything here is `core` only so `tools/aslr-host` can
//! test it, including against the real boot image.
//!
//! What this hides is the kernel *image*. Physical memory is still mapped at
//! its own addresses (the kernel runs on an identity map), so the layout of
//! RAM is known and a read primitive that can scan RAM would still find the
//! image; the entropy is the number of free 2 MiB slots (about 7 bits in a
//! 512 MiB machine, 13 or more on a real one).

#![allow(dead_code)]

pub const ALIGNMENT: u64 = 0x20_0000;
pub const MAX_SECTIONS: usize = 16;

const DIR64: u16 = 10;
const ABSOLUTE: u16 = 0;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Section {
    pub rva: usize,
    pub virtual_size: usize,
    pub raw_size: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pe {
    pub size_of_image: usize,
    pub size_of_headers: usize,
    pub entry_rva: usize,
    pub reloc_rva: usize,
    pub reloc_size: usize,
    pub sections: [Section; MAX_SECTIONS],
    pub section_count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    NoRelocations,
    Malformed,
    UnsupportedType,
    OutOfRange,
}

fn le16(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn le32(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// Reads the headers of a PE32+ image (the first page of it is enough).
pub fn parse(headers: &[u8]) -> Option<Pe> {
    if headers.get(..2)? != b"MZ" {
        return None;
    }
    let pe = le32(headers, 0x3c)?;
    if headers.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let section_count = le16(headers, pe + 6)?;
    let optional_size = le16(headers, pe + 20)?;
    let optional = pe + 24;
    if le16(headers, optional)? != 0x20b || section_count > MAX_SECTIONS {
        return None;
    }
    let directory_count = le32(headers, optional + 108)?;
    let (reloc_rva, reloc_size) = if directory_count > 5 {
        (
            le32(headers, optional + 112 + 8 * 5)?,
            le32(headers, optional + 112 + 8 * 5 + 4)?,
        )
    } else {
        (0, 0)
    };
    let mut sections = [Section::default(); MAX_SECTIONS];
    let table = optional + optional_size;
    for (index, slot) in sections.iter_mut().enumerate().take(section_count) {
        let entry = table + 40 * index;
        *slot = Section {
            virtual_size: le32(headers, entry + 8)?,
            rva: le32(headers, entry + 12)?,
            raw_size: le32(headers, entry + 16)?,
        };
    }
    Some(Pe {
        size_of_image: le32(headers, optional + 56)?,
        size_of_headers: le32(headers, optional + 60)?,
        entry_rva: le32(headers, optional + 16)?,
        reloc_rva,
        reloc_size,
        sections,
        section_count,
    })
}

/// A slot start chosen uniformly among every `ALIGNMENT`-aligned position at
/// which `size` bytes fit inside one of the `(start, end)` free regions;
/// also returns how many slots there were.
pub fn pick_slot(
    regions: impl Iterator<Item = (u64, u64)> + Clone,
    size: u64,
    random: u64,
) -> Option<(u64, u64)> {
    let slots = |(start, end): (u64, u64)| -> (u64, u64) {
        let first = start.div_ceil(ALIGNMENT) * ALIGNMENT;
        if end < size || first > end - size {
            (first, 0)
        } else {
            (first, (end - size - first) / ALIGNMENT + 1)
        }
    };
    let total: u64 = regions.clone().map(|region| slots(region).1).sum();
    if total == 0 {
        return None;
    }
    let mut choice = random % total;
    for region in regions {
        let (first, count) = slots(region);
        if choice < count {
            return Some((first + choice * ALIGNMENT, total));
        }
        choice -= count;
    }
    None
}

/// Adds `delta` to every 64-bit address the relocation table names.
///
/// # Safety
/// `base` must point to `image_size` writable bytes holding an image whose
/// relocation table is at `reloc_rva`.
pub unsafe fn apply_relocations(
    base: *mut u8,
    image_size: usize,
    reloc_rva: usize,
    reloc_size: usize,
    delta: u64,
) -> Result<u32, Error> {
    if reloc_rva == 0 || reloc_size == 0 {
        return Err(Error::NoRelocations);
    }
    if reloc_rva
        .checked_add(reloc_size)
        .is_none_or(|end| end > image_size)
    {
        return Err(Error::OutOfRange);
    }
    let table = unsafe { core::slice::from_raw_parts(base.add(reloc_rva), reloc_size) };
    let mut applied = 0u32;
    let mut at = 0;
    while at + 8 <= table.len() {
        let page = le32(table, at).ok_or(Error::Malformed)?;
        let block = le32(table, at + 4).ok_or(Error::Malformed)?;
        if block < 8 || at + block > table.len() || block % 2 != 0 {
            return Err(Error::Malformed);
        }
        let mut entry = at + 8;
        while entry < at + block {
            let value = le16(table, entry).ok_or(Error::Malformed)? as u16;
            entry += 2;
            let kind = value >> 12;
            let target = page + usize::from(value & 0x0fff);
            match kind {
                ABSOLUTE => {}
                DIR64 => {
                    if target + 8 > image_size {
                        return Err(Error::OutOfRange);
                    }
                    let pointer = unsafe { base.add(target) }.cast::<u64>();
                    unsafe {
                        pointer.write_unaligned(pointer.read_unaligned().wrapping_add(delta));
                    }
                    applied += 1;
                }
                _ => return Err(Error::UnsupportedType),
            }
        }
        at += block;
    }
    Ok(applied)
}
