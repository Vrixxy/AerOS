//! Socket options kept per socket: what `setsockopt` stores and `getsockopt`
//! reads back, with Linux's option numbers, value layouts and errors. Reuse
//! of addresses, receive and send time-outs, and abortive close by linger are
//! acted on by the socket layer; the rest is remembered and reported.

pub const SOL_SOCKET: u64 = 1;
pub const IPPROTO_IP: u64 = 0;
pub const IPPROTO_TCP: u64 = 6;
pub const IPPROTO_IPV6: u64 = 41;

const SO_DEBUG: u64 = 1;
const SO_REUSEADDR: u64 = 2;
const SO_DONTROUTE: u64 = 5;
const SO_BROADCAST: u64 = 6;
const SO_SNDBUF: u64 = 7;
const SO_RCVBUF: u64 = 8;
const SO_KEEPALIVE: u64 = 9;
const SO_OOBINLINE: u64 = 10;
const SO_LINGER: u64 = 13;
const SO_REUSEPORT: u64 = 15;
const SO_RCVTIMEO: u64 = 20;
const SO_SNDTIMEO: u64 = 21;
const IP_TOS: u64 = 1;
const IP_TTL: u64 = 2;
const IPV6_UNICAST_HOPS: u64 = 16;
const IPV6_V6ONLY: u64 = 26;
const TCP_NODELAY: u64 = 1;
const TCP_CORK: u64 = 3;
const TCP_KEEPIDLE: u64 = 4;
const TCP_KEEPINTVL: u64 = 5;
const TCP_KEEPCNT: u64 = 6;
const TCP_QUICKACK: u64 = 12;

const EINVAL: u64 = 22;
const EDOM: u64 = 33;
const ENOPROTOOPT: u64 = 92;
const MIN_SEND_BUFFER: u32 = 4608;
const MIN_RECEIVE_BUFFER: u32 = 2304;
const MAX_BUFFER: u32 = 4_194_304;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Options {
    pub reuse_address: bool,
    pub reuse_port: bool,
    pub keepalive: bool,
    pub broadcast: bool,
    pub debug: bool,
    pub dont_route: bool,
    pub oob_inline: bool,
    pub nodelay: bool,
    pub cork: bool,
    pub quickack: bool,
    pub v6_only: bool,
    pub linger_on: bool,
    pub linger_seconds: i32,
    pub receive_timeout_ns: u64,
    pub send_timeout_ns: u64,
    pub send_buffer: u32,
    pub receive_buffer: u32,
    pub ttl: u32,
    pub tos: u32,
    pub keep_idle: u32,
    pub keep_interval: u32,
    pub keep_count: u32,
}

impl Options {
    pub const DEFAULT: Options = Options {
        reuse_address: false,
        reuse_port: false,
        keepalive: false,
        broadcast: false,
        debug: false,
        dont_route: false,
        oob_inline: false,
        nodelay: false,
        cork: false,
        quickack: false,
        v6_only: false,
        linger_on: false,
        linger_seconds: 0,
        receive_timeout_ns: 0,
        send_timeout_ns: 0,
        send_buffer: 16_384,
        receive_buffer: 16_384,
        ttl: 64,
        tos: 0,
        keep_idle: 7200,
        keep_interval: 75,
        keep_count: 9,
    };

    /// Applies `setsockopt(level, option, data)`.
    pub fn set(&mut self, level: u64, option: u64, data: &[u8]) -> Result<(), u64> {
        let integer = || -> Result<u32, u64> {
            data.get(..4)
                .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                .ok_or(EINVAL)
        };
        match (level, option) {
            (SOL_SOCKET, SO_DEBUG) => self.debug = integer()? != 0,
            (SOL_SOCKET, SO_REUSEADDR) => self.reuse_address = integer()? != 0,
            (SOL_SOCKET, SO_REUSEPORT) => self.reuse_port = integer()? != 0,
            (SOL_SOCKET, SO_DONTROUTE) => self.dont_route = integer()? != 0,
            (SOL_SOCKET, SO_BROADCAST) => self.broadcast = integer()? != 0,
            (SOL_SOCKET, SO_KEEPALIVE) => self.keepalive = integer()? != 0,
            (SOL_SOCKET, SO_OOBINLINE) => self.oob_inline = integer()? != 0,
            (SOL_SOCKET, SO_SNDBUF) => {
                let requested = integer()?.min(MAX_BUFFER / 2);
                self.send_buffer = requested.saturating_mul(2).max(MIN_SEND_BUFFER);
            }
            (SOL_SOCKET, SO_RCVBUF) => {
                let requested = integer()?.min(MAX_BUFFER / 2);
                self.receive_buffer = requested.saturating_mul(2).max(MIN_RECEIVE_BUFFER);
            }
            (SOL_SOCKET, SO_LINGER) => {
                if data.len() < 8 {
                    return Err(EINVAL);
                }
                self.linger_on = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) != 0;
                self.linger_seconds = i32::from_le_bytes([data[4], data[5], data[6], data[7]]);
                if self.linger_seconds < 0 {
                    self.linger_seconds = 0;
                }
            }
            (SOL_SOCKET, SO_RCVTIMEO | SO_SNDTIMEO) => {
                if data.len() < 16 {
                    return Err(EINVAL);
                }
                let seconds = i64::from_le_bytes(data[..8].try_into().map_err(|_| EINVAL)?);
                let micros = i64::from_le_bytes(data[8..16].try_into().map_err(|_| EINVAL)?);
                if seconds < 0 || !(0..1_000_000).contains(&micros) {
                    return Err(EDOM);
                }
                let nanoseconds = (seconds as u64)
                    .saturating_mul(1_000_000_000)
                    .saturating_add(micros as u64 * 1000);
                if option == SO_RCVTIMEO {
                    self.receive_timeout_ns = nanoseconds;
                } else {
                    self.send_timeout_ns = nanoseconds;
                }
            }
            (IPPROTO_IP, IP_TTL) | (IPPROTO_IPV6, IPV6_UNICAST_HOPS) => {
                let value = integer()?;
                if value == u32::MAX && level == IPPROTO_IPV6 {
                    self.ttl = 64;
                } else if (1..=255).contains(&value) {
                    self.ttl = value;
                } else {
                    return Err(EINVAL);
                }
            }
            (IPPROTO_IP, IP_TOS) => self.tos = integer()? & 0xff,
            (IPPROTO_IPV6, IPV6_V6ONLY) => self.v6_only = integer()? != 0,
            (IPPROTO_TCP, TCP_NODELAY) => self.nodelay = integer()? != 0,
            (IPPROTO_TCP, TCP_CORK) => self.cork = integer()? != 0,
            (IPPROTO_TCP, TCP_QUICKACK) => self.quickack = integer()? != 0,
            (IPPROTO_TCP, TCP_KEEPIDLE) => self.keep_idle = positive(integer()?)?,
            (IPPROTO_TCP, TCP_KEEPINTVL) => self.keep_interval = positive(integer()?)?,
            (IPPROTO_TCP, TCP_KEEPCNT) => self.keep_count = positive(integer()?)?,
            _ => return Err(ENOPROTOOPT),
        }
        Ok(())
    }

    /// The value `getsockopt(level, option)` returns for an option kept here:
    /// its bytes and their count.
    pub fn get(&self, level: u64, option: u64) -> Option<([u8; 16], usize)> {
        let mut bytes = [0u8; 16];
        let integer = |value: u32, bytes: &mut [u8; 16]| {
            bytes[..4].copy_from_slice(&value.to_le_bytes());
            4
        };
        let size = match (level, option) {
            (SOL_SOCKET, SO_DEBUG) => integer(u32::from(self.debug), &mut bytes),
            (SOL_SOCKET, SO_REUSEADDR) => integer(u32::from(self.reuse_address), &mut bytes),
            (SOL_SOCKET, SO_REUSEPORT) => integer(u32::from(self.reuse_port), &mut bytes),
            (SOL_SOCKET, SO_DONTROUTE) => integer(u32::from(self.dont_route), &mut bytes),
            (SOL_SOCKET, SO_BROADCAST) => integer(u32::from(self.broadcast), &mut bytes),
            (SOL_SOCKET, SO_KEEPALIVE) => integer(u32::from(self.keepalive), &mut bytes),
            (SOL_SOCKET, SO_OOBINLINE) => integer(u32::from(self.oob_inline), &mut bytes),
            (SOL_SOCKET, SO_SNDBUF) => integer(self.send_buffer, &mut bytes),
            (SOL_SOCKET, SO_RCVBUF) => integer(self.receive_buffer, &mut bytes),
            (SOL_SOCKET, SO_LINGER) => {
                bytes[..4].copy_from_slice(&u32::from(self.linger_on).to_le_bytes());
                bytes[4..8].copy_from_slice(&self.linger_seconds.to_le_bytes());
                8
            }
            (SOL_SOCKET, SO_RCVTIMEO | SO_SNDTIMEO) => {
                let nanoseconds = if option == SO_RCVTIMEO {
                    self.receive_timeout_ns
                } else {
                    self.send_timeout_ns
                };
                bytes[..8].copy_from_slice(&((nanoseconds / 1_000_000_000) as i64).to_le_bytes());
                bytes[8..16].copy_from_slice(
                    &(((nanoseconds % 1_000_000_000) / 1000) as i64).to_le_bytes(),
                );
                16
            }
            (IPPROTO_IP, IP_TTL) | (IPPROTO_IPV6, IPV6_UNICAST_HOPS) => {
                integer(self.ttl, &mut bytes)
            }
            (IPPROTO_IP, IP_TOS) => integer(self.tos, &mut bytes),
            (IPPROTO_IPV6, IPV6_V6ONLY) => integer(u32::from(self.v6_only), &mut bytes),
            (IPPROTO_TCP, TCP_NODELAY) => integer(u32::from(self.nodelay), &mut bytes),
            (IPPROTO_TCP, TCP_CORK) => integer(u32::from(self.cork), &mut bytes),
            (IPPROTO_TCP, TCP_QUICKACK) => integer(u32::from(self.quickack), &mut bytes),
            (IPPROTO_TCP, TCP_KEEPIDLE) => integer(self.keep_idle, &mut bytes),
            (IPPROTO_TCP, TCP_KEEPINTVL) => integer(self.keep_interval, &mut bytes),
            (IPPROTO_TCP, TCP_KEEPCNT) => integer(self.keep_count, &mut bytes),
            _ => return None,
        };
        Some((bytes, size))
    }
}

fn positive(value: u32) -> Result<u32, u64> {
    if (1..=32_767).contains(&value) {
        Ok(value)
    } else {
        Err(EINVAL)
    }
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> bool {
    let mut options = Options::DEFAULT;
    let int = |value: u32| value.to_le_bytes();
    let mut linger = [0u8; 8];
    linger[..4].copy_from_slice(&1u32.to_le_bytes());
    linger[4..].copy_from_slice(&7i32.to_le_bytes());
    let mut timeout = [0u8; 16];
    timeout[..8].copy_from_slice(&2i64.to_le_bytes());
    timeout[8..].copy_from_slice(&500_000i64.to_le_bytes());
    let mut bad_timeout = timeout;
    bad_timeout[8..].copy_from_slice(&1_000_000i64.to_le_bytes());
    let stored = options.set(SOL_SOCKET, SO_REUSEADDR, &int(5)).is_ok()
        && options.set(SOL_SOCKET, SO_KEEPALIVE, &int(1)).is_ok()
        && options.set(SOL_SOCKET, SO_LINGER, &linger).is_ok()
        && options.set(SOL_SOCKET, SO_RCVTIMEO, &timeout).is_ok()
        && options.set(SOL_SOCKET, SO_SNDBUF, &int(10_000)).is_ok()
        && options.set(SOL_SOCKET, SO_RCVBUF, &int(100)).is_ok()
        && options.set(IPPROTO_TCP, TCP_NODELAY, &int(1)).is_ok()
        && options.set(IPPROTO_TCP, TCP_KEEPIDLE, &int(30)).is_ok()
        && options.set(IPPROTO_IP, IP_TTL, &int(33)).is_ok();
    let read_int = |level, option| {
        options
            .get(level, option)
            .filter(|(_, size)| *size == 4)
            .map(|(bytes, _)| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    };
    let linger_back = options
        .get(SOL_SOCKET, SO_LINGER)
        .is_some_and(|(bytes, size)| size == 8 && bytes[..8] == linger);
    let timeout_back = options
        .get(SOL_SOCKET, SO_RCVTIMEO)
        .is_some_and(|(bytes, size)| size == 16 && bytes == timeout)
        && options.receive_timeout_ns == 2_500_000_000
        && options.send_timeout_ns == 0;
    let values = read_int(SOL_SOCKET, SO_REUSEADDR) == Some(1)
        && read_int(SOL_SOCKET, SO_KEEPALIVE) == Some(1)
        && read_int(SOL_SOCKET, SO_BROADCAST) == Some(0)
        && read_int(SOL_SOCKET, SO_SNDBUF) == Some(20_000)
        && read_int(SOL_SOCKET, SO_RCVBUF) == Some(MIN_RECEIVE_BUFFER)
        && read_int(IPPROTO_TCP, TCP_NODELAY) == Some(1)
        && read_int(IPPROTO_TCP, TCP_KEEPIDLE) == Some(30)
        && read_int(IPPROTO_TCP, TCP_KEEPCNT) == Some(9)
        && read_int(IPPROTO_IP, IP_TTL) == Some(33);
    let errors = options.set(SOL_SOCKET, SO_REUSEADDR, &[1, 0]) == Err(EINVAL)
        && options.set(SOL_SOCKET, SO_RCVTIMEO, &bad_timeout) == Err(EDOM)
        && options.set(SOL_SOCKET, SO_LINGER, &[0; 4]) == Err(EINVAL)
        && options.set(IPPROTO_IP, IP_TTL, &int(0)) == Err(EINVAL)
        && options.set(IPPROTO_IP, IP_TTL, &int(256)) == Err(EINVAL)
        && options.set(IPPROTO_TCP, TCP_KEEPIDLE, &int(0)) == Err(EINVAL)
        && options.set(SOL_SOCKET, 999, &int(1)) == Err(ENOPROTOOPT)
        && options.set(99, 1, &int(1)) == Err(ENOPROTOOPT)
        && options.get(SOL_SOCKET, 999).is_none()
        && options.reuse_address;
    stored && linger_back && timeout_back && values && errors
}
