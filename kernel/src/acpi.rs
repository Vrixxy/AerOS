const SDT_HEADER_SIZE: usize = 36;
const MAX_SDT_SIZE: usize = 1024 * 1024;
const MAX_ROOT_ENTRIES: usize = 4096;
pub const MAX_PROCESSORS: usize = 8;
const MAX_OVERRIDES: usize = 16;

#[derive(Clone, Copy)]
pub struct InterruptOverride {
    pub source: u8,
    pub gsi: u32,
    pub flags: u16,
}

impl InterruptOverride {
    const EMPTY: Self = Self {
        source: 0,
        gsi: 0,
        flags: 0,
    };
}

#[derive(Clone, Copy)]
pub struct AcpiInfo {
    pub revision: u8,
    pub root_table: u64,
    pub valid: bool,
    pub table_count: usize,
    pub madt_address: u64,
    pub hpet_address: u64,
    pub hpet_table: u64,
    pub hpet_address_space: u8,
    pub hpet_bit_width: u8,
    pub local_apic_address: u64,
    pub processor_count: u16,
    pub enabled_processor_count: u16,
    pub enabled_apic_ids: [u32; MAX_PROCESSORS],
    pub stored_processor_count: u8,
    pub io_apic_count: u8,
    pub io_apic_address: u64,
    pub io_apic_gsi_base: u32,
    pub interrupt_override_count: u8,
    interrupt_overrides: [InterruptOverride; MAX_OVERRIDES],
    stored_override_count: u8,
    pub madt_valid: bool,
    pub hpet_valid: bool,
    pub facp_address: u64,
    pub dsdt_address: u64,
    pub smi_command_port: u32,
    pub acpi_enable_value: u8,
    pub pm1a_control_block: u32,
    pub pm1b_control_block: u32,
    pub pm1_control_length: u8,
    pub fadt_valid: bool,
}

impl AcpiInfo {
    const MISSING: Self = Self {
        revision: 0,
        root_table: 0,
        valid: false,
        table_count: 0,
        madt_address: 0,
        hpet_address: 0,
        hpet_table: 0,
        hpet_address_space: 0xff,
        hpet_bit_width: 0,
        local_apic_address: 0,
        processor_count: 0,
        enabled_processor_count: 0,
        enabled_apic_ids: [0; MAX_PROCESSORS],
        stored_processor_count: 0,
        io_apic_count: 0,
        io_apic_address: 0,
        io_apic_gsi_base: 0,
        interrupt_override_count: 0,
        interrupt_overrides: [InterruptOverride::EMPTY; MAX_OVERRIDES],
        stored_override_count: 0,
        madt_valid: false,
        hpet_valid: false,
        facp_address: 0,
        dsdt_address: 0,
        smi_command_port: 0,
        acpi_enable_value: 0,
        pm1a_control_block: 0,
        pm1b_control_block: 0,
        pm1_control_length: 0,
        fadt_valid: false,
    };

    pub fn legacy_route(&self, source: u8) -> InterruptOverride {
        self.interrupt_overrides[..self.stored_override_count as usize]
            .iter()
            .copied()
            .find(|entry| entry.source == source)
            .unwrap_or(InterruptOverride {
                source,
                gsi: source as u32,
                flags: 0,
            })
    }
}

pub unsafe fn inspect(rsdp_address: u64) -> AcpiInfo {
    if rsdp_address == 0 {
        return AcpiInfo::MISSING;
    }
    let base = rsdp_address as usize as *const u8;
    if unsafe { read_signature8(base) } != *b"RSD PTR " || unsafe { checksum(base, 20) } != 0 {
        return AcpiInfo::MISSING;
    }
    let revision = unsafe { core::ptr::read_volatile(base.add(15)) };
    let (root_table, entry_size, root_signature) = if revision >= 2 {
        let length = (unsafe { read_u32(base.add(20)) }) as usize;
        if !(36..=4096).contains(&length) || unsafe { checksum(base, length) } != 0 {
            return AcpiInfo {
                revision,
                ..AcpiInfo::MISSING
            };
        }
        (unsafe { read_u64(base.add(24)) }, 8usize, *b"XSDT")
    } else {
        ((unsafe { read_u32(base.add(16)) }) as u64, 4usize, *b"RSDT")
    };
    if root_table == 0 {
        return AcpiInfo {
            revision,
            ..AcpiInfo::MISSING
        };
    }
    let root = root_table as usize as *const u8;
    let Some(root_length) = (unsafe { validate_sdt(root, Some(root_signature)) }) else {
        return AcpiInfo {
            revision,
            root_table,
            ..AcpiInfo::MISSING
        };
    };
    let table_count = ((root_length - SDT_HEADER_SIZE) / entry_size).min(MAX_ROOT_ENTRIES);
    let mut madt_address = 0u64;
    let mut hpet_table = 0u64;
    let mut facp_address = 0u64;
    for index in 0..table_count {
        let entry = unsafe { root.add(SDT_HEADER_SIZE + index * entry_size) };
        let address = if entry_size == 8 {
            unsafe { read_u64(entry) }
        } else {
            (unsafe { read_u32(entry) }) as u64
        };
        if address == 0 {
            continue;
        }
        let table = address as usize as *const u8;
        let signature = unsafe { read_signature4(table) };
        if signature == *b"APIC" && unsafe { validate_sdt(table, Some(*b"APIC")) }.is_some() {
            madt_address = address;
        }
        if signature == *b"HPET" && unsafe { validate_sdt(table, Some(*b"HPET")) }.is_some() {
            hpet_table = address;
        }
        if signature == *b"FACP" && unsafe { validate_sdt(table, Some(*b"FACP")) }.is_some() {
            facp_address = address;
        }
    }
    let mut info = AcpiInfo {
        revision,
        root_table,
        valid: true,
        table_count,
        madt_address,
        facp_address,
        ..AcpiInfo::MISSING
    };
    if madt_address != 0 {
        unsafe {
            parse_madt(&mut info);
        }
    }
    if hpet_table != 0 {
        unsafe {
            parse_hpet(&mut info, hpet_table);
        }
    }
    if facp_address != 0 {
        unsafe {
            parse_fadt(&mut info);
        }
    }
    info
}

unsafe fn parse_fadt(info: &mut AcpiInfo) {
    let base = info.facp_address as usize as *const u8;
    let Some(length) = (unsafe { validate_sdt(base, Some(*b"FACP")) }) else {
        return;
    };
    if length < 90 {
        return;
    }
    let dsdt = (unsafe { read_u32(base.add(40)) }) as u64;
    let smi_cmd = unsafe { read_u32(base.add(48)) };
    let acpi_enable = unsafe { core::ptr::read_volatile(base.add(52)) };
    let pm1a_cnt = unsafe { read_u32(base.add(64)) };
    let pm1b_cnt = unsafe { read_u32(base.add(68)) };
    let pm1_cnt_len = unsafe { core::ptr::read_volatile(base.add(89)) };
    info.dsdt_address = dsdt;
    info.smi_command_port = smi_cmd;
    info.acpi_enable_value = acpi_enable;
    info.pm1a_control_block = pm1a_cnt;
    info.pm1b_control_block = pm1b_cnt;
    info.pm1_control_length = pm1_cnt_len;
    info.fadt_valid = dsdt != 0 && pm1a_cnt != 0 && pm1_cnt_len != 0;
}

unsafe fn parse_hpet(info: &mut AcpiInfo, table_address: u64) {
    info.hpet_table = table_address;
    let base = table_address as usize as *const u8;
    let Some(length) = (unsafe { validate_sdt(base, Some(*b"HPET")) }) else {
        return;
    };
    if length < 56 {
        return;
    }
    let address_space = unsafe { core::ptr::read_volatile(base.add(40)) };
    let bit_width = unsafe { core::ptr::read_volatile(base.add(41)) };
    let bit_offset = unsafe { core::ptr::read_volatile(base.add(42)) };
    let address = unsafe { read_u64(base.add(44)) };
    info.hpet_address_space = address_space;
    info.hpet_bit_width = bit_width;
    if address_space == 0 && matches!(bit_width, 0 | 32 | 64) && bit_offset == 0 && address != 0 {
        info.hpet_address = address;
        info.hpet_valid = true;
    }
}

unsafe fn parse_madt(info: &mut AcpiInfo) {
    let base = info.madt_address as usize as *const u8;
    let Some(length) = (unsafe { validate_sdt(base, Some(*b"APIC")) }) else {
        return;
    };
    if length < 44 {
        return;
    }
    info.local_apic_address = (unsafe { read_u32(base.add(36)) }) as u64;
    let mut offset = 44usize;
    while offset + 2 <= length {
        let entry = unsafe { base.add(offset) };
        let kind = unsafe { core::ptr::read_volatile(entry) };
        let entry_length = unsafe { core::ptr::read_volatile(entry.add(1)) } as usize;
        if entry_length < 2 || offset + entry_length > length {
            return;
        }
        match kind {
            0 if entry_length >= 8 => {
                info.processor_count = info.processor_count.saturating_add(1);
                let flags = unsafe { read_u32(entry.add(4)) };
                if flags & 0x03 != 0 {
                    info.enabled_processor_count = info.enabled_processor_count.saturating_add(1);
                    store_processor(info, unsafe {
                        core::ptr::read_volatile(entry.add(3)) as u32
                    });
                }
            }
            1 if entry_length >= 12 => {
                info.io_apic_count = info.io_apic_count.saturating_add(1);
                if info.io_apic_address == 0 {
                    info.io_apic_address = unsafe { read_u32(entry.add(4)) } as u64;
                    info.io_apic_gsi_base = unsafe { read_u32(entry.add(8)) };
                }
            }
            2 if entry_length >= 10 => {
                info.interrupt_override_count = info.interrupt_override_count.saturating_add(1);
                if unsafe { core::ptr::read_volatile(entry.add(2)) } == 0
                    && info.stored_override_count as usize != MAX_OVERRIDES
                {
                    info.interrupt_overrides[info.stored_override_count as usize] =
                        InterruptOverride {
                            source: unsafe { core::ptr::read_volatile(entry.add(3)) },
                            gsi: unsafe { read_u32(entry.add(4)) },
                            flags: unsafe { read_u16(entry.add(8)) },
                        };
                    info.stored_override_count += 1;
                }
            }
            5 if entry_length >= 12 => {
                info.local_apic_address = unsafe { read_u64(entry.add(4)) };
            }
            9 if entry_length >= 16 => {
                info.processor_count = info.processor_count.saturating_add(1);
                let flags = unsafe { read_u32(entry.add(8)) };
                if flags & 0x03 != 0 {
                    info.enabled_processor_count = info.enabled_processor_count.saturating_add(1);
                    store_processor(info, unsafe { read_u32(entry.add(4)) });
                }
            }
            _ => {}
        }
        offset += entry_length;
    }
    info.madt_valid = offset == length
        && info.local_apic_address != 0
        && info.enabled_processor_count != 0
        && info.io_apic_count != 0
        && info.io_apic_address != 0;
}

fn store_processor(info: &mut AcpiInfo, apic_id: u32) {
    if info.enabled_apic_ids[..info.stored_processor_count as usize].contains(&apic_id)
        || info.stored_processor_count as usize == MAX_PROCESSORS
    {
        return;
    }
    info.enabled_apic_ids[info.stored_processor_count as usize] = apic_id;
    info.stored_processor_count += 1;
}

pub(crate) unsafe fn validate_sdt(base: *const u8, expected: Option<[u8; 4]>) -> Option<usize> {
    if base.is_null() {
        return None;
    }
    if expected.is_some_and(|signature| unsafe { read_signature4(base) } != signature) {
        return None;
    }
    let length = (unsafe { read_u32(base.add(4)) }) as usize;
    if !(SDT_HEADER_SIZE..=MAX_SDT_SIZE).contains(&length) {
        return None;
    }
    if unsafe { checksum(base, length) } != 0 {
        return None;
    }
    Some(length)
}

unsafe fn read_signature8(base: *const u8) -> [u8; 8] {
    let mut value = [0u8; 8];
    for (index, byte) in value.iter_mut().enumerate() {
        *byte = unsafe { core::ptr::read_volatile(base.add(index)) };
    }
    value
}

unsafe fn read_signature4(base: *const u8) -> [u8; 4] {
    let mut value = [0u8; 4];
    for (index, byte) in value.iter_mut().enumerate() {
        *byte = unsafe { core::ptr::read_volatile(base.add(index)) };
    }
    value
}

unsafe fn checksum(base: *const u8, length: usize) -> u8 {
    let mut sum = 0u8;
    for offset in 0..length {
        sum = sum.wrapping_add(unsafe { core::ptr::read_volatile(base.add(offset)) });
    }
    sum
}

unsafe fn read_u32(address: *const u8) -> u32 {
    unsafe { core::ptr::read_unaligned(address.cast::<u32>()) }
}

unsafe fn read_u16(address: *const u8) -> u16 {
    unsafe { core::ptr::read_unaligned(address.cast::<u16>()) }
}

unsafe fn read_u64(address: *const u8) -> u64 {
    unsafe { core::ptr::read_unaligned(address.cast::<u64>()) }
}
