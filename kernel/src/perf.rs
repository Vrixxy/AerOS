//! Boot-test timing of the pieces behind the slow benchmark rows, so a
//! regression shows up in the log and the cost can be attributed without a
//! profiler. Printed as `AEROS_PERF name=... ns_per_op=...`; informational.

use crate::vfs;

fn time(name: &str, iterations: u32, mut body: impl FnMut()) {
    // One untimed round to warm caches.
    body();
    let start = crate::time::monotonic_nanoseconds();
    for _ in 0..iterations {
        body();
    }
    let total = crate::time::monotonic_nanoseconds().saturating_sub(start);
    crate::serial::format(format_args!(
        "AEROS_PERF name={name} ops={iterations} ns_per_op={}\n",
        total / u64::from(iterations)
    ));
}

pub fn run() {
    let data = [0x5au8; 4096];
    let path = "/tmp/perf.bin";
    time("vfs_create_write_close_4k", 200, || {
        if let Ok(handle) = vfs::open_file(path, true, false, true, 0o644, true) {
            let _ = vfs::write(handle, &data, false);
            let _ = vfs::close(handle);
        }
    });
    time("vfs_open_read_close_4k", 200, || {
        if let Ok(handle) = vfs::open_file(path, false, false, false, 0, false) {
            let mut buffer = [0u8; 4096];
            let _ = vfs::read(handle, &mut buffer);
            let _ = vfs::close(handle);
        }
    });
    time("vfs_open_raw_read_close_4k", 200, || {
        if let Ok(handle) = vfs::open_file_raw(path) {
            let mut buffer = [0u8; 4096];
            let _ = vfs::read(handle, &mut buffer);
            let _ = vfs::close(handle);
        }
    });
    time("vfs_remove_create", 200, || {
        let _ = vfs::remove(path, false);
        if let Ok(handle) = vfs::open_file(path, true, false, true, 0o644, true) {
            let _ = vfs::close(handle);
        }
    });
    time("antivirus_scan_4k", 200, || {
        let _ = crate::antivirus::scan_bytes(&data);
    });
    time("sha256_4k", 200, || {
        let _ = crate::auth::sha256(&data);
    });
    time("save_restore_process_state", 2000, || {
        let state = crate::syscall::save_process_state();
        crate::syscall::restore_process_state(&state);
    });
    time("monotonic_clock", 100_000, || {
        let _ = crate::time::monotonic_nanoseconds();
    });
    let _ = vfs::remove(path, false);
}
