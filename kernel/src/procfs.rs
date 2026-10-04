//! `/proc`: a read-only view generated on demand. There is no separate mount;
//! the VFS rewrites `/proc/...` to a hidden directory under `/tmp` and calls
//! `refresh` first, which writes the requested file (or, for a directory, all
//! of its files) fresh from kernel state. Two readers of the same file at the
//! same moment share one copy, so a read can see content written for the
//! other. `/proc/self` and the current process's own id map to the same
//! directory; other processes are not exposed.

use core::fmt::Write;

use crate::vfs;

const BACKING: &str = "/tmp/.proc";
const BACKING_SYS: &str = "/tmp/.sys";
const MAX_PATH: usize = 160;
const TEXT_BYTES: usize = 4096;

pub struct Mapped {
    bytes: [u8; MAX_PATH],
    length: usize,
}

impl Mapped {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or(BACKING)
    }
}

struct Text {
    bytes: [u8; TEXT_BYTES],
    length: usize,
}

impl Text {
    fn new() -> Self {
        Self {
            bytes: [0; TEXT_BYTES],
            length: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Write for Text {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let count = text.len().min(TEXT_BYTES - self.length);
        self.bytes[self.length..self.length + count].copy_from_slice(&text.as_bytes()[..count]);
        self.length += count;
        Ok(())
    }
}

fn current_pid() -> u64 {
    crate::scheduler::current_task_pid_for_linux().unwrap_or_else(crate::scheduler::current_task_id)
}

/// Paths served by this module: `/proc` and the small `/sys`.
pub fn is_virtual_path(path: &str) -> bool {
    ["/proc", "/sys"].iter().any(|root| {
        path.strip_prefix(root)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

/// The backing path for a `/proc` path, with the files it names refreshed.
/// `None` for any path outside `/proc`.
pub fn resolve(path: &str) -> Option<Mapped> {
    let (rest, is_proc) = match (path.strip_prefix("/proc"), path.strip_prefix("/sys")) {
        (Some(""), _) => ("", true),
        (Some(rest), _) if rest.starts_with('/') => (rest, true),
        (_, Some("")) => ("", false),
        (_, Some(rest)) if rest.starts_with('/') => (rest, false),
        _ => return None,
    };
    let mut mapped = Mapped {
        bytes: [0; MAX_PATH],
        length: 0,
    };
    let mut push = |text: &str| {
        let count = text.len().min(MAX_PATH - mapped.length);
        mapped.bytes[mapped.length..mapped.length + count]
            .copy_from_slice(&text.as_bytes()[..count]);
        mapped.length += count;
    };
    push(if is_proc { BACKING } else { BACKING_SYS });
    let mut components = rest.split('/').filter(|part| !part.is_empty());
    let mut relative = [0u8; MAX_PATH];
    let mut relative_length = 0usize;
    let pid = current_pid();
    let mut first = true;
    for component in components.by_ref() {
        if component == "." || component == ".." {
            return Some(Mapped {
                bytes: [0; MAX_PATH],
                length: 0,
            });
        }
        let is_self = is_proc
            && first
            && (component == "self" || component.parse::<u64>().is_ok_and(|number| number == pid));
        first = false;
        let part = if is_self { "self" } else { component };
        for text in ["/", part] {
            let count = text.len().min(MAX_PATH - relative_length);
            relative[relative_length..relative_length + count]
                .copy_from_slice(&text.as_bytes()[..count]);
            relative_length += count;
        }
    }
    let relative = core::str::from_utf8(&relative[..relative_length]).unwrap_or("");
    push(relative);
    if is_proc {
        refresh(relative);
    } else {
        refresh_sys(relative);
    }
    Some(mapped)
}

fn ensure_directory(path: &str) {
    let _ = vfs::create_directory(path, 0o755);
}

fn write_file(backing: &str, relative: &str, content: &[u8]) {
    let mut path = Text::new();
    let _ = write!(path, "{backing}{relative}");
    let Ok(path) = core::str::from_utf8(path.as_bytes()) else {
        return;
    };
    let Ok(handle) = vfs::open_file(path, true, false, true, 0o644, true) else {
        return;
    };
    let _ = vfs::write(handle, content, false);
    let _ = vfs::close(handle);
}

const FILES: [&str; 14] = [
    "version",
    "uptime",
    "meminfo",
    "cpuinfo",
    "loadavg",
    "stat",
    "mounts",
    "filesystems",
    "diskstats",
    "vmstat",
    "schedstat",
    "slabinfo",
    "iommu",
    "self",
];
const SELF_FILES: [&str; 7] = [
    "status", "stat", "statm", "comm", "cmdline", "environ", "maps",
];
const SYS_FILES: [&str; 6] = [
    "sys/kernel/ostype",
    "sys/kernel/osrelease",
    "sys/kernel/hostname",
    "sys/kernel/pid_max",
    "sys/vm/overcommit_memory",
    "sys/vm/swappiness",
];

fn refresh(relative: &str) {
    ensure_directory(BACKING);
    match relative {
        "" => {
            for name in FILES {
                refresh_one(name);
            }
            refresh("/sys");
        }
        "/self" => {
            for name in SELF_FILES {
                refresh_one(join("self/", name));
            }
        }
        "/sys" | "/sys/kernel" | "/sys/vm" => {
            let prefix = &relative[1..];
            for name in SYS_FILES {
                if name.starts_with(prefix) {
                    refresh_one(name);
                }
            }
        }
        other => refresh_one(other.trim_start_matches('/')),
    }
}

fn join(prefix: &str, name: &str) -> Text {
    let mut text = Text::new();
    let _ = write!(text, "{prefix}{name}");
    text
}

impl AsRef<str> for Text {
    fn as_ref(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
}

fn refresh_one(name: impl AsRef<str>) {
    refresh_backed(BACKING, name.as_ref());
}

const SYSFS_FILES: [&str; 5] = [
    "devices/system/cpu/online",
    "devices/system/cpu/possible",
    "devices/system/cpu/present",
    "devices/system/cpu/kernel_max",
    "kernel/mm/transparent_hugepage/enabled",
];

fn refresh_sys(relative: &str) {
    ensure_directory(BACKING_SYS);
    let prefix = relative.trim_matches('/');
    if SYSFS_FILES.contains(&prefix) {
        refresh_backed(BACKING_SYS, prefix);
        return;
    }
    for name in SYSFS_FILES {
        if name.starts_with(prefix) {
            refresh_backed(BACKING_SYS, name);
        }
    }
}

fn refresh_backed(backing: &str, name: &str) {
    if name == "self" {
        refresh("/self");
        return;
    }
    let mut content = Text::new();
    if !render(name, &mut content) {
        return;
    }
    let mut directory = Text::new();
    let _ = write!(directory, "{backing}");
    let mut parts = name.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_some() {
            let _ = write!(directory, "/{part}");
            if let Ok(path) = core::str::from_utf8(directory.as_bytes()) {
                ensure_directory(path);
            }
        }
    }
    let mut relative = Text::new();
    let _ = write!(relative, "/{name}");
    write_file(backing, relative.as_ref(), content.as_bytes());
}

fn render(name: &str, out: &mut Text) -> bool {
    match name {
        "version" => {
            let _ = writeln!(out, "Linux version 6.8.0-aeros (aeros@aeros) #1 AerOS SMP");
        }
        "uptime" => {
            let nanoseconds = crate::time::monotonic_nanoseconds();
            let _ = writeln!(
                out,
                "{}.{:02} 0.00",
                nanoseconds / 1_000_000_000,
                nanoseconds / 10_000_000 % 100
            );
        }
        "meminfo" => {
            let snapshot = crate::sysmon::snapshot();
            let total = snapshot.memory_total / 1024;
            let free = snapshot.memory_free / 1024;
            let _ = write!(
                out,
                "MemTotal:       {total:>8} kB\nMemFree:        {free:>8} kB\nMemAvailable:   {free:>8} kB\n\
                 Buffers:        {:>8} kB\nCached:         {:>8} kB\nSwapCached:     {:>8} kB\n\
                 SwapTotal:      {:>8} kB\nSwapFree:       {:>8} kB\n",
                0,
                crate::block::resident_pages() * 4,
                0,
                crate::swap::stats().slots as u64 * 4,
                (crate::swap::stats().slots - crate::swap::stats().in_use) as u64 * 4
            );
        }
        "slabinfo" => {
            let _ = writeln!(
                out,
                "slabinfo - version: 2.1\n# name <active_objs> <objsize> <slabs> <allocations>"
            );
            for class in 0..crate::slab::class_count() {
                if let Some(stats) = crate::slab::class_stats(class) {
                    let _ = writeln!(
                        out,
                        "kmalloc-{} {} {} {} {}",
                        stats.size, stats.in_use, stats.size, stats.slabs, stats.allocations
                    );
                }
            }
        }
        "iommu" => crate::iommu::describe(out),
        "schedstat" => {
            let _ = writeln!(
                out,
                "version 15\ntimestamp {}",
                crate::time::monotonic_nanoseconds() / 10_000_000
            );
            let online = crate::smp::online_mask();
            for cpu in 0..crate::acpi::MAX_PROCESSORS {
                if online & (1 << cpu) != 0 || cpu == 0 {
                    let _ = writeln!(out, "cpu{cpu} {}", crate::smpsched::jobs_executed(cpu));
                }
            }
            let _ = writeln!(out, "steals {}", crate::smpsched::steals());
        }
        "vmstat" => {
            let (resident, readaheads, reclaimed) = crate::block::cache_counters();
            let (mut hits, mut misses) = (0, 0);
            for index in 0..9 {
                let stats = crate::block::stats(index);
                hits += stats.cache_hits;
                misses += stats.cache_misses;
            }
            let _ = write!(
                out,
                "nr_page_cache {resident}\npgcache_hit {hits}\npgcache_miss {misses}\npgreadahead {readaheads}\npgreclaim {reclaimed}\noom_kill {}\n",
                crate::oom::kills()
            );
        }
        "diskstats" => {
            for index in 0..9 {
                let stats = crate::block::stats(index);
                if stats.reads + stats.writes != 0 {
                    let _ = writeln!(
                        out,
                        "{:>4} {:>3} {} {} {} {} 0 {} {} {} 0 0 0 0",
                        8,
                        index * 16,
                        crate::block::Disk::name(index),
                        stats.reads,
                        stats.merges,
                        stats.read_sectors,
                        stats.writes,
                        0,
                        stats.write_sectors
                    );
                }
            }
        }
        "cpuinfo" => {
            let cpu = crate::arch::CpuInfo::detect();
            let count = crate::sysmon::snapshot().cpu_count.max(1);
            for index in 0..count {
                let _ = write!(
                    out,
                    "processor\t: {index}\nvendor_id\t: {}\ncpu family\t: 6\nmodel name\t: {} processor\nflags\t\t:",
                    cpu.vendor(),
                    cpu.vendor()
                );
                for (present, flag) in [
                    (cpu.fxsave, "fxsr"),
                    (cpu.sse, "sse"),
                    (cpu.nx, "nx"),
                    (cpu.syscall, "syscall"),
                    (cpu.one_gib_pages, "pdpe1gb"),
                    (cpu.x2apic, "x2apic"),
                    (cpu.smep, "smep"),
                    (cpu.smap, "smap"),
                    (cpu.xsave, "xsave"),
                    (cpu.avx, "avx"),
                    (cpu.rdrand, "rdrand"),
                    (cpu.rdseed, "rdseed"),
                    (cpu.invariant_tsc, "constant_tsc"),
                ] {
                    if present {
                        let _ = write!(out, " {flag}");
                    }
                }
                let _ = write!(out, "\n\n");
            }
        }
        "loadavg" => {
            let snapshot = crate::sysmon::snapshot();
            let load = snapshot.cpu_permille as usize * snapshot.cpu_count.max(1) / 10;
            let mut tasks = 1usize;
            crate::scheduler::list_tasks(|_| tasks += 1);
            let _ = writeln!(
                out,
                "{}.{:02} {}.{:02} {}.{:02} 1/{tasks} {}",
                load / 100,
                load % 100,
                load / 100,
                load % 100,
                load / 100,
                load % 100,
                current_pid()
            );
        }
        "stat" => {
            let snapshot = crate::sysmon::snapshot();
            let mut tasks = 1usize;
            crate::scheduler::list_tasks(|_| tasks += 1);
            let boot = crate::rtc::unix_seconds().saturating_sub(snapshot.uptime_secs);
            let _ = writeln!(out, "cpu  0 0 0 0 0 0 0 0 0 0");
            for index in 0..snapshot.cpu_count.max(1) {
                let _ = writeln!(out, "cpu{index} 0 0 0 0 0 0 0 0 0 0");
            }
            let _ = write!(
                out,
                "intr 0\nctxt 0\nbtime {boot}\nprocesses {tasks}\nprocs_running 1\nprocs_blocked 0\n"
            );
        }
        "mounts" => {
            crate::mounts::procfs_text(out);
        }
        "filesystems" => {
            let _ = write!(
                out,
                "nodev\trootfs\nnodev\ttmpfs\nnodev\tproc\nnodev\tsysfs\nnodev\tbind\n\tvfat\n"
            );
        }
        "devices/system/cpu/online"
        | "devices/system/cpu/possible"
        | "devices/system/cpu/present" => {
            let count = crate::sysmon::snapshot().cpu_count.max(1);
            if count == 1 {
                let _ = writeln!(out, "0");
            } else {
                let _ = writeln!(out, "0-{}", count - 1);
            }
        }
        "devices/system/cpu/kernel_max" => {
            let _ = writeln!(out, "511");
        }
        "kernel/mm/transparent_hugepage/enabled" => {
            let _ = writeln!(out, "always madvise [never]");
        }
        "sys/kernel/ostype" => {
            let _ = writeln!(out, "Linux");
        }
        "sys/kernel/osrelease" => {
            let _ = writeln!(out, "6.8.0-aeros");
        }
        "sys/kernel/hostname" => {
            let _ = writeln!(out, "aeros");
        }
        "sys/kernel/pid_max" => {
            let _ = writeln!(out, "32768");
        }
        "sys/vm/overcommit_memory" => {
            let _ = writeln!(out, "0");
        }
        "sys/vm/swappiness" => {
            let _ = writeln!(out, "0");
        }
        "self/comm" => {
            let _ = writeln!(out, "aeros");
        }
        "self/cmdline" | "self/environ" => {}
        "self/maps" => {
            if let Some(space) = crate::scheduler::current_process_space() {
                space.regions(&mut |start, end, permissions, name| {
                    let permissions = core::str::from_utf8(&permissions).unwrap_or("---p");
                    let _ = writeln!(
                        out,
                        "{start:08x}-{end:08x} {permissions} 00000000 00:00 0 {name}"
                    );
                });
            }
        }
        "self/statm" => {
            let mut pages = 0u64;
            let resident = crate::scheduler::current_process_space().map_or(0, |space| {
                space.regions(&mut |start, end, _, _| pages += (end - start) / 4096);
                space.resident_pages()
            });
            let _ = writeln!(out, "{pages} {resident} 0 0 0 0 0");
        }
        "self/status" => render_status(out),
        "self/stat" => render_stat(out),
        _ => return false,
    }
    true
}

struct Own {
    pid: u64,
    parent: u64,
    group: u64,
    session: u64,
    resident_kib: u64,
    nice: i8,
}

fn own() -> Own {
    let pid = current_pid();
    let mut details = Own {
        pid,
        parent: crate::scheduler::current_parent_pid_for_linux().unwrap_or(0),
        group: pid,
        session: pid,
        resident_kib: 0,
        nice: 0,
    };
    crate::scheduler::list_tasks(|task| {
        if task.id == pid {
            details.group = task.pgid;
            details.session = task.sid;
            details.resident_kib = task.resident_pages * 4;
            details.nice = task.nice;
        }
    });
    details
}

fn render_status(out: &mut Text) {
    let own = own();
    let capabilities = crate::syscall::proc_capabilities();
    let seccomp = crate::syscall::proc_seccomp_mode();
    let _ = write!(
        out,
        "Name:\taeros\nState:\tR (running)\nTgid:\t{pid}\nPid:\t{pid}\nPPid:\t{parent}\n\
         Uid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\nThreads:\t1\nVmRSS:\t{rss:>8} kB\n\
         CapPrm:\t{permitted:016x}\nCapEff:\t{effective:016x}\nSeccomp:\t{seccomp}\n",
        pid = own.pid,
        parent = own.parent,
        rss = own.resident_kib,
        permitted = capabilities.permitted,
        effective = capabilities.effective,
    );
}

fn render_stat(out: &mut Text) {
    let own = own();
    let _ = write!(
        out,
        "{pid} (aeros) R {parent} {group} {session} 0 -1 0 0 0 0 0 0 0 0 0 {priority} {nice} 1 0 0 {vsize} {rss}",
        pid = own.pid,
        parent = own.parent,
        group = own.group,
        session = own.session,
        priority = 20 + own.nice as i32,
        nice = own.nice,
        vsize = own.resident_kib * 1024,
        rss = own.resident_kib / 4,
    );
    for _ in 25..=52 {
        let _ = write!(out, " 0");
    }
    let _ = writeln!(out);
}

pub fn self_test() -> bool {
    let read_text = |path: &str| -> Option<Text> {
        let handle = vfs::open_file(path, false, false, false, 0, false).ok()?;
        let mut text = Text::new();
        let count = vfs::read(handle, &mut text.bytes).ok()?;
        text.length = count;
        let _ = vfs::close(handle);
        Some(text)
    };
    let contains = |text: &Option<Text>, needle: &str| {
        text.as_ref().is_some_and(|text| {
            core::str::from_utf8(text.as_bytes()).is_ok_and(|text| text.contains(needle))
        })
    };

    let meminfo = read_text("/proc/meminfo");
    let meminfo_ok = contains(&meminfo, "MemTotal:") && contains(&meminfo, "MemFree:");
    let version_ok = contains(&read_text("/proc/version"), "Linux version 6.8.0-aeros");
    let uptime = read_text("/proc/uptime");
    let uptime_ok = uptime.as_ref().is_some_and(|text| {
        core::str::from_utf8(text.as_bytes()).is_ok_and(|text| {
            let mut fields = text.split_whitespace();
            fields.next().is_some_and(|first| first.contains('.')) && fields.next().is_some()
        })
    });
    let cpuinfo_ok = contains(&read_text("/proc/cpuinfo"), "processor\t: 0")
        && contains(&read_text("/proc/cpuinfo"), "vendor_id");
    let status = read_text("/proc/self/status");
    let own_pid = current_pid();
    let mut pid_line = Text::new();
    let _ = writeln!(pid_line, "Pid:\t{own_pid}");
    let status_ok = contains(&status, pid_line.as_ref())
        && contains(&status, "CapEff:\t000001ffffffffff")
        && contains(&status, "Seccomp:\t0");
    let mut numeric = Text::new();
    let _ = write!(numeric, "/proc/{own_pid}/stat");
    let stat_ok = contains(&read_text(numeric.as_ref()), "(aeros) R ");
    let mounts_ok = contains(&read_text("/proc/mounts"), "tmpfs /tmp tmpfs");
    let cpu_online = read_text("/sys/devices/system/cpu/online");
    let sys_ok = contains(&read_text("/proc/sys/kernel/osrelease"), "6.8.0-aeros")
        && cpu_online.as_ref().is_some_and(|text| {
            core::str::from_utf8(text.as_bytes()).is_ok_and(|text| text.starts_with('0'))
        })
        && contains(
            &read_text("/sys/kernel/mm/transparent_hugepage/enabled"),
            "[never]",
        )
        && vfs::metadata("/sys/devices/system/cpu")
            .is_ok_and(|metadata| metadata.mode & 0o170000 == 0o040000)
        && matches!(
            vfs::metadata("/sys/no-such-file"),
            Err(vfs::VfsError::NotFound)
        )
        && matches!(
            vfs::open_file(
                "/sys/devices/system/cpu/online",
                false,
                false,
                false,
                0,
                true
            ),
            Err(vfs::VfsError::PermissionDenied)
        );

    let metadata = vfs::metadata("/proc/meminfo");
    let metadata_ok = metadata
        .is_ok_and(|metadata| metadata.mode & 0o170000 == 0o100000 && metadata.size > 0)
        && vfs::metadata("/proc").is_ok_and(|metadata| metadata.mode & 0o170000 == 0o040000)
        && vfs::metadata("/proc/sys/kernel")
            .is_ok_and(|metadata| metadata.mode & 0o170000 == 0o040000);

    let missing = matches!(
        vfs::metadata("/proc/no-such-file"),
        Err(vfs::VfsError::NotFound)
    ) && matches!(
        vfs::metadata("/proc/4242424/status"),
        Err(vfs::VfsError::NotFound)
    ) && vfs::metadata("/procfoo").is_err();
    let read_only = matches!(
        vfs::open_file("/proc/meminfo", false, false, false, 0, true),
        Err(vfs::VfsError::PermissionDenied)
    ) && matches!(
        vfs::open_file("/proc/new", true, false, false, 0o644, true),
        Err(vfs::VfsError::PermissionDenied)
    );
    let escapes = vfs::metadata("/proc/../etc").is_err();

    let mut listed = [false; 3];
    if let Ok(handle) = vfs::open_directory("/proc") {
        while let Ok(Some(entry)) = vfs::next_directory_entry(handle) {
            let name = &entry.name[..entry.name_len as usize];
            for (slot, wanted) in listed.iter_mut().zip([&b"meminfo"[..], b"self", b"sys"]) {
                *slot |= name == wanted;
            }
        }
        let _ = vfs::close(handle);
    }
    let mut sys_listed = [false; 2];
    if let Ok(handle) = vfs::open_directory("/sys") {
        while let Ok(Some(entry)) = vfs::next_directory_entry(handle) {
            let name = &entry.name[..entry.name_len as usize];
            for (slot, wanted) in sys_listed.iter_mut().zip([&b"devices"[..], b"kernel"]) {
                *slot |= name == wanted;
            }
        }
        let _ = vfs::close(handle);
    }
    let listing_ok = listed.iter().all(|found| *found) && sys_listed.iter().all(|found| *found);

    meminfo_ok
        && version_ok
        && uptime_ok
        && cpuinfo_ok
        && status_ok
        && stat_ok
        && mounts_ok
        && sys_ok
        && metadata_ok
        && missing
        && read_only
        && escapes
        && listing_ok
}
