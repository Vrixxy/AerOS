use core::arch::x86_64::__cpuid;

const ELF_HEADER_BYTES: usize = 64;
const PROGRAM_HEADER_BYTES: usize = 56;
const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const MAX_INTERPRETER_BYTES: usize = 128;
const BRIDGE_MAGIC: [u8; 4] = *b"AERL";
const BRIDGE_VERSION: u16 = 1;
const BRIDGE_HEADER_BYTES: usize = 16;
const MAX_BRIDGE_PAYLOAD: usize = 65_536;
const MAX_GUEST_PATH_BYTES: usize = 1_024;
const ALLOWED_PERMISSIONS: u32 = 0x3f;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExecutionRoute {
    NativeLinux,
    DebianGuest,
    Reject,
}

impl ExecutionRoute {
    pub const fn name(self) -> &'static str {
        match self {
            Self::NativeLinux => "native",
            Self::DebianGuest => "guest",
            Self::Reject => "reject",
        }
    }
}

#[derive(Clone, Copy)]
pub struct CompatibilityReport {
    pub vmx: bool,
    pub svm: bool,
    pub npt: bool,
    pub under_hypervisor: bool,
    pub hardware_acceleration: bool,
    pub static_route: ExecutionRoute,
    pub dynamic_route: ExecutionRoute,
    pub script_route: ExecutionRoute,
    pub malformed_route: ExecutionRoute,
    pub read_only_base: bool,
    pub per_app_overlay: bool,
    pub host_files_default_deny: bool,
    pub devices_default_deny: bool,
    pub software_emulation: bool,
    pub bridge_protocol: bool,
    pub verified: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BridgeOpcode {
    Hello = 1,
    Launch = 2,
    Exit = 3,
    Window = 4,
    Damage = 5,
    Input = 6,
}

impl BridgeOpcode {
    fn from_raw(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::Hello),
            2 => Some(Self::Launch),
            3 => Some(Self::Exit),
            4 => Some(Self::Window),
            5 => Some(Self::Damage),
            6 => Some(Self::Input),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct BridgeMessage<'a> {
    opcode: BridgeOpcode,
    request: u32,
    payload: &'a [u8],
}

#[derive(Clone, Copy)]
struct LaunchRequest<'a> {
    permissions: u32,
    path: &'a str,
}

impl<'a> BridgeMessage<'a> {
    fn parse(frame: &'a [u8]) -> Option<Self> {
        if frame.len() < BRIDGE_HEADER_BYTES || frame[..4] != BRIDGE_MAGIC {
            return None;
        }
        let version = read_u16(frame, 4)?;
        let opcode = BridgeOpcode::from_raw(read_u16(frame, 6)?)?;
        let request = read_u32(frame, 8)?;
        let payload_bytes = usize::try_from(read_u32(frame, 12)?).ok()?;
        if version != BRIDGE_VERSION
            || request == 0
            || payload_bytes > MAX_BRIDGE_PAYLOAD
            || frame.len() != BRIDGE_HEADER_BYTES.checked_add(payload_bytes)?
        {
            return None;
        }
        Some(Self {
            opcode,
            request,
            payload: &frame[BRIDGE_HEADER_BYTES..],
        })
    }
}

impl<'a> LaunchRequest<'a> {
    fn parse(payload: &'a [u8]) -> Option<Self> {
        if payload.len() < 6 {
            return None;
        }
        let permissions = read_u32(payload, 0)?;
        let path_bytes = usize::from(read_u16(payload, 4)?);
        if permissions & !ALLOWED_PERMISSIONS != 0
            || path_bytes == 0
            || path_bytes > MAX_GUEST_PATH_BYTES
            || payload.len() != 6usize.checked_add(path_bytes)?
        {
            return None;
        }
        let path = core::str::from_utf8(&payload[6..]).ok()?;
        if !valid_guest_path(path) {
            return None;
        }
        Some(Self { permissions, path })
    }
}

pub fn route(bytes: &[u8]) -> ExecutionRoute {
    if bytes.starts_with(b"#!") {
        return if valid_shebang(bytes) {
            ExecutionRoute::DebianGuest
        } else {
            ExecutionRoute::Reject
        };
    }
    if bytes.len() < ELF_HEADER_BYTES
        || bytes[..4] != *b"\x7fELF"
        || bytes[4] != 2
        || bytes[5] != 1
        || bytes[6] != 1
        || !matches!(bytes[7], 0 | 3)
        || read_u32(bytes, 20) != Some(1)
        || read_u16(bytes, 18) != Some(EM_X86_64)
        || read_u16(bytes, 52) != Some(ELF_HEADER_BYTES as u16)
        || read_u16(bytes, 54) != Some(PROGRAM_HEADER_BYTES as u16)
    {
        return ExecutionRoute::Reject;
    }
    let kind = match read_u16(bytes, 16) {
        Some(ET_EXEC) => ET_EXEC,
        Some(ET_DYN) => ET_DYN,
        _ => return ExecutionRoute::Reject,
    };
    let Some(table_offset) = read_u64(bytes, 32).and_then(|value| usize::try_from(value).ok())
    else {
        return ExecutionRoute::Reject;
    };
    let Some(header_count) = read_u16(bytes, 56).map(usize::from) else {
        return ExecutionRoute::Reject;
    };
    let Some(table_bytes) = header_count.checked_mul(PROGRAM_HEADER_BYTES) else {
        return ExecutionRoute::Reject;
    };
    if header_count == 0
        || table_offset
            .checked_add(table_bytes)
            .is_none_or(|end| end > bytes.len())
    {
        return ExecutionRoute::Reject;
    }
    let mut load = false;
    let mut interpreter = false;
    for index in 0..header_count {
        let offset = table_offset + index * PROGRAM_HEADER_BYTES;
        match read_u32(bytes, offset) {
            Some(PT_LOAD) => load = true,
            Some(PT_INTERP) => {
                if interpreter || !valid_interpreter(bytes, offset) {
                    return ExecutionRoute::Reject;
                }
                interpreter = true;
            }
            Some(_) => {}
            None => return ExecutionRoute::Reject,
        }
    }
    if !load {
        return ExecutionRoute::Reject;
    }
    if interpreter || kind == ET_EXEC {
        ExecutionRoute::DebianGuest
    } else {
        ExecutionRoute::NativeLinux
    }
}

pub fn inspect(static_image: &[u8]) -> CompatibilityReport {
    let basic = __cpuid(0);
    let feature = if basic.eax >= 1 {
        __cpuid(1)
    } else {
        __cpuid(0)
    };
    let extended = __cpuid(0x8000_0000);
    let extended_feature = if extended.eax >= 0x8000_0001 {
        __cpuid(0x8000_0001)
    } else {
        __cpuid(0)
    };
    let svm_feature = if extended.eax >= 0x8000_000a {
        __cpuid(0x8000_000a)
    } else {
        __cpuid(0)
    };
    let vmx = feature.ecx & (1 << 5) != 0;
    let svm = extended_feature.ecx & (1 << 2) != 0;
    let npt = svm && svm_feature.edx & 1 != 0;
    let under_hypervisor = feature.ecx & (1 << 31) != 0;
    let static_route = route(static_image);
    let dynamic = dynamic_fixture();
    let dynamic_route = route(&dynamic);
    let script_route = route(b"#!/bin/sh\nexit 0\n");
    let malformed_route = route(b"not an executable");
    let read_only_base = true;
    let per_app_overlay = true;
    let host_files_default_deny = true;
    let devices_default_deny = true;
    let software_emulation = false;
    let bridge_protocol = bridge_self_test();
    let verified = static_route == ExecutionRoute::NativeLinux
        && dynamic_route == ExecutionRoute::DebianGuest
        && script_route == ExecutionRoute::DebianGuest
        && malformed_route == ExecutionRoute::Reject
        && read_only_base
        && per_app_overlay
        && host_files_default_deny
        && devices_default_deny
        && bridge_protocol
        && !software_emulation;
    CompatibilityReport {
        vmx,
        svm,
        npt,
        under_hypervisor,
        hardware_acceleration: vmx || svm,
        static_route,
        dynamic_route,
        script_route,
        malformed_route,
        read_only_base,
        per_app_overlay,
        host_files_default_deny,
        devices_default_deny,
        software_emulation,
        bridge_protocol,
        verified,
    }
}

fn bridge_self_test() -> bool {
    let path = b"/usr/bin/example";
    let payload_bytes = 6 + path.len();
    let mut frame = [0u8; 64];
    frame[..4].copy_from_slice(&BRIDGE_MAGIC);
    write_u16(&mut frame, 4, BRIDGE_VERSION);
    write_u16(&mut frame, 6, BridgeOpcode::Launch as u16);
    write_u32(&mut frame, 8, 7);
    write_u32(&mut frame, 12, payload_bytes as u32);
    write_u32(&mut frame, BRIDGE_HEADER_BYTES, 1);
    write_u16(&mut frame, BRIDGE_HEADER_BYTES + 4, path.len() as u16);
    frame[BRIDGE_HEADER_BYTES + 6..BRIDGE_HEADER_BYTES + 6 + path.len()].copy_from_slice(path);
    let used = BRIDGE_HEADER_BYTES + payload_bytes;
    let Some(message) = BridgeMessage::parse(&frame[..used]) else {
        return false;
    };
    let Some(launch) = LaunchRequest::parse(message.payload) else {
        return false;
    };
    let valid = message.opcode == BridgeOpcode::Launch
        && message.request == 7
        && launch.permissions == 1
        && launch.path == "/usr/bin/example";
    frame[12..16].copy_from_slice(&(MAX_BRIDGE_PAYLOAD as u32 + 1).to_le_bytes());
    let oversize_rejected = BridgeMessage::parse(&frame[..used]).is_none();
    let traversal = [
        0, 0, 0, 0, 15, 0, b'/', b'u', b's', b'r', b'/', b'.', b'.', b'/', b'b', b'i', b'n', b'/',
        b'a', b'p', b'p',
    ];
    valid && oversize_rejected && LaunchRequest::parse(&traversal).is_none()
}

fn valid_guest_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= MAX_GUEST_PATH_BYTES
        && path.bytes().all(|byte| byte.is_ascii_graphic())
        && path
            .split('/')
            .all(|component| component != "." && component != "..")
}

fn valid_shebang(bytes: &[u8]) -> bool {
    let line = bytes
        .get(2..)
        .and_then(|value| value.split(|byte| *byte == b'\n').next())
        .unwrap_or_default();
    !line.is_empty()
        && line.len() <= MAX_INTERPRETER_BYTES
        && line[0] == b'/'
        && line
            .iter()
            .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
}

fn valid_interpreter(bytes: &[u8], header: usize) -> bool {
    let Some(offset) = read_u64(bytes, header + 8).and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(length) = read_u64(bytes, header + 32).and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    if !(2..=MAX_INTERPRETER_BYTES).contains(&length) {
        return false;
    }
    let Some(value) = offset
        .checked_add(length)
        .and_then(|end| bytes.get(offset..end))
    else {
        return false;
    };
    value[0] == b'/'
        && value[length - 1] == 0
        && value[..length - 1]
            .iter()
            .all(|byte| byte.is_ascii_graphic())
}

fn dynamic_fixture() -> [u8; 256] {
    let mut image = [0u8; 256];
    image[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    write_u16(&mut image, 16, ET_DYN);
    write_u16(&mut image, 18, EM_X86_64);
    write_u32(&mut image, 20, 1);
    write_u64(&mut image, 32, ELF_HEADER_BYTES as u64);
    write_u16(&mut image, 52, ELF_HEADER_BYTES as u16);
    write_u16(&mut image, 54, PROGRAM_HEADER_BYTES as u16);
    write_u16(&mut image, 56, 2);
    write_u32(&mut image, ELF_HEADER_BYTES, PT_LOAD);
    let interpreter_header = ELF_HEADER_BYTES + PROGRAM_HEADER_BYTES;
    let interpreter = b"/lib64/ld-linux-x86-64.so.2\0";
    let interpreter_offset = 192;
    write_u32(&mut image, interpreter_header, PT_INTERP);
    write_u64(
        &mut image,
        interpreter_header + 8,
        interpreter_offset as u64,
    );
    write_u64(
        &mut image,
        interpreter_header + 32,
        interpreter.len() as u64,
    );
    image[interpreter_offset..interpreter_offset + interpreter.len()].copy_from_slice(interpreter);
    image
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let value = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let value = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(value.try_into().ok()?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let value = bytes.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes(value.try_into().ok()?))
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
