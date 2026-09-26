use crate::arch::paging::UserImageMapping;
use crate::elf::ElfImage;
use crate::sync::TicketLock;
use core::sync::atomic::{AtomicU64, Ordering};

const PAGE_SIZE: u64 = 4096;
const STACK_BYTES: usize = 8192;
const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_ENTRY: u64 = 9;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_PLATFORM: u64 = 15;
const AT_SECURE: u64 = 23;
const AT_RANDOM: u64 = 25;
const AT_EXECFN: u64 = 31;
const MAX_PROCESSES: usize = 32;
const MAX_PROCESS_NAME: usize = 48;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProcessState {
    Empty,
    Ready,
    Running,
    Zombie,
}

#[derive(Clone, Copy)]
struct ProcessSlot {
    pid: u64,
    parent: u64,
    generation: u16,
    state: ProcessState,
    status: u64,
    name: [u8; MAX_PROCESS_NAME],
    name_len: u8,
}

impl ProcessSlot {
    const EMPTY: Self = Self {
        pid: 0,
        parent: 0,
        generation: 1,
        state: ProcessState::Empty,
        status: 0,
        name: [0; MAX_PROCESS_NAME],
        name_len: 0,
    };
}

struct ProcessTable {
    slots: [ProcessSlot; MAX_PROCESSES],
    next_pid: u64,
    spawned: u64,
    reaped: u64,
    highest_pid: u64,
}

impl ProcessTable {
    const fn new() -> Self {
        Self {
            slots: [ProcessSlot::EMPTY; MAX_PROCESSES],
            next_pid: 1,
            spawned: 0,
            reaped: 0,
            highest_pid: 0,
        }
    }
}

static PROCESSES: TicketLock<ProcessTable> = TicketLock::new(ProcessTable::new());
static CURRENT_PID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct ProcessToken {
    slot: u8,
    generation: u16,
    pid: u64,
}

#[derive(Clone, Copy)]
pub struct ProcessTableStats {
    pub spawned: u64,
    pub reaped: u64,
    pub highest_pid: u64,
    pub ready: usize,
    pub running: usize,
    pub zombies: usize,
    pub verified: bool,
}

#[derive(Clone, Copy)]
pub struct ProcessStackReport {
    pub stack_pointer: u64,
    pub bytes_used: usize,
    pub argc: u64,
    pub aux_entries: usize,
    pub random_nonzero: bool,
    pub aligned: bool,
    pub verified: bool,
}

pub fn initialize() {
    *PROCESSES.lock() = ProcessTable::new();
    CURRENT_PID.store(0, Ordering::Release);
}

pub fn spawn(name: &str, parent: u64) -> Option<ProcessToken> {
    if name.is_empty() || name.len() > MAX_PROCESS_NAME {
        return None;
    }
    let mut processes = PROCESSES.lock();
    let slot = processes
        .slots
        .iter()
        .position(|process| process.state == ProcessState::Empty)?;
    let pid = processes.next_pid;
    processes.next_pid = processes.next_pid.checked_add(1)?;
    let generation = processes.slots[slot].generation.max(1);
    let mut stored = [0u8; MAX_PROCESS_NAME];
    stored[..name.len()].copy_from_slice(name.as_bytes());
    processes.slots[slot] = ProcessSlot {
        pid,
        parent,
        generation,
        state: ProcessState::Ready,
        status: 0,
        name: stored,
        name_len: name.len() as u8,
    };
    processes.spawned = processes.spawned.saturating_add(1);
    processes.highest_pid = processes.highest_pid.max(pid);
    Some(ProcessToken {
        slot: slot as u8,
        generation,
        pid,
    })
}

pub fn activate(token: &ProcessToken) -> bool {
    let mut processes = PROCESSES.lock();
    let process = &mut processes.slots[token.slot as usize];
    if process.pid != token.pid
        || process.generation != token.generation
        || process.state != ProcessState::Ready
    {
        return false;
    }
    process.state = ProcessState::Running;
    CURRENT_PID.store(token.pid, Ordering::Release);
    true
}

pub fn mark_exit(token: &ProcessToken, status: u64) -> bool {
    let mut processes = PROCESSES.lock();
    let process = &mut processes.slots[token.slot as usize];
    if process.pid != token.pid
        || process.generation != token.generation
        || process.state != ProcessState::Running
    {
        return false;
    }
    process.status = status;
    process.state = ProcessState::Zombie;
    CURRENT_PID.store(0, Ordering::Release);
    true
}

pub fn reap(token: &ProcessToken) -> bool {
    let mut processes = PROCESSES.lock();
    let process = &mut processes.slots[token.slot as usize];
    if process.pid != token.pid
        || process.generation != token.generation
        || process.state != ProcessState::Zombie
    {
        return false;
    }
    let generation = process.generation.wrapping_add(1).max(1);
    *process = ProcessSlot {
        generation,
        ..ProcessSlot::EMPTY
    };
    processes.reaped = processes.reaped.saturating_add(1);
    true
}

pub fn current_pid() -> u64 {
    CURRENT_PID.load(Ordering::Acquire)
}

pub fn current_parent() -> u64 {
    let pid = current_pid();
    PROCESSES
        .lock()
        .slots
        .iter()
        .find(|process| process.pid == pid && process.state == ProcessState::Running)
        .map(|process| process.parent)
        .unwrap_or(0)
}

pub fn request_exit(pid: u64, status: u64) -> bool {
    if pid == 0 || pid == CURRENT_PID.load(Ordering::Acquire) {
        return false;
    }
    let mut processes = PROCESSES.lock();
    let Some(process) = processes.slots.iter_mut().find(|process| {
        process.pid == pid && matches!(process.state, ProcessState::Ready | ProcessState::Running)
    }) else {
        return false;
    };
    process.status = status;
    process.state = ProcessState::Zombie;
    true
}

pub fn distinct_generation(first: &ProcessToken, second: &ProcessToken) -> bool {
    first.pid != second.pid && (first.slot != second.slot || first.generation != second.generation)
}

pub fn stats() -> ProcessTableStats {
    let processes = PROCESSES.lock();
    let ready = processes
        .slots
        .iter()
        .filter(|process| process.state == ProcessState::Ready)
        .count();
    let running = processes
        .slots
        .iter()
        .filter(|process| process.state == ProcessState::Running)
        .count();
    let zombies = processes
        .slots
        .iter()
        .filter(|process| process.state == ProcessState::Zombie)
        .count();
    let names_valid = processes.slots.iter().all(|process| {
        process.name_len as usize <= MAX_PROCESS_NAME
            && (process.state == ProcessState::Empty
                || process.name[..process.name_len as usize]
                    .iter()
                    .all(|byte| *byte != 0))
    });
    ProcessTableStats {
        spawned: processes.spawned,
        reaped: processes.reaped,
        highest_pid: processes.highest_pid,
        ready,
        running,
        zombies,
        verified: names_valid
            && processes.spawned >= processes.reaped
            && CURRENT_PID.load(Ordering::Acquire) == 0,
    }
}

pub fn prepare_linux_stack(
    mapping: &mut UserImageMapping,
    image: &ElfImage<'_>,
    executable: &str,
) -> ProcessStackReport {
    if executable.is_empty()
        || executable.len() >= 256
        || !image.range_loaded(64, image.program_header_count() * 56)
    {
        return empty_report();
    }
    let stack_virtual = mapping.load_bias + 510 * PAGE_SIZE;
    let mut cursor = STACK_BYTES - 16;
    let Some(executable_address) = push_bytes(
        mapping.stack_physical,
        stack_virtual,
        &mut cursor,
        executable.as_bytes(),
        true,
    ) else {
        return empty_report();
    };
    let Some(platform_address) = push_bytes(
        mapping.stack_physical,
        stack_virtual,
        &mut cursor,
        b"x86_64",
        true,
    ) else {
        return empty_report();
    };
    let mut random = [0u8; 16];
    if !crate::random::fill(&mut random) {
        return empty_report();
    }
    let random_nonzero = random.iter().any(|byte| *byte != 0);
    let Some(random_address) = push_bytes(
        mapping.stack_physical,
        stack_virtual,
        &mut cursor,
        &random,
        false,
    ) else {
        return empty_report();
    };
    random.fill(0);
    cursor &= !15;
    let mut words = [0u64; 32];
    let mut count = 0;
    push_word(&mut words, &mut count, 1);
    push_word(&mut words, &mut count, executable_address);
    push_word(&mut words, &mut count, 0);
    push_word(&mut words, &mut count, 0);
    push_pair(&mut words, &mut count, AT_PHDR, mapping.load_bias + 64);
    push_pair(&mut words, &mut count, AT_PHENT, 56);
    push_pair(
        &mut words,
        &mut count,
        AT_PHNUM,
        image.program_header_count() as u64,
    );
    push_pair(&mut words, &mut count, AT_PAGESZ, PAGE_SIZE);
    push_pair(&mut words, &mut count, AT_ENTRY, mapping.entry);
    push_pair(&mut words, &mut count, AT_UID, 0);
    push_pair(&mut words, &mut count, AT_EUID, 0);
    push_pair(&mut words, &mut count, AT_GID, 0);
    push_pair(&mut words, &mut count, AT_EGID, 0);
    push_pair(&mut words, &mut count, AT_PLATFORM, platform_address);
    push_pair(&mut words, &mut count, AT_SECURE, 0);
    push_pair(&mut words, &mut count, AT_RANDOM, random_address);
    push_pair(&mut words, &mut count, AT_EXECFN, executable_address);
    push_pair(&mut words, &mut count, AT_NULL, 0);
    let vector_bytes = count * core::mem::size_of::<u64>();
    if cursor < vector_bytes {
        return empty_report();
    }
    cursor -= vector_bytes;
    cursor &= !15;
    unsafe {
        core::ptr::copy_nonoverlapping(
            words.as_ptr() as *const u8,
            (mapping.stack_physical as usize as *mut u8).add(cursor),
            vector_bytes,
        );
    }
    let stack_pointer = stack_virtual + cursor as u64;
    mapping.stack_top = stack_pointer;
    let argc = unsafe {
        core::ptr::read_volatile((mapping.stack_physical + cursor as u64) as usize as *const u64)
    };
    let aligned = stack_pointer & 15 == 0;
    ProcessStackReport {
        stack_pointer,
        bytes_used: STACK_BYTES - cursor,
        argc,
        aux_entries: 13,
        random_nonzero,
        aligned,
        verified: argc == 1 && aligned && random_nonzero && executable_address >= stack_virtual,
    }
}

/// The scheduler-integrated equivalent of `prepare_linux_stack`, for a real
/// `arch::paging::ProcessAddressSpace` (fork/exec-capable) instead of the
/// boot-time single-shot `UserImageMapping`. Without this, a statically
/// linked binary whose runtime reads the standard argc/argv/envp/auxv
/// stack contract (anything using Rust's `std`, which reads `AT_RANDOM`
/// during init) dereferences a zeroed stack and segfaults immediately -
/// confirmed live: `aeros-init` (no_std, never touches argv/auxv) ran
/// fine through `run` before this existed, `aeros-std-smoke` did not.
/// The payload here is small (under 400 bytes), so the one-page stack
/// these processes get is ample room, unlike the old path's two pages.
/// Same, with an explicit argv and environment (each string without its NUL).
pub fn prepare_scheduled_process_stack_with(
    space: &mut crate::arch::paging::ProcessAddressSpace,
    image: &ElfImage<'_>,
    executable: &str,
    args: &[&[u8]],
    env: &[&[u8]],
) -> bool {
    if executable.is_empty() || executable.len() >= 256 || args.len() > 16 || env.len() > 16 {
        return false;
    }
    let stack_physical = space.stack_physical;
    let stack_virtual = crate::arch::paging::process_stack_base();
    let mut cursor = crate::arch::paging::PROCESS_STACK_BYTES as usize - 16;
    let Some(executable_address) = push_bytes(
        stack_physical,
        stack_virtual,
        &mut cursor,
        executable.as_bytes(),
        true,
    ) else {
        return false;
    };
    let Some(platform_address) =
        push_bytes(stack_physical, stack_virtual, &mut cursor, b"x86_64", true)
    else {
        return false;
    };
    let mut random = [0u8; 16];
    if !crate::random::fill(&mut random) {
        return false;
    }
    let Some(random_address) =
        push_bytes(stack_physical, stack_virtual, &mut cursor, &random, false)
    else {
        return false;
    };
    random.fill(0);
    let mut arg_addresses = [0u64; 16];
    for (slot, arg) in args.iter().enumerate() {
        let Some(address) = push_bytes(stack_physical, stack_virtual, &mut cursor, arg, true)
        else {
            return false;
        };
        arg_addresses[slot] = address;
    }
    let mut env_addresses = [0u64; 16];
    for (slot, value) in env.iter().enumerate() {
        let Some(address) = push_bytes(stack_physical, stack_virtual, &mut cursor, value, true)
        else {
            return false;
        };
        env_addresses[slot] = address;
    }
    cursor &= !15;
    let mut words = [0u64; 96];
    let mut count = 0;
    push_word(&mut words, &mut count, args.len() as u64);
    for address in &arg_addresses[..args.len()] {
        push_word(&mut words, &mut count, *address);
    }
    push_word(&mut words, &mut count, 0);
    for address in &env_addresses[..env.len()] {
        push_word(&mut words, &mut count, *address);
    }
    push_word(&mut words, &mut count, 0);
    let virtual_base = crate::arch::paging::process_virtual_base();
    push_pair(&mut words, &mut count, AT_PHDR, virtual_base + 64);
    push_pair(&mut words, &mut count, AT_PHENT, 56);
    push_pair(
        &mut words,
        &mut count,
        AT_PHNUM,
        image.program_header_count() as u64,
    );
    push_pair(&mut words, &mut count, AT_PAGESZ, PAGE_SIZE);
    push_pair(&mut words, &mut count, AT_ENTRY, space.entry);
    push_pair(&mut words, &mut count, AT_UID, 0);
    push_pair(&mut words, &mut count, AT_EUID, 0);
    push_pair(&mut words, &mut count, AT_GID, 0);
    push_pair(&mut words, &mut count, AT_EGID, 0);
    push_pair(&mut words, &mut count, AT_PLATFORM, platform_address);
    push_pair(&mut words, &mut count, AT_SECURE, 0);
    push_pair(&mut words, &mut count, AT_RANDOM, random_address);
    push_pair(&mut words, &mut count, AT_EXECFN, executable_address);
    push_pair(&mut words, &mut count, AT_NULL, 0);
    let vector_bytes = count * core::mem::size_of::<u64>();
    if cursor < vector_bytes {
        return false;
    }
    cursor -= vector_bytes;
    cursor &= !15;
    unsafe {
        core::ptr::copy_nonoverlapping(
            words.as_ptr() as *const u8,
            (stack_physical as usize as *mut u8).add(cursor),
            vector_bytes,
        );
    }
    space.stack_top = stack_virtual + cursor as u64;
    true
}

fn push_bytes(
    physical: u64,
    virtual_base: u64,
    cursor: &mut usize,
    bytes: &[u8],
    nul: bool,
) -> Option<u64> {
    let size = bytes.len().checked_add(usize::from(nul))?;
    *cursor = cursor.checked_sub(size)?;
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (physical as usize as *mut u8).add(*cursor),
            bytes.len(),
        );
        if nul {
            core::ptr::write((physical as usize as *mut u8).add(*cursor + bytes.len()), 0);
        }
    }
    Some(virtual_base + *cursor as u64)
}

fn push_pair(words: &mut [u64], count: &mut usize, key: u64, value: u64) {
    push_word(words, count, key);
    push_word(words, count, value);
}

fn push_word(words: &mut [u64], count: &mut usize, value: u64) {
    words[*count] = value;
    *count += 1;
}

fn empty_report() -> ProcessStackReport {
    ProcessStackReport {
        stack_pointer: 0,
        bytes_used: 0,
        argc: 0,
        aux_entries: 0,
        random_nonzero: false,
        aligned: false,
        verified: false,
    }
}
