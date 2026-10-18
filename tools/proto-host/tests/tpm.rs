use std::collections::VecDeque;

use proto_host::tpm::*;

#[test]
fn commands_have_the_spec_bytes() {
    assert_eq!(
        startup().as_slice(),
        [0x80, 0x01, 0, 0, 0, 0x0c, 0, 0, 0x01, 0x44, 0, 0]
    );
    assert_eq!(
        get_random(32).as_slice(),
        [0x80, 0x01, 0, 0, 0, 0x0c, 0, 0, 0x01, 0x7b, 0, 32]
    );
    assert_eq!(
        pcr_read(10).as_slice(),
        [
            0x80, 0x01, 0, 0, 0, 0x14, 0, 0, 0x01, 0x7e, 0, 0, 0, 1, 0, 0x0b, 3, 0, 0x04, 0
        ]
    );
    let extend = pcr_extend(7, &[0xab; 32]);
    let bytes = extend.as_slice();
    assert_eq!(bytes.len(), 10 + 4 + 4 + 9 + 4 + 2 + 32);
    assert_eq!(&bytes[..2], [0x80, 0x02]);
    assert_eq!(u32::from_be_bytes(bytes[2..6].try_into().unwrap()) as usize, bytes.len());
    assert_eq!(&bytes[6..10], [0, 0, 0x01, 0x82]);
    assert_eq!(&bytes[10..14], [0, 0, 0, 7]);
    assert_eq!(&bytes[14..18], [0, 0, 0, 9]);
    assert_eq!(&bytes[18..22], [0x40, 0, 0, 9]);
    assert_eq!(&bytes[27..31], [0, 0, 0, 1]);
    assert_eq!(&bytes[31..33], [0, 0x0b]);
    assert_eq!(&bytes[33..], [0xab; 32]);
}

/// A model of a TIS chip: a four-byte burst count, the ready/go/data-available
/// handshake, and a few commands.
struct Chip {
    access_active: bool,
    ready: bool,
    received: Vec<u8>,
    response: VecDeque<u8>,
    clock: u64,
    present: bool,
    dead: bool,
    commands: Vec<u32>,
}

impl Chip {
    fn new() -> Self {
        Chip {
            access_active: false,
            ready: false,
            received: Vec::new(),
            response: VecDeque::new(),
            clock: 0,
            present: true,
            dead: false,
            commands: Vec::new(),
        }
    }

    fn expected(&self) -> usize {
        if self.received.len() < 6 {
            return 6;
        }
        u32::from_be_bytes(self.received[2..6].try_into().unwrap()) as usize
    }

    fn execute(&mut self) {
        let code = u32::from_be_bytes(self.received[6..10].try_into().unwrap());
        self.commands.push(code);
        let mut body = Vec::new();
        let mut result = 0u32;
        match code {
            0x144 => {}
            0x17b => {
                let count = u16::from_be_bytes([self.received[10], self.received[11]]);
                body.extend_from_slice(&count.to_be_bytes());
                body.extend((0..count).map(|index| index as u8 ^ 0x5a));
            }
            0x17e => {
                body.extend_from_slice(&5u32.to_be_bytes());
                body.extend_from_slice(&[0, 0, 0, 1, 0, 0x0b, 3, 0, 0x04, 0]);
                body.extend_from_slice(&1u32.to_be_bytes());
                body.extend_from_slice(&32u16.to_be_bytes());
                body.extend((0..32).map(|index| 0xc0 + index as u8));
            }
            0x182 => {}
            _ => result = 0x143,
        }
        let tag: u16 = if code == 0x182 { 0x8002 } else { 0x8001 };
        let mut out = Vec::new();
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&((10 + body.len()) as u32).to_be_bytes());
        out.extend_from_slice(&result.to_be_bytes());
        out.extend(body);
        self.response = out.into();
    }
}

impl Bus for Chip {
    fn read8(&mut self, offset: u64) -> u8 {
        match offset {
            0x00 if !self.present => 0xff,
            0x00 => 0x80 | if self.access_active { 0x20 } else { 0 },
            0x24 => self.response.pop_front().unwrap_or(0xff),
            _ => 0,
        }
    }

    fn read32(&mut self, offset: u64) -> u32 {
        match offset {
            0xf00 if !self.present => 0xffff_ffff,
            0xf00 => 0x0001_104a,
            0x18 if self.dead => 0,
            0x18 => {
                let mut status = 0x80;
                if self.ready {
                    status |= 0x40;
                }
                if !self.response.is_empty() {
                    status |= 0x10;
                }
                if self.ready && !self.received.is_empty() && self.received.len() < self.expected() {
                    status |= 0x08;
                }
                status | (4 << 8)
            }
            _ => 0,
        }
    }

    fn write8(&mut self, offset: u64, value: u8) {
        match offset {
            0x00 if value & 0x02 != 0 => self.access_active = true,
            0x24 => self.received.push(value),
            _ => {}
        }
    }

    fn write32(&mut self, offset: u64, value: u32) {
        if offset == 0x18 {
            if value & 0x40 != 0 {
                self.ready = true;
                self.received.clear();
                self.response.clear();
            }
            if value & 0x20 != 0 && self.received.len() == self.expected() {
                self.execute();
            }
        }
    }

    fn now_ns(&mut self) -> u64 {
        self.clock += 1_000;
        self.clock
    }
}

#[test]
fn commands_run_through_the_fifo() {
    let (mut tis, id) = Tis::open(Chip::new()).unwrap();
    assert_eq!(id, 0x0001_104a);
    let mut response = [0u8; MAX_RESPONSE];

    let length = tis.run(&startup(), &mut response).unwrap();
    assert_eq!(length, 10);

    let length = tis.run(&get_random(8), &mut response).unwrap();
    assert_eq!(random_bytes(&response[..length]).unwrap(), [0x5a, 0x5b, 0x58, 0x59, 0x5e, 0x5f, 0x5c, 0x5d]);

    let length = tis.run(&pcr_read(3), &mut response).unwrap();
    let value = pcr_value(&response[..length]).unwrap();
    assert_eq!(value[0], 0xc0);
    assert_eq!(value[31], 0xdf);

    // A command that needs several bursts (the extend is 69 bytes).
    let length = tis.run(&pcr_extend(7, &[1; 32]), &mut response).unwrap();
    assert_eq!(length, 10);
}

#[test]
fn a_rejected_command_reports_the_code() {
    let (mut tis, _) = Tis::open(Chip::new()).unwrap();
    let mut response = [0u8; MAX_RESPONSE];
    let mut bad = startup();
    bad.bytes[9] = 0x99;
    assert_eq!(tis.run(&bad, &mut response).err(), Some(Error::Code(0x143)));
}

#[test]
fn no_chip_and_a_dead_chip() {
    let mut absent = Chip::new();
    absent.present = false;
    assert_eq!(Tis::open(absent).err(), Some(Error::NoDevice));

    let mut dead = Chip::new();
    dead.dead = true;
    let (mut tis, _) = Tis::open(dead).unwrap();
    let mut response = [0u8; MAX_RESPONSE];
    assert_eq!(tis.run(&startup(), &mut response).err(), Some(Error::Timeout));
}

#[test]
fn malformed_responses_are_refused() {
    assert_eq!(response_code(&[0x80, 1, 0, 0, 0, 10, 0, 0, 0, 0]), Ok(0));
    assert_eq!(response_code(&[0x80, 1, 0, 0, 0, 11, 0, 0, 0, 0]), Err(Error::Protocol));
    assert_eq!(response_code(&[0x80, 1, 0]), Err(Error::Protocol));
    assert_eq!(
        response_code(&[0x80, 1, 0, 0, 0, 10, 0, 0, 1, 0x01]),
        Err(Error::Code(0x101))
    );
    assert!(random_bytes(&[0x80, 1, 0, 0, 0, 12, 0, 0, 0, 0, 0, 8]).is_err());
}
