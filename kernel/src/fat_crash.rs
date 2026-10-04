//! Power-loss testing of the FAT writer: writes are cut off after every
//! possible number of sector writes, then the volume is checked for chains
//! that run into free clusters, shared clusters and torn file contents.

use crate::fat::{
    ChainCheck, Fat, check_root_chains, delete_root_file, forget_caches, read_root_file,
    reset_check_baseline, write_root_file,
};
use crate::sync::TicketLock;

const NAME: [u8; 11] = *b"CRASHT  BIN";
const MAX_BYTES: usize = 9000;
/// (old size, new size) of the file in each round of cases: grow, shrink,
/// empty to non-empty, non-empty to empty, and an unchanged size.
const SIZES: [(usize, usize); 5] = [
    (3000, 9000),
    (9000, 3000),
    (0, 5000),
    (5000, 0),
    (1024, 1024),
];
const BUDGET_NS: u64 = 60_000_000_000;

static OLD_DATA: TicketLock<[u8; MAX_BYTES]> = TicketLock::new([0; MAX_BYTES]);
static NEW_DATA: TicketLock<[u8; MAX_BYTES]> = TicketLock::new([0; MAX_BYTES]);
static READBACK: TicketLock<[u8; MAX_BYTES]> = TicketLock::new([0; MAX_BYTES]);

pub struct CrashReport {
    pub cases: u32,
    pub dangling: u32,
    pub cross_linked: u32,
    pub short: u32,
    pub torn: u32,
    pub lost: u32,
    pub elapsed_ms: u64,
    pub verified: bool,
}

fn pattern(seed: u8, buffer: &mut [u8]) {
    for (index, byte) in buffer.iter_mut().enumerate() {
        *byte = (index % 251) as u8 ^ seed;
    }
}

fn matches(seed: u8, buffer: &[u8]) -> bool {
    buffer
        .iter()
        .enumerate()
        .all(|(index, byte)| *byte == (index % 251) as u8 ^ seed)
}

#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    Replace,
    Create,
    Delete,
}

fn is_clean(found: &ChainCheck) -> bool {
    found.dangling == 0 && found.cross_linked == 0 && found.short == 0
}

const FAT_SECTORS: u64 = 32;
const ROOT_SECTORS: u64 = 32;

/// Formats the RAM disk as an 8 MiB FAT16 volume: 1 reserved sector, two
/// 32-sector FATs, a 512-entry root directory, 1 KiB clusters.
pub fn format_ram_disk() -> bool {
    let zero = [0u8; 512];
    let data_start = 1 + 2 * FAT_SECTORS + ROOT_SECTORS;
    for lba in 1..data_start {
        if !crate::blockdev::write_boot_sector(lba, &zero) {
            return false;
        }
    }
    let mut boot = [0u8; 512];
    boot[..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    boot[3..11].copy_from_slice(b"AEROSFAT");
    boot[11..13].copy_from_slice(&512u16.to_le_bytes());
    boot[13] = 2;
    boot[14..16].copy_from_slice(&1u16.to_le_bytes());
    boot[16] = 2;
    boot[17..19].copy_from_slice(&512u16.to_le_bytes());
    boot[19..21].copy_from_slice(&(crate::blockdev::RAM_DISK_SECTORS as u16).to_le_bytes());
    boot[21] = 0xf8;
    boot[22..24].copy_from_slice(&(FAT_SECTORS as u16).to_le_bytes());
    boot[24..26].copy_from_slice(&32u16.to_le_bytes());
    boot[26..28].copy_from_slice(&2u16.to_le_bytes());
    boot[38] = 0x29;
    boot[54..62].copy_from_slice(b"FAT16   ");
    boot[510] = 0x55;
    boot[511] = 0xaa;
    let mut first_fat = [0u8; 512];
    first_fat[..4].copy_from_slice(&[0xf8, 0xff, 0xff, 0xff]);
    crate::blockdev::write_boot_sector(0, &boot)
        && crate::blockdev::write_boot_sector(1, &first_fat)
        && crate::blockdev::write_boot_sector(1 + FAT_SECTORS, &first_fat)
}

/// Runs the power-loss cases against a RAM-backed FAT volume (never the real
/// ESP, whose QEMU `vvfat` backend cannot take simulated half-finished writes).
pub fn run() -> CrashReport {
    let mut report = CrashReport {
        cases: 0,
        dangling: 0,
        cross_linked: 0,
        short: 0,
        torn: 0,
        lost: 0,
        elapsed_ms: 0,
        verified: false,
    };
    let started = crate::time::monotonic_nanoseconds();
    crate::blockdev::use_ram_disk(true);
    if format_ram_disk() {
        let _ = crate::fat::with_ram_volume(|| execute(&mut report, started));
    }
    crate::blockdev::clear_power_loss();
    crate::blockdev::use_ram_disk(false);
    report.elapsed_ms = (crate::time::monotonic_nanoseconds() - started) / 1_000_000;
    report
}
fn execute(report: &mut CrashReport, started: u64) {
    let Some(fat) = Fat::mounted() else {
        return;
    };
    reset_check_baseline();
    pattern(0xa1, &mut OLD_DATA.lock()[..]);
    pattern(0xb2, &mut NEW_DATA.lock()[..]);
    let _ = delete_root_file(&NAME);

    for (old_bytes, new_bytes) in SIZES {
        for scenario in [Scenario::Replace, Scenario::Create, Scenario::Delete] {
            for limit in 0..400u32 {
                if crate::time::monotonic_nanoseconds() - started > BUDGET_NS {
                    return;
                }
                let _ = delete_root_file(&NAME);
                if scenario != Scenario::Create
                    && !write_root_file(&NAME, &OLD_DATA.lock()[..old_bytes])
                {
                    return;
                }
                crate::blockdev::inject_power_loss_after(limit);
                if scenario == Scenario::Delete {
                    let _ = delete_root_file(&NAME);
                } else {
                    let _ = write_root_file(&NAME, &NEW_DATA.lock()[..new_bytes]);
                }
                let dropped = crate::blockdev::clear_power_loss();
                forget_caches();
                report.cases += 1;

                let Some(found) = check_root_chains(&fat, &NAME) else {
                    return;
                };
                report.dangling += found.dangling;
                report.cross_linked += found.cross_linked;
                report.short += found.short;

                let mut readback = READBACK.lock();
                match read_root_file(&NAME, &mut readback[..]) {
                    None => {
                        if scenario == Scenario::Replace {
                            report.lost += 1;
                        }
                    }
                    Some(length) => {
                        let old = length == old_bytes && matches(0xa1, &readback[..length]);
                        let new = length == new_bytes && matches(0xb2, &readback[..length]);
                        let acceptable = match scenario {
                            Scenario::Replace => old || new,
                            Scenario::Create => new,
                            Scenario::Delete => old,
                        };
                        if !acceptable {
                            report.torn += 1;
                        }
                    }
                }
                drop(readback);
                if dropped == 0 {
                    break;
                }
            }
        }
    }
    let _ = delete_root_file(&NAME);
    forget_caches();
    let clean = check_root_chains(&fat, &NAME).is_some_and(|found| is_clean(&found));
    report.verified = report.cases > 0
        && report.dangling == 0
        && report.cross_linked == 0
        && report.short == 0
        && report.torn == 0
        && report.lost == 0
        && clean;
}
