//! Deterministic mutation fuzzing of the kernel's untrusted-input parsers.
//! Every input is derived from a fixed seed, so a failure is reproducible; the
//! check is that nothing panics or hangs, and a panic in the kernel halts the
//! boot, which fails the boot-test.

use crate::sync::TicketLock;

const PNG_CORPUS: &[u8] = include_bytes!("../../assets/test/png-corpus.bin");
const JPEG_SEED: &[u8] = include_bytes!("../../assets/test/photo-small.jpg");
const ELF_SEEDS: [&[u8]; 2] = [
    include_bytes!("../../assets/userspace/aeros-init"),
    include_bytes!("../../assets/userspace/aeros-fork-probe"),
];

const SCRATCH_BYTES: usize = 40 * 1024;
static SCRATCH: TicketLock<[u8; SCRATCH_BYTES]> = TicketLock::new([0; SCRATCH_BYTES]);

pub struct FuzzReport {
    pub iterations: u32,
    pub elapsed_ms: u64,
    pub verified: bool,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }
}

/// Copies `seed` into `buffer`, corrupts a few bytes (mostly near the start,
/// where headers live) and sometimes truncates it. Returns the new length.
fn mutate(rng: &mut Rng, seed: &[u8], buffer: &mut [u8]) -> usize {
    let length = seed.len().min(buffer.len());
    buffer[..length].copy_from_slice(&seed[..length]);
    for _ in 0..1 + rng.below(6) {
        let window = if rng.below(10) < 7 {
            length.min(128)
        } else {
            length
        };
        let at = rng.below(window);
        buffer[at] = match rng.below(4) {
            0 => 0,
            1 => 0xff,
            2 => buffer[at] ^ (1 << rng.below(8)),
            _ => rng.next() as u8,
        };
    }
    if rng.below(10) < 3 {
        rng.below(length + 1)
    } else {
        length
    }
}

fn png_cases() -> impl Iterator<Item = &'static [u8]> {
    let mut at = 0usize;
    core::iter::from_fn(move || {
        if at + 8 > PNG_CORPUS.len() {
            return None;
        }
        let length = u32::from_le_bytes([
            PNG_CORPUS[at],
            PNG_CORPUS[at + 1],
            PNG_CORPUS[at + 2],
            PNG_CORPUS[at + 3],
        ]) as usize;
        let file = PNG_CORPUS.get(at + 8..at + 8 + length)?;
        at += 8 + length;
        Some(file)
    })
}

pub fn run() -> FuzzReport {
    let started = crate::time::monotonic_nanoseconds();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut iterations = 0u32;
    let mut scratch = SCRATCH.lock();

    for _ in 0..30_000 {
        let seed = ELF_SEEDS[rng.below(ELF_SEEDS.len())];
        let length = mutate(&mut rng, seed, &mut scratch[..]);
        let _ = crate::elf::ElfImage::parse(&scratch[..length]);
        let _ = crate::compat::inspect(&scratch[..length]);
        iterations += 1;
    }

    let mut cases = [&[][..]; 64];
    let mut case_count = 0;
    for case in png_cases() {
        if case_count < cases.len() {
            cases[case_count] = case;
            case_count += 1;
        }
    }
    for _ in 0..3000 {
        if case_count == 0 {
            break;
        }
        let seed = cases[rng.below(case_count)];
        let length = mutate(&mut rng, seed, &mut scratch[..]);
        let _ = crate::image::decode(&scratch[..length]);
        iterations += 1;
    }

    for _ in 0..100 {
        let length = mutate(&mut rng, JPEG_SEED, &mut scratch[..]);
        let _ = crate::image::decode(&scratch[..length]);
        iterations += 1;
    }

    for _ in 0..20_000 {
        let length = 1 + rng.below(512);
        for byte in scratch[..length].iter_mut() {
            *byte = rng.next() as u8;
        }
        let _ = crate::antivirus::scan_bytes(&scratch[..length]);
        let _ = crate::image::decode(&scratch[..length]);
        iterations += 1;
    }

    for _ in 0..80_000 {
        let length = rng.below(160);
        for byte in scratch[..length].iter_mut() {
            *byte = rng.next() as u8;
        }
        if length >= 38 && rng.below(4) != 0 {
            scratch[12] = 0x08;
            scratch[13] = 0x00;
            scratch[14] = 0x40 | (rng.next() as u8 & 0x0f);
            scratch[23] = [6, 17, 1][rng.below(3)];
        }
        let _ = crate::firewall::blocks(&scratch[..length]);
        iterations += 1;
    }

    for _ in 0..5000 {
        let mut program = [crate::seccomp::Instruction::EMPTY; crate::seccomp::MAX_INSTRUCTIONS];
        let length = 1 + rng.below(crate::seccomp::MAX_INSTRUCTIONS);
        for slot in program[..length].iter_mut() {
            let code = [
                0x00u16, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x15, 0x20, 0x25, 0x35, 0x45,
                0x54, 0x61, 0x64, 0x74, 0x94, 0xa4, 0x16, 0x87, 0x07, 0x80,
            ][rng.below(23)];
            *slot = crate::seccomp::Instruction {
                code: code
                    | if rng.below(4) == 0 {
                        rng.next() as u16 & 0xf8
                    } else {
                        0
                    },
                jt: rng.below(6) as u8,
                jf: rng.below(6) as u8,
                k: if rng.below(3) == 0 {
                    rng.next() as u32
                } else {
                    rng.below(70) as u32
                },
            };
        }
        if rng.below(2) == 0 {
            program[length - 1].code = 0x06;
        }
        if let Some(filter) = crate::seccomp::Filter::new(&program[..length]) {
            let data = crate::seccomp::data_bytes(
                rng.next() as u32,
                rng.next(),
                &[
                    rng.next(),
                    rng.next(),
                    rng.next(),
                    rng.next(),
                    rng.next(),
                    rng.next(),
                ],
            );
            let _ = crate::seccomp::run(&filter, &data);
        }
        iterations += 1;
    }

    const LINE_ALPHABET: &[u8] = b"abcXYZ019 /._-~'\"\\|<>$*?;&=\t#";
    let mut line = [0u8; 96];
    for _ in 0..30_000 {
        let length = rng.below(line.len());
        for byte in line[..length].iter_mut() {
            *byte = LINE_ALPHABET[rng.below(LINE_ALPHABET.len())];
        }
        if let Ok(text) = core::str::from_utf8(&line[..length]) {
            let _ = crate::shell::fuzz_line(text);
            let _ = crate::vfs::metadata(text);
            let _ = crate::vfs::symlink_metadata(text);
        }
        iterations += 1;
    }

    for _ in 0..40 {
        let mut key = [0u8; 32];
        let mut signature = [0u8; 64];
        for byte in key.iter_mut().chain(signature.iter_mut()) {
            *byte = rng.next() as u8;
        }
        let length = rng.below(300);
        for byte in scratch[..length].iter_mut() {
            *byte = rng.next() as u8;
        }
        let _ = crate::ed25519::verify(&key, &scratch[..length], &signature);
        iterations += 1;
    }

    use crate::pkg_vectors as vectors;
    let package_seeds: [&[u8]; 5] = [
        vectors::V1,
        vectors::V2,
        vectors::V3,
        vectors::BAD_HASH,
        vectors::TRAVERSAL,
    ];
    for _ in 0..6_000 {
        let seed = package_seeds[rng.below(package_seeds.len())];
        let length = mutate(&mut rng, seed, &mut scratch[..]);
        crate::pkg::fuzz_header(&scratch[..length]);
        iterations += 1;
    }

    let mut proc_ok = true;
    let alphabet = b"/proc/self/sys/kernel/vm.0123456789meminfo-";
    let mut path = [0u8; 48];
    for _ in 0..1_500 {
        let length = 1 + rng.below(path.len() - 5);
        let start = if rng.below(2) == 0 { 5 } else { 0 };
        path[..5].copy_from_slice(if rng.below(4) == 0 {
            b"/sys/"
        } else {
            b"/proc"
        });
        for byte in path[start..start + length].iter_mut() {
            *byte = alphabet[rng.below(alphabet.len())];
        }
        let text = core::str::from_utf8(&path[..start + length]).unwrap_or("/");
        let mapped = crate::procfs::resolve(text);
        match mapped {
            Some(mapped) => {
                let target = mapped.as_str();
                proc_ok &= crate::procfs::is_virtual_path(text)
                    && (target.is_empty()
                        || (target.starts_with("/tmp/.proc") || target.starts_with("/tmp/.sys"))
                            && !target.split('/').any(|part| part == ".."));
            }
            None => proc_ok &= !crate::procfs::is_virtual_path(text),
        }
        iterations += 1;
    }

    crate::net::fuzz_parse_tcp(|| rng.next(), 8_000);
    iterations += 8_000;

    iterations += crate::syscall::fuzz_syscalls(&mut || rng.next(), 30_000);

    iterations += fuzz_fatfs(&mut rng, 150);
    iterations += fuzz_boot_fat(&mut rng, 150);
    let cyclic_ok = cyclic_directory_terminates();
    FuzzReport {
        iterations,
        elapsed_ms: (crate::time::monotonic_nanoseconds() - started) / 1_000_000,
        verified: cyclic_ok && proc_ok,
    }
}

/// Builds a small FAT16 volume on the RAM disk, flips random bytes in its
/// metadata (boot sector, FATs, root directory, first clusters), then mounts,
/// lists, reads, truncates, removes and checks it. A hostile or damaged disk
/// may produce errors but must never panic or hang the kernel.
fn fuzz_fatfs(rng: &mut Rng, rounds: u32) -> u32 {
    use crate::fatfs::{Disk, Fs};
    let mut done = 0;
    for _ in 0..rounds {
        if Fs::format(Disk::Ram, 0, crate::blockdev::RAM_DISK_SECTORS, b"FUZZVOL").is_err() {
            break;
        }
        if let Ok(mut fs) = Fs::mount(Disk::Ram, 0) {
            let root = fs.root().as_dir();
            let data = [0x5au8; 3000];
            if let Ok(mut node) = fs.create_file(root, b"A.BIN") {
                let _ = fs.write_at(&mut node, 0, &data);
            }
            if let Ok(directory) = fs.create_dir(root, b"SUB") {
                for name in [&b"Long file name one.txt"[..], b"B.BIN", b"C.BIN"] {
                    if let Ok(mut node) = fs.create_file(directory.as_dir(), name) {
                        let _ = fs.write_at(&mut node, 0, &data[..1500]);
                    }
                }
            }
        }
        for _ in 0..1 + rng.below(12) {
            let lba = rng.below(130) as u64;
            let mut sector = [0u8; 512];
            if crate::blockdev::ram_read(lba, &mut sector) {
                sector[rng.below(512)] = rng.next() as u8;
                let _ = crate::blockdev::ram_write(lba, &sector);
            }
        }
        if let Ok(mut fs) = Fs::mount(Disk::Ram, 0) {
            let root = fs.root().as_dir();
            let mut cursor = 0;
            for _ in 0..64 {
                let Ok(Some(listed)) = fs.list_next(root, &mut cursor) else {
                    break;
                };
                let mut node = listed.node;
                let mut buffer = [0u8; 600];
                if node.is_directory() {
                    let mut inner = 0;
                    for _ in 0..32 {
                        let Ok(Some(_)) = fs.list_next(node.as_dir(), &mut inner) else {
                            break;
                        };
                    }
                } else {
                    let _ = fs.read_at(&mut node, 0, &mut buffer);
                    let _ = fs.truncate(&mut node, 700);
                }
            }
            if let Ok(node) = fs.find(root, b"A.BIN") {
                let _ = fs.remove(&node);
            }
            let _ = fs.create_file(root, b"NEW.TXT");
            let _ = fs.fsck(false);
            let _ = fs.fsck(true);
            let _ = fs.fsck(false);
        }
        done += 1;
    }
    done
}

/// A directory whose only cluster has no end marker and whose FAT entry points
/// back at itself would list forever without a bound. Builds that damage and
/// requires listing, creating a file and checking the volume to all finish.
fn cyclic_directory_terminates() -> bool {
    use crate::fatfs::{Disk, Fs};
    if Fs::format(Disk::Ram, 0, crate::blockdev::RAM_DISK_SECTORS, b"CYCLEVOL").is_err() {
        return false;
    }
    let first_cluster = {
        let Ok(mut fs) = Fs::mount(Disk::Ram, 0) else {
            return false;
        };
        let root = fs.root().as_dir();
        let Ok(directory) = fs.create_dir(root, b"LOOP") else {
            return false;
        };
        directory.as_dir()
    };
    let mut sector = [0u8; 512];
    let mut found = None;
    for lba in 0..400u64 {
        if crate::blockdev::ram_read(lba, &mut sector) && sector[0] == b'.' && sector[1] == b' ' {
            found = Some(lba);
            break;
        }
    }
    let Some(directory_lba) = found else {
        return false;
    };
    for entry in sector.chunks_exact_mut(32) {
        entry.fill(0x41);
        entry[0] = 0xe5;
    }
    let looped = crate::blockdev::ram_write(directory_lba, &sector);
    let mut fat_sector = [0u8; 512];
    let fat_lba = 1 + u64::from(first_cluster) * 2 / 512;
    let at = (u64::from(first_cluster) * 2 % 512) as usize;
    let linked = crate::blockdev::ram_read(fat_lba, &mut fat_sector) && {
        fat_sector[at..at + 2].copy_from_slice(&(first_cluster as u16).to_le_bytes());
        crate::blockdev::ram_write(fat_lba, &fat_sector)
    };
    if !looped || !linked {
        return false;
    }
    let Ok(mut fs) = Fs::mount(Disk::Ram, 0) else {
        return false;
    };
    let mut cursor = 0;
    let mut listed = 0u32;
    while let Ok(Some(_)) = fs.list_next(first_cluster, &mut cursor) {
        listed += 1;
        if listed > 70_000 {
            return false;
        }
    }
    let created = fs.create_file(first_cluster, b"X.TXT");
    let _ = fs.fsck(false);
    created.is_err() || listed == 0
}

/// The boot-disk FAT code (`fat.rs`) on a formatted RAM volume whose metadata
/// has been randomly corrupted: mounting, listing, reading, writing, deleting
/// and chain checking must finish without panicking or hanging.
fn fuzz_boot_fat(rng: &mut Rng, rounds: u32) -> u32 {
    use crate::fat;
    let name = *b"FUZZ    BIN";
    let mut done = 0;
    crate::blockdev::use_ram_disk(true);
    for _ in 0..rounds {
        if !crate::fat_crash::format_ram_disk() {
            break;
        }
        let _ = fat::with_ram_volume(|| {
            let data = [0x33u8; 2500];
            let _ = fat::write_root_file(&name, &data);
            let _ = fat::write_root_file(b"OTHER   TXT", &data[..700]);
        });
        for _ in 0..1 + rng.below(10) {
            let lba = rng.below(110) as u64;
            let mut sector = [0u8; 512];
            if crate::blockdev::read_sector(lba, &mut sector) {
                sector[rng.below(512)] = rng.next() as u8;
                let _ = crate::blockdev::write_boot_sector(lba, &sector);
            }
        }
        let _ = fat::with_ram_volume(|| {
            let mut listing = [fat::FatFileEntry::EMPTY; 16];
            let _ = fat::list_root_files(&mut listing);
            let _ = fat::list_root_directories(&mut listing);
            let mut buffer = [0u8; 3000];
            let _ = fat::read_root_file(&name, &mut buffer);
            let _ = fat::root_file_size(&name);
            let _ = fat::write_root_file(&name, &buffer[..1800]);
            let _ = fat::create_root_directory(b"FUZZDIR    ");
            let _ = fat::delete_root_file(b"OTHER   TXT");
            if let Some(volume) = fat::Fat::mounted() {
                fat::reset_check_baseline();
                let _ = fat::check_root_chains(&volume, &name);
            }
        });
        done += 1;
    }
    crate::blockdev::use_ram_disk(false);
    done
}
