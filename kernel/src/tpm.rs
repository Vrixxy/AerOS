//! TPM 2.0 over the TIS/FIFO memory-mapped interface: command marshalling
//! (`Startup`, `GetRandom`, `PCR_Read`, `PCR_Extend`) and the register
//! protocol that carries a command to the chip and fetches its response.
//! Generic over the register access, so the host harness drives it against a
//! model of the chip. Nothing in this emulator has a TPM, so the driver has
//! only ever run against that model.

#![allow(dead_code)]

pub const TIS_BASE: u64 = 0xfed4_0000;

const ACCESS: u64 = 0x00;
const STS: u64 = 0x18;
const DATA_FIFO: u64 = 0x24;
const DID_VID: u64 = 0xf00;

const ACCESS_VALID: u8 = 0x80;
const ACCESS_ACTIVE: u8 = 0x20;
const ACCESS_REQUEST_USE: u8 = 0x02;
const STS_VALID: u32 = 0x80;
const STS_READY: u32 = 0x40;
const STS_GO: u32 = 0x20;
const STS_DATA_AVAILABLE: u32 = 0x10;
const STS_EXPECT: u32 = 0x08;

const TAG_NO_SESSIONS: u16 = 0x8001;
const TAG_SESSIONS: u16 = 0x8002;
const ALG_SHA256: u16 = 0x000b;
const RS_PASSWORD: u32 = 0x4000_0009;

pub const CC_STARTUP: u32 = 0x0000_0144;
pub const CC_GET_RANDOM: u32 = 0x0000_017b;
pub const CC_PCR_READ: u32 = 0x0000_017e;
pub const CC_PCR_EXTEND: u32 = 0x0000_0182;

pub const MAX_COMMAND: usize = 128;
pub const MAX_RESPONSE: usize = 256;

pub trait Bus {
    fn read8(&mut self, offset: u64) -> u8;
    fn read32(&mut self, offset: u64) -> u32;
    fn write8(&mut self, offset: u64, value: u8);
    fn write32(&mut self, offset: u64, value: u32);
    /// A monotonic clock, to bound every wait.
    fn now_ns(&mut self) -> u64;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    NoDevice,
    Timeout,
    Protocol,
    /// The chip answered with this non-zero response code.
    Code(u32),
}

/// A marshalled command.
pub struct Command {
    pub bytes: [u8; MAX_COMMAND],
    pub length: usize,
}

impl Command {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    fn new(tag: u16, code: u32) -> Self {
        let mut command = Command {
            bytes: [0; MAX_COMMAND],
            length: 10,
        };
        command.bytes[0..2].copy_from_slice(&tag.to_be_bytes());
        command.bytes[6..10].copy_from_slice(&code.to_be_bytes());
        command
    }

    fn push(&mut self, data: &[u8]) {
        self.bytes[self.length..self.length + data.len()].copy_from_slice(data);
        self.length += data.len();
    }

    fn finish(mut self) -> Self {
        let length = self.length as u32;
        self.bytes[2..6].copy_from_slice(&length.to_be_bytes());
        self
    }
}

/// `TPM2_Startup(TPM_SU_CLEAR)`.
pub fn startup() -> Command {
    let mut command = Command::new(TAG_NO_SESSIONS, CC_STARTUP);
    command.push(&0u16.to_be_bytes());
    command.finish()
}

pub fn get_random(count: u16) -> Command {
    let mut command = Command::new(TAG_NO_SESSIONS, CC_GET_RANDOM);
    command.push(&count.to_be_bytes());
    command.finish()
}

/// Reads one SHA-256 PCR (0 to 23).
pub fn pcr_read(index: u8) -> Command {
    let mut command = Command::new(TAG_NO_SESSIONS, CC_PCR_READ);
    command.push(&1u32.to_be_bytes());
    command.push(&ALG_SHA256.to_be_bytes());
    command.push(&[3, 0, 0, 0]);
    command.bytes[command.length - 3 + (index / 8) as usize] |= 1 << (index % 8);
    command.finish()
}

/// Extends one PCR with a SHA-256 digest, authorised by the empty password.
pub fn pcr_extend(index: u8, digest: &[u8; 32]) -> Command {
    let mut command = Command::new(TAG_SESSIONS, CC_PCR_EXTEND);
    command.push(&(index as u32).to_be_bytes());
    command.push(&9u32.to_be_bytes());
    command.push(&RS_PASSWORD.to_be_bytes());
    command.push(&[0, 0, 0, 0, 0]);
    command.push(&1u32.to_be_bytes());
    command.push(&ALG_SHA256.to_be_bytes());
    command.push(digest);
    command.finish()
}

/// The response code of a response, after checking its framing.
pub fn response_code(response: &[u8]) -> Result<u32, Error> {
    if response.len() < 10 {
        return Err(Error::Protocol);
    }
    let size = u32::from_be_bytes([response[2], response[3], response[4], response[5]]) as usize;
    if size != response.len() {
        return Err(Error::Protocol);
    }
    match u32::from_be_bytes([response[6], response[7], response[8], response[9]]) {
        0 => Ok(0),
        code => Err(Error::Code(code)),
    }
}

/// The random bytes in a `GetRandom` response.
pub fn random_bytes(response: &[u8]) -> Result<&[u8], Error> {
    response_code(response)?;
    let count = u16::from_be_bytes([
        *response.get(10).ok_or(Error::Protocol)?,
        *response.get(11).ok_or(Error::Protocol)?,
    ]) as usize;
    response.get(12..12 + count).ok_or(Error::Protocol)
}

/// The SHA-256 value in a `PCR_Read` response for a single selected PCR.
pub fn pcr_value(response: &[u8]) -> Result<[u8; 32], Error> {
    response_code(response)?;
    // Update counter (4), selection out (count 4 + alg 2 + size 1 + 3 bitmap),
    // digest count (4), digest size (2), digest.
    let at = 10 + 4 + 4 + 2 + 1 + 3;
    let count = u32::from_be_bytes(
        response
            .get(at..at + 4)
            .ok_or(Error::Protocol)?
            .try_into()
            .unwrap(),
    );
    let size = u16::from_be_bytes(
        response
            .get(at + 4..at + 6)
            .ok_or(Error::Protocol)?
            .try_into()
            .unwrap(),
    );
    if count != 1 || size != 32 {
        return Err(Error::Protocol);
    }
    response
        .get(at + 6..at + 38)
        .ok_or(Error::Protocol)?
        .try_into()
        .map_err(|_| Error::Protocol)
}

const WAIT_NS: u64 = 750_000_000;

pub struct Tis<B: Bus> {
    bus: B,
}

impl<B: Bus> Tis<B> {
    /// Checks there is a chip behind the registers and takes locality 0.
    pub fn open(mut bus: B) -> Result<(Self, u32), Error> {
        let access = bus.read8(ACCESS);
        let id = bus.read32(DID_VID);
        if access == 0xff || id == 0xffff_ffff || id == 0 || access & ACCESS_VALID == 0 {
            return Err(Error::NoDevice);
        }
        let mut tis = Tis { bus };
        tis.bus.write8(ACCESS, ACCESS_REQUEST_USE);
        tis.wait(|bus| bus.read8(ACCESS) & ACCESS_ACTIVE != 0)?;
        Ok((tis, id))
    }

    fn wait(&mut self, mut condition: impl FnMut(&mut B) -> bool) -> Result<(), Error> {
        let start = self.bus.now_ns();
        loop {
            if condition(&mut self.bus) {
                return Ok(());
            }
            if self.bus.now_ns().saturating_sub(start) > WAIT_NS {
                return Err(Error::Timeout);
            }
        }
    }

    fn burst(&mut self) -> Result<usize, Error> {
        let start = self.bus.now_ns();
        loop {
            let burst = (self.bus.read32(STS) >> 8) & 0xffff;
            if burst != 0 {
                return Ok(burst as usize);
            }
            if self.bus.now_ns().saturating_sub(start) > WAIT_NS {
                return Err(Error::Timeout);
            }
        }
    }

    /// Sends one command and returns the length of the response written to
    /// `response`.
    pub fn transmit(
        &mut self,
        command: &[u8],
        response: &mut [u8; MAX_RESPONSE],
    ) -> Result<usize, Error> {
        self.wait(|bus| bus.read32(STS) & STS_READY != 0)
            .or_else(|_| {
                self.bus.write32(STS, STS_READY);
                self.wait(|bus| bus.read32(STS) & STS_READY != 0)
            })?;
        let mut sent = 0;
        while sent < command.len() {
            let burst = self.burst()?.min(command.len() - sent);
            for byte in &command[sent..sent + burst] {
                self.bus.write8(DATA_FIFO, *byte);
            }
            sent += burst;
            let status = self.bus.read32(STS);
            let more = status & STS_EXPECT != 0;
            if more != (sent < command.len()) {
                return Err(Error::Protocol);
            }
        }
        self.bus.write32(STS, STS_GO);
        self.wait(|bus| {
            bus.read32(STS) & (STS_DATA_AVAILABLE | STS_VALID) == (STS_DATA_AVAILABLE | STS_VALID)
        })?;
        let mut received = 0;
        while self.bus.read32(STS) & STS_DATA_AVAILABLE != 0 {
            if received == response.len() {
                return Err(Error::Protocol);
            }
            let burst = self.burst().unwrap_or(1).min(response.len() - received);
            for slot in &mut response[received..received + burst] {
                if self.bus.read32(STS) & STS_DATA_AVAILABLE == 0 {
                    break;
                }
                *slot = self.bus.read8(DATA_FIFO);
                received += 1;
            }
        }
        self.bus.write32(STS, STS_READY);
        Ok(received)
    }

    pub fn run(
        &mut self,
        command: &Command,
        response: &mut [u8; MAX_RESPONSE],
    ) -> Result<usize, Error> {
        let length = self.transmit(command.as_slice(), response)?;
        response_code(&response[..length])?;
        Ok(length)
    }
}
