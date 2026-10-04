//! Power-loss testing of the `/home`/`/media` filesystem (`fatfs`): each
//! operation is cut off after every possible number of sector writes on a
//! RAM disk, then the volume is checked for broken chains and the file's
//! contents compared with what either the old or the new state allows.

use crate::fat::{self, Fat};
use crate::fatfs::{Disk, Fs};

const FILE: &[u8] = b"CRASHTST.BIN";
const SOURCE: &[u8] = b"CRASHSRC.BIN";
const SHORT_NAME: [u8; 11] = *b"CRASHTSTBIN";
const MAX_BYTES: usize = 9000;

#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    Append,
    Create,
    Shrink,
    Empty,
    Remove,
    Rename,
}

pub struct FsCrashReport {
    pub cases: u32,
    pub structural: u32,
    pub torn: u32,
    pub leaks_repaired: u32,
    pub repair_failures: u32,
    pub verified: bool,
}

fn pattern(seed: u8, index: usize) -> u8 {
    (index % 251) as u8 ^ seed
}

fn fill(seed: u8, from: usize, buffer: &mut [u8]) {
    for (offset, byte) in buffer.iter_mut().enumerate() {
        *byte = pattern(seed, from + offset);
    }
}

/// What a surviving file must look like: its size must be one of `sizes`,
/// and its first `initial` bytes follow the old pattern while the rest follow
/// the new one.
struct Expectation {
    sizes: [usize; 2],
    initial: usize,
}

fn expectation(scenario: Scenario) -> Option<Expectation> {
    Some(match scenario {
        Scenario::Append => Expectation {
            sizes: [3000, 9000],
            initial: 3000,
        },
        Scenario::Create => Expectation {
            sizes: [0, 5000],
            initial: 0,
        },
        Scenario::Shrink => Expectation {
            sizes: [9000, 2500],
            initial: 9000,
        },
        Scenario::Empty => Expectation {
            sizes: [5000, 0],
            initial: 5000,
        },
        Scenario::Remove | Scenario::Rename => return None,
    })
}

/// After a cut during rename-over: the destination must exist holding either
/// its old bytes or the source's, and the source, if still there, must be intact.
fn rename_torn(fs: &mut Fs, root: u32, buffer: &mut [u8; MAX_BYTES]) -> bool {
    let Ok(mut dest) = fs.find(root, FILE) else {
        return true;
    };
    let size = dest.size as usize;
    let seed = match size {
        3000 => 0xa1,
        5000 => 0xb2,
        _ => return true,
    };
    if fs.read_at(&mut dest, 0, &mut buffer[..size]) != Ok(size)
        || !buffer[..size]
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == pattern(seed, index))
    {
        return true;
    }
    if let Ok(mut source) = fs.find(root, SOURCE) {
        let intact = source.size == 5000
            && fs.read_at(&mut source, 0, &mut buffer[..5000]) == Ok(5000)
            && buffer[..5000]
                .iter()
                .enumerate()
                .all(|(index, byte)| *byte == pattern(0xb2, index));
        if !intact {
            return true;
        }
    }
    false
}

/// Runs one case; returns `(writes dropped, structural problems, torn)`.
fn run_case(scenario: Scenario, limit: u32) -> Option<(u64, u32, bool, u32, bool)> {
    Fs::format(Disk::Ram, 0, crate::blockdev::RAM_DISK_SECTORS, b"CRASHVOL").ok()?;
    let mut buffer = [0u8; MAX_BYTES];
    let dropped = {
        let mut fs = Fs::mount(Disk::Ram, 0).ok()?;
        let root = fs.root().as_dir();
        match scenario {
            Scenario::Create => {}
            Scenario::Append => {
                let mut node = fs.create_file(root, FILE).ok()?;
                fill(0xa1, 0, &mut buffer[..3000]);
                fs.write_at(&mut node, 0, &buffer[..3000]).ok()?;
            }
            Scenario::Shrink => {
                let mut node = fs.create_file(root, FILE).ok()?;
                fill(0xa1, 0, &mut buffer[..9000]);
                fs.write_at(&mut node, 0, &buffer[..9000]).ok()?;
            }
            Scenario::Empty | Scenario::Remove => {
                let mut node = fs.create_file(root, FILE).ok()?;
                fill(0xa1, 0, &mut buffer[..5000]);
                fs.write_at(&mut node, 0, &buffer[..5000]).ok()?;
            }
            Scenario::Rename => {
                let mut dest = fs.create_file(root, FILE).ok()?;
                fill(0xa1, 0, &mut buffer[..3000]);
                fs.write_at(&mut dest, 0, &buffer[..3000]).ok()?;
                let mut source = fs.create_file(root, SOURCE).ok()?;
                fill(0xb2, 0, &mut buffer[..5000]);
                fs.write_at(&mut source, 0, &buffer[..5000]).ok()?;
            }
        }
        crate::blockdev::inject_power_loss_after(limit);
        match scenario {
            Scenario::Create => {
                if let Ok(mut node) = fs.create_file(root, FILE) {
                    fill(0xb2, 0, &mut buffer[..5000]);
                    let _ = fs.write_at(&mut node, 0, &buffer[..5000]);
                }
            }
            Scenario::Append => {
                if let Ok(mut node) = fs.find(root, FILE) {
                    fill(0xb2, 3000, &mut buffer[..6000]);
                    let _ = fs.write_at(&mut node, 3000, &buffer[..6000]);
                }
            }
            Scenario::Shrink | Scenario::Empty => {
                if let Ok(mut node) = fs.find(root, FILE) {
                    let length = if scenario == Scenario::Shrink {
                        2500
                    } else {
                        0
                    };
                    let _ = fs.truncate(&mut node, length);
                }
            }
            Scenario::Remove => {
                if let Ok(node) = fs.find(root, FILE) {
                    let _ = fs.remove(&node);
                }
            }
            Scenario::Rename => {
                if let Ok(node) = fs.find(root, SOURCE) {
                    let _ = fs.rename(&node, root, FILE, true);
                }
            }
        }
        crate::blockdev::clear_power_loss()
    };

    crate::blockdev::use_ram_disk(true);
    fat::reset_check_baseline();
    let checked = fat::with_ram_volume(|| {
        fat::Fat::mounted().and_then(|volume: Fat| fat::check_root_chains(&volume, &SHORT_NAME))
    })
    .flatten();
    crate::blockdev::use_ram_disk(false);
    let checked = checked?;
    let cross_linked = if scenario == Scenario::Rename {
        0
    } else {
        checked.cross_linked
    };
    let mut structural = checked.dangling + cross_linked + checked.short;

    let mut fs = Fs::mount(Disk::Ram, 0).ok()?;
    let root = fs.root().as_dir();
    let mut torn = false;
    let existing = fs.find(root, FILE);
    if existing.is_ok() && !checked.found {
        structural += 1;
    }
    if scenario == Scenario::Rename {
        torn = rename_torn(&mut fs, root, &mut buffer);
    }
    match (existing, expectation(scenario)) {
        (_, _) if scenario == Scenario::Rename => {}
        (Ok(mut node), Some(expected)) => {
            let size = node.size as usize;
            if !expected.sizes.contains(&size) {
                torn = true;
            } else {
                buffer.fill(0);
                let read = fs.read_at(&mut node, 0, &mut buffer[..size]);
                let new_seed = 0xb2;
                let content_ok = read == Ok(size)
                    && buffer[..size].iter().enumerate().all(|(index, byte)| {
                        let seed = if index < expected.initial {
                            0xa1
                        } else {
                            new_seed
                        };
                        *byte == pattern(seed, index)
                    });
                torn = !content_ok;
            }
        }
        (Ok(mut node), None) => {
            let size = node.size as usize;
            let read = fs.read_at(&mut node, 0, &mut buffer[..size.min(MAX_BYTES)]);
            torn = size != 5000
                || read != Ok(5000)
                || !buffer[..5000]
                    .iter()
                    .enumerate()
                    .all(|(index, byte)| *byte == pattern(0xa1, index));
        }
        (Err(_), _) => {}
    }
    let first = fs.fsck(true).ok()?;
    let second = fs.fsck(false).ok()?;
    let both_gone = scenario == Scenario::Rename
        && fs.find(root, FILE).is_err()
        && fs.find(root, SOURCE).is_err();
    let repair_failed = second.damaged() || second.orphan_clusters != 0 || both_gone;
    Some((
        dropped,
        structural,
        torn,
        first.orphan_clusters,
        repair_failed,
    ))
}

pub fn run() -> FsCrashReport {
    let mut report = FsCrashReport {
        cases: 0,
        structural: 0,
        torn: 0,
        leaks_repaired: 0,
        repair_failures: 0,
        verified: false,
    };
    let mut complete = true;
    'scenarios: for scenario in [
        Scenario::Create,
        Scenario::Append,
        Scenario::Shrink,
        Scenario::Empty,
        Scenario::Remove,
        Scenario::Rename,
    ] {
        for limit in 0..600u32 {
            let Some((dropped, structural, torn, leaked, repair_failed)) =
                run_case(scenario, limit)
            else {
                complete = false;
                break 'scenarios;
            };
            report.cases += 1;
            report.structural += structural;
            report.torn += u32::from(torn);
            report.leaks_repaired += leaked;
            report.repair_failures += u32::from(repair_failed);
            if dropped == 0 {
                break;
            }
        }
    }
    crate::blockdev::clear_power_loss();
    crate::blockdev::use_ram_disk(false);
    report.verified = complete
        && report.cases > 0
        && report.structural == 0
        && report.torn == 0
        && report.repair_failures == 0;
    report
}
