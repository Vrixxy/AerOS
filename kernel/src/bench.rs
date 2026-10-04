//! Micro-benchmarks behind the `bench` shell command. Each one runs for a
//! fixed slice of wall-clock time and reports a rate, so results stay
//! meaningful on both fast and slow (emulated) machines.

use core::fmt::Write;
use core::hint::black_box;

use core::sync::atomic::{AtomicU64, Ordering};

use crate::acpi::MAX_PROCESSORS;
use crate::time::monotonic_nanoseconds;

const SLICE_NS: u64 = 60_000_000;

pub const LABELS: [&str; 6] = ["memcpy", "sha256", "random", "lookup", "tmpfs-io", "yield"];

/// Calls `step` until the time slice is used up and returns `(units, ns)`,
/// where `step` returns how many units (bytes or operations) it handled.
fn measure(mut step: impl FnMut() -> u64) -> (u64, u64) {
    let started = monotonic_nanoseconds();
    let mut units = 0;
    loop {
        units += step();
        let elapsed = monotonic_nanoseconds() - started;
        if elapsed >= SLICE_NS {
            return (units, elapsed);
        }
    }
}

fn mib_per_second(bytes: u64, ns: u64) -> u64 {
    (bytes as u128 * 1_000_000_000 / ns.max(1) as u128 / 1_048_576) as u64
}

fn per_second(operations: u64, ns: u64) -> u64 {
    (operations as u128 * 1_000_000_000 / ns.max(1) as u128) as u64
}

pub fn run(output: &mut impl Write) {
    let mut source = [0xa5u8; 16 * 1024];
    let mut target = [0u8; 16 * 1024];
    let (bytes, ns) = measure(|| {
        target.copy_from_slice(black_box(&source));
        source[0] = target[1];
        source.len() as u64
    });
    let _ = writeln!(output, "memcpy    {:>8} MiB/s", mib_per_second(bytes, ns));

    let block = [0x5au8; 4096];
    let (bytes, ns) = measure(|| {
        black_box(crate::auth::sha256(black_box(&block)));
        block.len() as u64
    });
    let _ = writeln!(output, "sha256    {:>8} MiB/s", mib_per_second(bytes, ns));

    let mut random = [0u8; 4096];
    let (bytes, ns) = measure(|| {
        crate::random::fill(&mut random);
        random.len() as u64
    });
    let _ = writeln!(output, "random    {:>8} MiB/s", mib_per_second(bytes, ns));

    let (operations, ns) = measure(|| {
        black_box(crate::vfs::file("/bin/init").is_ok());
        1
    });
    let _ = writeln!(output, "lookup    {:>8} ops/s", per_second(operations, ns));

    let path = "/tmp/.bench";
    let chunk = [0x42u8; 512];
    let (operations, ns) = measure(|| {
        let written = crate::vfs::open_file(path, true, false, true, 0o644, true)
            .and_then(|handle| {
                let result = crate::vfs::write(handle, &chunk, false);
                let _ = crate::vfs::close(handle);
                result
            })
            .is_ok();
        u64::from(written)
    });
    let _ = crate::vfs::remove(path, false);
    let _ = writeln!(output, "tmpfs-io  {:>8} ops/s", per_second(operations, ns));

    let (operations, ns) = measure(|| {
        black_box(crate::scheduler::yield_now());
        1
    });
    let _ = writeln!(
        output,
        "yield     {:>8} calls/s",
        per_second(operations, ns)
    );
}

static SMP_RATES: [AtomicU64; MAX_PROCESSORS] = [const { AtomicU64::new(0) }; MAX_PROCESSORS];

fn sha256_rate() -> u64 {
    let block = [0x5au8; 4096];
    let (bytes, ns) = measure(|| {
        black_box(crate::auth::sha256(black_box(&block)));
        block.len() as u64
    });
    mib_per_second(bytes, ns)
}

fn sha256_job(_: u64) {
    if let Some(cpu) = crate::smp::current_logical() {
        SMP_RATES[cpu].store(sha256_rate().max(1), Ordering::Release);
    }
}

/// Hashes on every processor at once and reports each one's rate and the
/// total.
pub fn run_smp(output: &mut impl Write) {
    let online = crate::smp::online_mask();
    for rate in &SMP_RATES {
        rate.store(0, Ordering::Release);
    }
    let mut jobs = [None; MAX_PROCESSORS];
    for (cpu, slot) in jobs.iter_mut().enumerate().skip(1) {
        if online & (1 << cpu) != 0 {
            *slot = crate::smpsched::spawn(sha256_job, 0, 0, 1 << cpu);
        }
    }
    SMP_RATES[0].store(sha256_rate().max(1), Ordering::Release);
    let mut total = 0;
    for (cpu, job) in jobs.iter().enumerate() {
        if let Some(id) = job {
            let _ = crate::smpsched::wait(*id, 2_000_000_000);
        }
        let rate = SMP_RATES[cpu].load(Ordering::Acquire);
        if rate != 0 {
            let _ = writeln!(output, "cpu{cpu}      sha256 {rate:>8} MiB/s");
            total += rate;
        }
    }
    let _ = writeln!(output, "total     sha256 {total:>8} MiB/s");
}
