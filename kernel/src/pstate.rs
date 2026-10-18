//! Moves every processor to one of the performance states the firmware lists.
//! The write is the one `cpufreq::plan` chooses; it runs here and, through
//! the work queue, on the other processors. Nothing happens under a
//! hypervisor, where the host decides the clock.

use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::acpi::MAX_PROCESSORS;
use crate::cpufreq::{self, Switch};
use crate::{acpi_ns, microcode, smp};

const NONE: usize = usize::MAX;

static CURRENT: AtomicUsize = AtomicUsize::new(NONE);
static KIND: AtomicU32 = AtomicU32::new(0);
static FIRST: AtomicU64 = AtomicU64::new(0);
static SECOND: AtomicU64 = AtomicU64::new(0);
static WIDTH: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Virtualised,
    NoNamespace,
    NoStates,
    NoRegister,
    BadState,
    Stuck,
}

/// The state the last successful `set` chose.
pub fn current() -> Option<usize> {
    let index = CURRENT.load(Ordering::Acquire);
    (index != NONE).then_some(index)
}

fn publish(switch: Switch) {
    let (kind, first, second, width) = match switch {
        Switch::Io { port, width, value } => (1, u64::from(port), u64::from(value), width),
        Switch::Msr { register, value } => (2, u64::from(register), value, 0),
        Switch::Memory {
            address,
            width,
            value,
        } => (3, address, value, width),
    };
    FIRST.store(first, Ordering::Release);
    SECOND.store(second, Ordering::Release);
    WIDTH.store(u32::from(width), Ordering::Release);
    KIND.store(kind, Ordering::Release);
}

fn write_here() {
    let first = FIRST.load(Ordering::Acquire);
    let second = SECOND.load(Ordering::Acquire);
    let width = WIDTH.load(Ordering::Acquire);
    unsafe {
        match KIND.load(Ordering::Acquire) {
            1 => {
                let port = first as u16;
                match width {
                    8 => {
                        asm!("out dx, al", in("dx") port, in("al") second as u8, options(nomem, nostack))
                    }
                    16 => {
                        asm!("out dx, ax", in("dx") port, in("ax") second as u16, options(nomem, nostack))
                    }
                    _ => {
                        asm!("out dx, eax", in("dx") port, in("eax") second as u32, options(nomem, nostack))
                    }
                }
            }
            2 => asm!(
                "wrmsr",
                in("ecx") first as u32,
                in("eax") second as u32,
                in("edx") (second >> 32) as u32,
                options(nomem, nostack, preserves_flags)
            ),
            3 => match width {
                8 => core::ptr::write_volatile(first as *mut u8, second as u8),
                16 => core::ptr::write_volatile(first as *mut u16, second as u16),
                32 => core::ptr::write_volatile(first as *mut u32, second as u32),
                _ => core::ptr::write_volatile(first as *mut u64, second),
            },
            _ => {}
        }
    }
}

fn job(_: u64) {
    write_here();
}

/// Moves every online processor to state `index` of the firmware's list.
pub fn set(index: usize) -> Result<usize, Error> {
    if microcode::virtualised() {
        return Err(Error::Virtualised);
    }
    if !acpi_ns::report().loaded {
        return Err(Error::NoNamespace);
    }
    let intel = microcode::vendor() == Some(microcode::Vendor::Intel);
    let planned = acpi_ns::with(|aml| {
        let (node, table) = cpufreq::find(aml).ok_or(Error::NoStates)?;
        let state = *table.states().get(index).ok_or(Error::BadState)?;
        if index < table.first_allowed {
            return Err(Error::BadState);
        }
        let register = cpufreq::control_register(aml, node).ok_or(Error::NoRegister)?;
        Ok(cpufreq::plan(register, &state, intel))
    })?;
    publish(planned);
    let own = smp::current_logical().unwrap_or(0);
    let mut started = [false; MAX_PROCESSORS];
    for (logical, started) in started.iter_mut().enumerate().skip(1) {
        if logical != own && smp::online_mask() & (1 << logical) != 0 {
            *started = smp::start_job(logical, job, 0);
        }
    }
    write_here();
    let mut incomplete = false;
    for (logical, started) in started.iter().enumerate() {
        if !started {
            continue;
        }
        let begin = crate::time::monotonic_nanoseconds();
        let mut done = smp::reap_job(logical);
        while !done && crate::time::monotonic_nanoseconds().wrapping_sub(begin) < 500_000_000 {
            core::hint::spin_loop();
            done = smp::reap_job(logical);
        }
        incomplete |= !done;
    }
    if incomplete {
        return Err(Error::Stuck);
    }
    CURRENT.store(index, Ordering::Release);
    Ok(index)
}
