//! `perf_event_open` for software counters: CPU clock, task clock and
//! context switches of the calling process, read with `read` and driven by
//! the ENABLE/DISABLE/RESET ioctls. Hardware counters, sampling and events
//! AerOS does not count answer as Linux does when the PMU lacks them, so
//! tools print "not supported" and go on.

use super::*;

const PERF_TYPE_SOFTWARE: u32 = 1;
const CPU_CLOCK: u64 = 0;
const TASK_CLOCK: u64 = 1;
const CONTEXT_SWITCHES: u64 = 3;
const DUMMY: u64 = 9;
const FORMAT_ENABLED: u64 = 1;
const FORMAT_RUNNING: u64 = 2;
const FORMAT_ID: u64 = 4;
const FORMAT_GROUP: u64 = 8;
const FLAG_FD_CLOEXEC: u64 = 8;
const DISABLED_BIT: u64 = 1;
const IOC_ENABLE: u64 = 0x2400;
const IOC_DISABLE: u64 = 0x2401;
const IOC_RESET: u64 = 0x2403;
const IOC_ID: u64 = 0x8008_2407;

/// The live reading of counter `event` for the caller.
fn reading(event: u64) -> u64 {
    match event {
        CPU_CLOCK => crate::time::monotonic_nanoseconds(),
        TASK_CLOCK => {
            let (user, system) = crate::scheduler::current_cpu_time();
            user + system
        }
        CONTEXT_SWITCHES => crate::scheduler::current_switches(),
        _ => 0,
    }
}

/// `perf_event_open(attr, pid, cpu, group_fd, flags)`.
pub(super) fn open(attr: u64, pid: u64, cpu: u64, group: u64, flags: u64) -> u64 {
    let mut raw = [0u8; 48];
    if !user::copy_from_user(attr, &mut raw) {
        return error(14);
    }
    let field = |at: usize| u64::from_le_bytes(raw[at..at + 8].try_into().unwrap_or([0; 8]));
    let kind = u32::from_le_bytes(raw[..4].try_into().unwrap_or([0; 4]));
    let size = u32::from_le_bytes(raw[4..8].try_into().unwrap_or([0; 4]));
    let config = field(8);
    let sample_period = field(16);
    let read_format = field(32);
    let bits = field(40);
    if size != 0 && size < 64 {
        return error(7);
    }
    if flags & !FLAG_FD_CLOEXEC != 0 || group as i64 != -1 {
        return error(22);
    }
    let own =
        crate::scheduler::current_task_pid_for_linux().unwrap_or_else(crate::process::current_pid);
    // Only the caller itself: watching another process or a whole CPU needs
    // what perf_event_paranoid keeps from ordinary programs.
    if !(pid == 0 || pid == own) {
        return error(13);
    }
    if !(cpu as i64 == -1 || cpu == 0) {
        return error(13);
    }
    if kind != PERF_TYPE_SOFTWARE || !matches!(config, CPU_CLOCK | TASK_CLOCK | CONTEXT_SWITCHES | DUMMY)
    {
        return error(2);
    }
    if sample_period != 0 || read_format & FORMAT_GROUP != 0 {
        return error(95);
    }
    let enabled = bits & DISABLED_BIT == 0;
    let now = crate::time::monotonic_nanoseconds();
    let template = Pipe {
        perf: config as u8 + 1,
        perf_format: read_format as u8,
        perf_on: enabled,
        perf_base: if enabled { reading(config) } else { 0 },
        perf_since: now,
        ..Pipe::EMPTY
    };
    match create_single_fd_pipe(template, flags & FLAG_FD_CLOEXEC != 0, false) {
        Ok(fd) => fd,
        Err(failure) => error(failure),
    }
}

/// `read` on a counter: the value, then the enabled and running times and
/// the id as `read_format` asks.
pub(super) fn read(slot: usize, destination: &mut [u8]) -> Option<Result<usize, u64>> {
    let pipes = PIPES.lock();
    let pipe = pipes.get(slot).filter(|pipe| pipe.used && pipe.perf != 0)?;
    let event = pipe.perf as u64 - 1;
    let now = crate::time::monotonic_nanoseconds();
    let mut value = pipe.counter;
    let mut time = pipe.perf_time;
    if pipe.perf_on {
        value += reading(event).saturating_sub(pipe.perf_base);
        time += now.saturating_sub(pipe.perf_since);
    }
    let format = pipe.perf_format as u64;
    let mut words = [0u64; 4];
    let mut count = 0;
    for (wanted, word) in [(true, value), (format & FORMAT_ENABLED != 0, time), (format & FORMAT_RUNNING != 0, time), (format & FORMAT_ID != 0, slot as u64 + 1)] {
        if wanted {
            words[count] = word;
            count += 1;
        }
    }
    let bytes = count * 8;
    if destination.len() < bytes {
        return Some(Err(28));
    }
    for (index, word) in words[..count].iter().enumerate() {
        destination[index * 8..index * 8 + 8].copy_from_slice(&word.to_le_bytes());
    }
    Some(Ok(bytes))
}

/// The counter ioctls; `None` when `slot` is not a counter.
pub(super) fn ioctl(slot: usize, request: u64, address: u64) -> Option<u64> {
    if request == IOC_ID {
        PIPES.lock().get(slot).filter(|pipe| pipe.used && pipe.perf != 0)?;
        let id = slot as u64 + 1;
        return Some(if user::copy_to_user(address, &id.to_le_bytes()) { 0 } else { error(14) });
    }
    let mut pipes = PIPES.lock();
    let pipe = pipes.get_mut(slot).filter(|pipe| pipe.used && pipe.perf != 0)?;
    let event = pipe.perf as u64 - 1;
    let now = crate::time::monotonic_nanoseconds();
    Some(match request {
        IOC_ENABLE => {
            if !pipe.perf_on {
                pipe.perf_on = true;
                pipe.perf_base = reading(event);
                pipe.perf_since = now;
            }
            0
        }
        IOC_DISABLE => {
            if pipe.perf_on {
                pipe.counter += reading(event).saturating_sub(pipe.perf_base);
                pipe.perf_time += now.saturating_sub(pipe.perf_since);
                pipe.perf_on = false;
            }
            0
        }
        IOC_RESET => {
            pipe.counter = 0;
            pipe.perf_base = reading(event);
            0
        }
        _ => error(25),
    })
}
