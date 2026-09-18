const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const MAX_SEGMENTS: usize = 8;
const USER_IMAGE_LIMIT: u64 = 509 * 4096;
const PT_LOAD: u32 = 1;
const PF_EXECUTE: u32 = 1;
const PF_WRITE: u32 = 2;
const PF_READ: u32 = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    Truncated,
    BadMagic,
    UnsupportedClass,
    UnsupportedEncoding,
    UnsupportedVersion,
    UnsupportedAbi,
    UnsupportedType,
    UnsupportedMachine,
    InvalidHeader,
    TooManySegments,
    InvalidSegment,
    SegmentOverlap,
    WriteExecute,
    MissingEntry,
}

#[derive(Clone, Copy)]
pub struct LoadSegment {
    pub file_offset: usize,
    pub virtual_address: u64,
    pub file_size: usize,
    pub memory_size: usize,
    pub writable: bool,
    pub executable: bool,
}

impl LoadSegment {
    const EMPTY: Self = Self {
        file_offset: 0,
        virtual_address: 0,
        file_size: 0,
        memory_size: 0,
        writable: false,
        executable: false,
    };

    pub fn memory_end(&self) -> u64 {
        self.virtual_address + self.memory_size as u64
    }
}

pub struct ElfImage<'a> {
    bytes: &'a [u8],
    entry: u64,
    segments: [LoadSegment; MAX_SEGMENTS],
    segment_count: usize,
    program_header_count: usize,
}

impl<'a> ElfImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        if bytes.len() < ELF_HEADER_SIZE {
            return Err(ElfError::Truncated);
        }
        if bytes[..4] != *b"\x7fELF" {
            return Err(ElfError::BadMagic);
        }
        if bytes[4] != 2 {
            return Err(ElfError::UnsupportedClass);
        }
        if bytes[5] != 1 {
            return Err(ElfError::UnsupportedEncoding);
        }
        if bytes[6] != 1 || read_u32(bytes, 20)? != 1 {
            return Err(ElfError::UnsupportedVersion);
        }
        if bytes[7] != 0 && bytes[7] != 3 {
            return Err(ElfError::UnsupportedAbi);
        }
        if read_u16(bytes, 16)? != 3 {
            return Err(ElfError::UnsupportedType);
        }
        if read_u16(bytes, 18)? != 62 {
            return Err(ElfError::UnsupportedMachine);
        }
        if read_u16(bytes, 52)? as usize != ELF_HEADER_SIZE
            || read_u16(bytes, 54)? as usize != PROGRAM_HEADER_SIZE
        {
            return Err(ElfError::InvalidHeader);
        }
        let entry = read_u64(bytes, 24)?;
        let table_offset =
            usize::try_from(read_u64(bytes, 32)?).map_err(|_| ElfError::InvalidHeader)?;
        let header_count = read_u16(bytes, 56)? as usize;
        let table_size = header_count
            .checked_mul(PROGRAM_HEADER_SIZE)
            .ok_or(ElfError::InvalidHeader)?;
        if header_count == 0
            || table_offset
                .checked_add(table_size)
                .is_none_or(|end| end > bytes.len())
        {
            return Err(ElfError::InvalidHeader);
        }
        let mut image = Self {
            bytes,
            entry,
            segments: [LoadSegment::EMPTY; MAX_SEGMENTS],
            segment_count: 0,
            program_header_count: header_count,
        };
        for header_index in 0..header_count {
            let offset = table_offset + header_index * PROGRAM_HEADER_SIZE;
            if read_u32(bytes, offset)? != PT_LOAD {
                continue;
            }
            if image.segment_count == MAX_SEGMENTS {
                return Err(ElfError::TooManySegments);
            }
            let flags = read_u32(bytes, offset + 4)?;
            if flags & !(PF_READ | PF_WRITE | PF_EXECUTE) != 0
                || flags & PF_READ == 0
                || flags & (PF_WRITE | PF_EXECUTE) == PF_WRITE | PF_EXECUTE
            {
                return if flags & (PF_WRITE | PF_EXECUTE) == PF_WRITE | PF_EXECUTE {
                    Err(ElfError::WriteExecute)
                } else {
                    Err(ElfError::InvalidSegment)
                };
            }
            let file_offset = usize::try_from(read_u64(bytes, offset + 8)?)
                .map_err(|_| ElfError::InvalidSegment)?;
            let virtual_address = read_u64(bytes, offset + 16)?;
            let file_size = usize::try_from(read_u64(bytes, offset + 32)?)
                .map_err(|_| ElfError::InvalidSegment)?;
            let memory_size = usize::try_from(read_u64(bytes, offset + 40)?)
                .map_err(|_| ElfError::InvalidSegment)?;
            let alignment = read_u64(bytes, offset + 48)?;
            let memory_end = virtual_address
                .checked_add(memory_size as u64)
                .ok_or(ElfError::InvalidSegment)?;
            let file_end = file_offset
                .checked_add(file_size)
                .ok_or(ElfError::InvalidSegment)?;
            if memory_size == 0
                || file_size > memory_size
                || file_end > bytes.len()
                || memory_end > USER_IMAGE_LIMIT
                || (alignment > 1 && !alignment.is_power_of_two())
                || (alignment > 1
                    && virtual_address & (alignment - 1) != file_offset as u64 & (alignment - 1))
            {
                return Err(ElfError::InvalidSegment);
            }
            let segment = LoadSegment {
                file_offset,
                virtual_address,
                file_size,
                memory_size,
                writable: flags & PF_WRITE != 0,
                executable: flags & PF_EXECUTE != 0,
            };
            for previous in &image.segments[..image.segment_count] {
                if segment.virtual_address < previous.memory_end()
                    && segment.memory_end() > previous.virtual_address
                {
                    return Err(ElfError::SegmentOverlap);
                }
            }
            image.segments[image.segment_count] = segment;
            image.segment_count += 1;
        }
        if image.segment_count == 0
            || !image.segments().iter().any(|segment| {
                segment.executable
                    && entry >= segment.virtual_address
                    && entry < segment.memory_end()
            })
        {
            return Err(ElfError::MissingEntry);
        }
        Ok(image)
    }

    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    pub fn entry(&self) -> u64 {
        self.entry
    }

    pub fn segments(&self) -> &[LoadSegment] {
        &self.segments[..self.segment_count]
    }

    pub fn memory_bytes(&self) -> usize {
        self.segments()
            .iter()
            .map(|segment| segment.memory_size)
            .sum()
    }

    pub fn program_header_count(&self) -> usize {
        self.program_header_count
    }

    pub fn range_loaded(&self, address: u64, length: usize) -> bool {
        let Some(end) = address.checked_add(length as u64) else {
            return false;
        };
        self.segments().iter().any(|segment| {
            address >= segment.virtual_address
                && end <= segment.virtual_address + segment.file_size as u64
        })
    }

    pub fn self_test(&self) -> bool {
        self.segments().iter().all(|segment| {
            segment.file_size <= segment.memory_size
                && !(segment.writable && segment.executable)
                && segment.memory_end() <= USER_IMAGE_LIMIT
        }) && matches!(
            Self::parse(&self.bytes[..ELF_HEADER_SIZE - 1]),
            Err(ElfError::Truncated)
        )
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ElfError> {
    let value = bytes.get(offset..offset + 2).ok_or(ElfError::Truncated)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ElfError> {
    let value = bytes.get(offset..offset + 4).ok_or(ElfError::Truncated)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ElfError> {
    let value = bytes.get(offset..offset + 8).ok_or(ElfError::Truncated)?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}
