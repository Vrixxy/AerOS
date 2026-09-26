//! SNTP client: asks a public time server for the time over UDP and sets the
//! system clock (which then also runs on the network's idea of "now", not
//! only whatever the hardware clock said at boot).

use crate::net;

const PORT: u16 = 123;
/// Seconds between 1900-01-01 (NTP epoch) and 1970-01-01.
const NTP_TO_UNIX: u64 = 2_208_988_800;
const SERVERS: [&str; 2] = ["pool.ntp.org", "time.cloudflare.com"];

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct SyncReport {
    pub synced: bool,
    pub server: [u8; 4],
    /// New time minus the old clock, in seconds.
    pub adjustment: i64,
    pub unix_seconds: u64,
}

impl SyncReport {
    const NONE: Self = Self {
        synced: false,
        server: [0; 4],
        adjustment: 0,
        unix_seconds: 0,
    };
}

/// Asks one server; returns the Unix time it reported.
fn query(server: [u8; 4]) -> Option<u64> {
    let mut request = [0u8; 48];
    // LI 0, version 4, mode 3 (client).
    request[0] = 0x23;
    // Our transmit timestamp is an unpredictable nonce the server echoes.
    let mut nonce = [0u8; 8];
    crate::random::fill(&mut nonce);
    request[40..48].copy_from_slice(&nonce);
    let source_port = 49_200 + (nonce[0] as u16 % 200);
    if !net::send_udp(server, source_port, PORT, &request) {
        return None;
    }
    let mut response = [0u8; 128];
    let start = crate::time::monotonic_nanoseconds();
    while crate::time::monotonic_nanoseconds().saturating_sub(start) < 1_500_000_000 {
        let Some(datagram) = net::receive_udp(server, PORT, source_port, &mut response) else {
            continue;
        };
        if datagram.bytes < 48 {
            continue;
        }
        let mode = response[0] & 7;
        let stratum = response[1];
        // The reply must echo our nonce (origin timestamp) and be a server
        // that has a time (stratum 1..=15, not "kiss of death").
        if mode != 4 || stratum == 0 || stratum > 15 || response[24..32] != nonce {
            continue;
        }
        let seconds =
            u32::from_be_bytes([response[40], response[41], response[42], response[43]]) as u64;
        if seconds < NTP_TO_UNIX {
            continue;
        }
        return Some(seconds - NTP_TO_UNIX);
    }
    None
}

/// Tries the servers in turn and sets the clock from the first answer.
pub fn sync() -> SyncReport {
    for name in SERVERS {
        let lookup = net::resolve(name);
        if !lookup.verified {
            continue;
        }
        if let Some(time) = query(lookup.address) {
            let before = crate::rtc::unix_seconds();
            crate::rtc::set_unix_seconds(time);
            return SyncReport {
                synced: true,
                server: lookup.address,
                adjustment: time as i64 - before as i64,
                unix_seconds: time,
            };
        }
    }
    SyncReport::NONE
}

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static NEXT_ATTEMPT_NS: AtomicU64 = AtomicU64::new(0);
static FAILURES: AtomicU32 = AtomicU32::new(0);
static LAST_SYNC: AtomicU64 = AtomicU64::new(0);

/// Unix time of the last successful sync (0 = never).
#[allow(dead_code)]
pub fn last_sync() -> u64 {
    LAST_SYNC.load(Ordering::Relaxed)
}

/// Called from the desktop loop: syncs a while after start-up (once the
/// network is up), then every six hours; a failed try backs off.
pub fn tick(now_ns: u64) {
    if !crate::settings::ntp_enabled() || !net::is_ready() {
        return;
    }
    let next = NEXT_ATTEMPT_NS.load(Ordering::Relaxed);
    if next == 0 {
        NEXT_ATTEMPT_NS.store(now_ns + 12_000_000_000, Ordering::Relaxed);
        return;
    }
    if now_ns < next {
        return;
    }
    let report = sync();
    let delay = if report.synced {
        FAILURES.store(0, Ordering::Relaxed);
        LAST_SYNC.store(report.unix_seconds, Ordering::Relaxed);
        6 * 3600 * 1_000_000_000u64
    } else {
        let failures = FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
        60_000_000_000u64 * (1 << failures.min(5))
    };
    NEXT_ATTEMPT_NS.store(now_ns + delay, Ordering::Relaxed);
}
