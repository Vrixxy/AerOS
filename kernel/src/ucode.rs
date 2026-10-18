//! Loads a microcode update from a file onto every processor.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::acpi::MAX_PROCESSORS;
use crate::microcode::{self, Cpu};
use crate::{smp, vfs};

/// Read at boot when it exists (an Intel or AMD blob, see `microcode.rs`).
pub const BOOT_BLOB: &str = "/etc/ucode.bin";

static AFTER: [AtomicU32; MAX_PROCESSORS] = [const { AtomicU32::new(0) }; MAX_PROCESSORS];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// Under a hypervisor or on an unknown vendor: not the guest's to update.
    Unavailable,
    NoFile,
    NoUpdate {
        current: u32,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Loaded {
    pub before: u32,
    pub after: u32,
    pub processors: usize,
    pub failed: usize,
}

fn apply_job(address: u64) {
    let Some(logical) = smp::current_logical() else {
        return;
    };
    if let Some(cpu) = microcode::identify() {
        let revision = unsafe { microcode::apply_here(&cpu, address) };
        AFTER[logical].store(revision, Ordering::Release);
    }
}

fn load_blob(cpu: &Cpu, blob: &[u8]) -> Result<Loaded, Error> {
    let patch = microcode::find(blob, cpu).ok_or(Error::NoUpdate {
        current: cpu.revision,
    })?;
    let address = blob.as_ptr() as u64 + patch.offset as u64;
    let own = smp::current_logical().unwrap_or(0);
    for slot in &AFTER {
        slot.store(0, Ordering::Release);
    }
    let mut started = [false; MAX_PROCESSORS];
    for (logical, started) in started.iter_mut().enumerate().skip(1) {
        if logical != own && smp::online_mask() & (1 << logical) != 0 {
            *started = smp::start_job(logical, apply_job, address);
        }
    }
    let after = unsafe { microcode::apply_here(cpu, address) };
    AFTER[own].store(after, Ordering::Release);
    let mut processors = 1;
    let mut failed = usize::from(after != patch.revision);
    for (logical, started) in started.iter().enumerate() {
        if !started {
            continue;
        }
        processors += 1;
        let start = crate::time::monotonic_nanoseconds();
        while !smp::reap_job(logical)
            && crate::time::monotonic_nanoseconds().wrapping_sub(start) < 500_000_000
        {
            core::hint::spin_loop();
        }
        failed += usize::from(AFTER[logical].load(Ordering::Acquire) != patch.revision);
    }
    Ok(Loaded {
        before: cpu.revision,
        after,
        processors,
        failed,
    })
}

pub fn load(path: &str) -> Result<Loaded, Error> {
    let cpu = microcode::identify().ok_or(Error::Unavailable)?;
    if vfs::needs_buffer(path) {
        vfs::with_home_file(path, |data, _| load_blob(&cpu, data)).map_err(|_| Error::NoFile)?
    } else {
        let file = vfs::file(path).map_err(|_| Error::NoFile)?;
        load_blob(&cpu, file.data)
    }
}
