//! Bluetooth host controller interface packets: commands, events and ACL
//! headers, plus the bring-up sequence a host runs against a new controller.
//! Core-only and transport-free; the USB class driver that would carry the
//! packets does not exist yet.

#![allow(dead_code)]

pub const COMMAND: u8 = 0x01;
pub const ACL: u8 = 0x02;
pub const EVENT: u8 = 0x04;

pub const RESET: u16 = opcode(0x03, 0x0003);
pub const READ_LOCAL_VERSION: u16 = opcode(0x04, 0x0001);
pub const READ_BD_ADDR: u16 = opcode(0x04, 0x0009);
pub const SET_EVENT_MASK: u16 = opcode(0x03, 0x0001);
pub const INQUIRY: u16 = opcode(0x01, 0x0001);
pub const LE_SET_SCAN_PARAMETERS: u16 = opcode(0x08, 0x000b);
pub const LE_SET_SCAN_ENABLE: u16 = opcode(0x08, 0x000c);

pub const EVENT_INQUIRY_RESULT: u8 = 0x02;
pub const EVENT_COMMAND_COMPLETE: u8 = 0x0e;
pub const EVENT_COMMAND_STATUS: u8 = 0x0f;
pub const EVENT_LE_META: u8 = 0x3e;
const LE_ADVERTISING_REPORT: u8 = 0x02;

pub const MAX_PACKET: usize = 260;

pub const fn opcode(group: u16, command: u16) -> u16 {
    group << 10 | command
}

/// A command packet: the indicator, the opcode, the parameter length and the
/// parameters.
pub struct Packet {
    pub bytes: [u8; MAX_PACKET],
    pub length: usize,
}

impl Packet {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

pub fn command(code: u16, parameters: &[u8]) -> Option<Packet> {
    if parameters.len() > 255 {
        return None;
    }
    let mut packet = Packet {
        bytes: [0; MAX_PACKET],
        length: 4 + parameters.len(),
    };
    packet.bytes[0] = COMMAND;
    packet.bytes[1..3].copy_from_slice(&code.to_le_bytes());
    packet.bytes[3] = parameters.len() as u8;
    packet.bytes[4..4 + parameters.len()].copy_from_slice(parameters);
    Some(packet)
}

pub fn reset() -> Packet {
    command(RESET, &[]).unwrap()
}

pub fn read_local_version() -> Packet {
    command(READ_LOCAL_VERSION, &[]).unwrap()
}

pub fn read_bd_addr() -> Packet {
    command(READ_BD_ADDR, &[]).unwrap()
}

/// Everything the host wants to hear about, but the events that need no
/// handling.
pub fn set_event_mask(mask: u64) -> Packet {
    command(SET_EVENT_MASK, &mask.to_le_bytes()).unwrap()
}

/// General inquiry for `length` units of 1.28 seconds.
pub fn inquiry(length: u8, responses: u8) -> Packet {
    command(INQUIRY, &[0x33, 0x8b, 0x9e, length, responses]).unwrap()
}

/// Passive LE scan with the given interval and window (units of 0.625 ms).
pub fn le_set_scan_parameters(interval: u16, window: u16) -> Packet {
    let mut parameters = [0u8; 7];
    parameters[0] = 0; // passive
    parameters[1..3].copy_from_slice(&interval.to_le_bytes());
    parameters[3..5].copy_from_slice(&window.to_le_bytes());
    command(LE_SET_SCAN_PARAMETERS, &parameters).unwrap()
}

pub fn le_set_scan_enable(enable: bool) -> Packet {
    command(LE_SET_SCAN_ENABLE, &[enable as u8, 1]).unwrap()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event<'a> {
    CommandComplete {
        opcode: u16,
        status: u8,
        parameters: &'a [u8],
    },
    CommandStatus {
        opcode: u16,
        status: u8,
    },
    LeAdvertisement {
        address: [u8; 6],
        address_type: u8,
        kind: u8,
        rssi: i8,
        data: &'a [u8],
    },
    InquiryResult {
        address: [u8; 6],
    },
    Other(u8),
}

/// Reads one event packet (indicator included).
pub fn parse_event(packet: &[u8]) -> Option<Event<'_>> {
    if packet.len() < 3 || packet[0] != EVENT {
        return None;
    }
    let length = packet[2] as usize;
    let body = packet.get(3..3 + length)?;
    Some(match packet[1] {
        EVENT_COMMAND_COMPLETE if body.len() >= 4 => Event::CommandComplete {
            opcode: u16::from_le_bytes([body[1], body[2]]),
            status: body[3],
            parameters: &body[4..],
        },
        EVENT_COMMAND_STATUS if body.len() >= 4 => Event::CommandStatus {
            status: body[0],
            opcode: u16::from_le_bytes([body[2], body[3]]),
        },
        EVENT_INQUIRY_RESULT if body.len() >= 7 => Event::InquiryResult {
            address: body[1..7].try_into().ok()?,
        },
        EVENT_LE_META if body.first() == Some(&LE_ADVERTISING_REPORT) && body.len() >= 12 => {
            // One report per event is all controllers send when scanning.
            let data_length = body[10] as usize;
            let data = body.get(11..11 + data_length)?;
            Event::LeAdvertisement {
                kind: body[2],
                address_type: body[3],
                address: body[4..10].try_into().ok()?,
                data,
                rssi: *body.get(11 + data_length)? as i8,
            }
        }
        other => Event::Other(other),
    })
}

/// The header of an ACL data packet.
pub fn parse_acl_header(packet: &[u8]) -> Option<(u16, u8, usize)> {
    if packet.len() < 5 || packet[0] != ACL {
        return None;
    }
    let word = u16::from_le_bytes([packet[1], packet[2]]);
    let length = u16::from_le_bytes([packet[3], packet[4]]) as usize;
    (packet.len() >= 5 + length).then_some((word & 0x0fff, (word >> 12) as u8, length))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Reset,
    Version,
    Address,
    Ready,
    Failed(u8),
}

/// What the host learned about the controller while bringing it up.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Controller {
    pub hci_version: u8,
    pub manufacturer: u16,
    pub address: [u8; 6],
}

/// Walks a controller through reset, version and address queries, one command
/// at a time.
pub struct Bringup {
    pub stage: Stage,
    pub controller: Controller,
}

impl Bringup {
    pub fn new() -> Self {
        Self {
            stage: Stage::Reset,
            controller: Controller::default(),
        }
    }

    /// The command to send now, until the controller is ready.
    pub fn command(&self) -> Option<Packet> {
        match self.stage {
            Stage::Reset => Some(reset()),
            Stage::Version => Some(read_local_version()),
            Stage::Address => Some(read_bd_addr()),
            Stage::Ready | Stage::Failed(_) => None,
        }
    }

    /// Feeds an event from the controller; unrelated events are ignored.
    pub fn event(&mut self, packet: &[u8]) {
        let Some(Event::CommandComplete {
            opcode,
            status,
            parameters,
        }) = parse_event(packet)
        else {
            return;
        };
        let expected = match self.stage {
            Stage::Reset => RESET,
            Stage::Version => READ_LOCAL_VERSION,
            Stage::Address => READ_BD_ADDR,
            _ => return,
        };
        if opcode != expected {
            return;
        }
        if status != 0 {
            self.stage = Stage::Failed(status);
            return;
        }
        match self.stage {
            Stage::Reset => self.stage = Stage::Version,
            Stage::Version if parameters.len() >= 8 => {
                self.controller.hci_version = parameters[0];
                self.controller.manufacturer = u16::from_le_bytes([parameters[4], parameters[5]]);
                self.stage = Stage::Address;
            }
            Stage::Address if parameters.len() >= 6 => {
                self.controller.address.copy_from_slice(&parameters[..6]);
                self.stage = Stage::Ready;
            }
            _ => self.stage = Stage::Failed(0xff),
        }
    }
}

impl Default for Bringup {
    fn default() -> Self {
        Self::new()
    }
}

/// A scripted controller for the boot-test: reset, version, address.
pub fn self_test() -> bool {
    fn complete(opcode: u16, parameters: &[u8]) -> ([u8; 16], usize) {
        let mut packet = [0u8; 16];
        packet[..4].copy_from_slice(&[
            EVENT,
            EVENT_COMMAND_COMPLETE,
            4 + parameters.len() as u8,
            1,
        ]);
        packet[4..6].copy_from_slice(&opcode.to_le_bytes());
        packet[6] = 0;
        packet[7..7 + parameters.len()].copy_from_slice(parameters);
        (packet, 7 + parameters.len())
    }
    let mut host = Bringup::new();
    let (packet, length) = complete(RESET, &[]);
    host.event(&packet[..length]);
    let (packet, length) = complete(READ_LOCAL_VERSION, &[9, 0, 0, 9, 0x0f, 0, 0, 0]);
    host.event(&packet[..length]);
    let (packet, length) = complete(READ_BD_ADDR, &[1, 2, 3, 4, 5, 6]);
    host.event(&packet[..length]);
    host.stage == Stage::Ready
        && host.controller.address == [1, 2, 3, 4, 5, 6]
        && reset().as_slice() == [0x01, 0x03, 0x0c, 0x00]
}
