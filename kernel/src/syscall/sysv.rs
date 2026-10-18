//! System V message queues and semaphore sets. The tables belong to the
//! kernel, not to a process, so unrelated processes meet at the same object
//! through its key. Waiting calls give up the CPU and end with EINTR when a
//! signal arrives or EIDRM when the object is removed under them.

use super::*;

const IPC_CREAT: u64 = 0o1000;
const IPC_EXCL: u64 = 0o2000;
const IPC_NOWAIT: u64 = 0o4000;
const MSG_NOERROR: u64 = 0o10000;
const MSG_EXCEPT: u64 = 0o20000;
const IPC_RMID: u64 = 0;
const IPC_SET: u64 = 1;
const IPC_STAT: u64 = 2;

/// Identifiers are `sequence * ID_STRIDE + slot`, so a removed object's
/// number is not handed out again at once.
const ID_STRIDE: u64 = 32768;

const QUEUES: usize = 4;
const QUEUE_MESSAGES: usize = 16;
pub(super) const MESSAGE_BYTES: usize = 512;
const QUEUE_BYTES: u64 = (QUEUE_MESSAGES * MESSAGE_BYTES) as u64;

const SEMAPHORE_SETS: usize = 4;
const SEMAPHORES: usize = 16;
const SEMAPHORE_MAX: i32 = 32767;
const OPERATIONS_MAX: usize = 32;

#[derive(Clone, Copy)]
struct Message {
    kind: i64,
    length: usize,
    data: [u8; MESSAGE_BYTES],
}

impl Message {
    const EMPTY: Self = Self {
        kind: 0,
        length: 0,
        data: [0; MESSAGE_BYTES],
    };
}

struct Queue {
    used: bool,
    key: i32,
    sequence: u64,
    mode: u16,
    count: usize,
    messages: [Message; QUEUE_MESSAGES],
}

impl Queue {
    const EMPTY: Self = Self {
        used: false,
        key: 0,
        sequence: 0,
        mode: 0,
        count: 0,
        messages: [Message::EMPTY; QUEUE_MESSAGES],
    };
}

#[derive(Clone, Copy)]
struct SemaphoreSet {
    used: bool,
    key: i32,
    sequence: u64,
    mode: u16,
    count: usize,
    values: [i32; SEMAPHORES],
}

impl SemaphoreSet {
    const EMPTY: Self = Self {
        used: false,
        key: 0,
        sequence: 0,
        mode: 0,
        count: 0,
        values: [0; SEMAPHORES],
    };
}

static QUEUE_TABLE: TicketLock<[Queue; QUEUES]> = TicketLock::new([Queue::EMPTY; QUEUES]);
static SEMAPHORE_TABLE: TicketLock<[SemaphoreSet; SEMAPHORE_SETS]> =
    TicketLock::new([SemaphoreSet::EMPTY; SEMAPHORE_SETS]);
static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn next_sequence() -> u64 {
    NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed) % 65536
}

fn identifier(slot: usize, sequence: u64) -> u64 {
    sequence * ID_STRIDE + slot as u64
}

/// What a `get` call does given the object already holding its key: the slot
/// to hand back, nothing (make a new one) or an error.
fn get_decision(key: i32, flags: u64, existing: Option<usize>) -> Result<Option<usize>, u64> {
    match existing {
        Some(slot) if key != 0 => {
            if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 {
                Err(error(17))
            } else {
                Ok(Some(slot))
            }
        }
        _ if key != 0 && flags & IPC_CREAT == 0 => Err(error(2)),
        _ => Ok(None),
    }
}

pub(super) fn message_queue_get(key: i32, flags: u64) -> u64 {
    let mut table = QUEUE_TABLE.lock();
    let existing = table
        .iter()
        .position(|queue| queue.used && queue.key == key);
    match get_decision(key, flags, existing) {
        Err(failure) => failure,
        Ok(Some(slot)) => identifier(slot, table[slot].sequence),
        Ok(None) => {
            let Some(slot) = table.iter().position(|queue| !queue.used) else {
                return error(28);
            };
            let sequence = next_sequence();
            table[slot] = Queue {
                used: true,
                key,
                sequence,
                mode: (flags & 0o777) as u16,
                ..Queue::EMPTY
            };
            identifier(slot, sequence)
        }
    }
}

fn queue_slot(table: &[Queue; QUEUES], id: u64) -> Option<usize> {
    let slot = (id % ID_STRIDE) as usize;
    let queue = table.get(slot)?;
    (queue.used && queue.sequence == id / ID_STRIDE).then_some(slot)
}

pub(super) fn message_send(id: u64, kind: i64, data: &[u8], flags: u64) -> Result<(), u64> {
    if kind <= 0 || data.len() > MESSAGE_BYTES {
        return Err(error(22));
    }
    let mut first = true;
    loop {
        {
            let mut table = QUEUE_TABLE.lock();
            let Some(slot) = queue_slot(&table, id) else {
                return Err(error(if first { 22 } else { 43 }));
            };
            let queue = &mut table[slot];
            if queue.count < QUEUE_MESSAGES {
                let message = &mut queue.messages[queue.count];
                message.kind = kind;
                message.length = data.len();
                message.data[..data.len()].copy_from_slice(data);
                queue.count += 1;
                return Ok(());
            }
        }
        if flags & IPC_NOWAIT != 0 {
            return Err(error(11));
        }
        if interrupted() {
            return Err(error(4));
        }
        first = false;
        yield_in_syscall();
    }
}

/// Position of the message `selector` asks for: 0 the first, positive the
/// first of that type (or of any other with MSG_EXCEPT), negative the lowest
/// type up to its magnitude.
fn select_message(queue: &Queue, selector: i64, flags: u64) -> Option<usize> {
    let messages = &queue.messages[..queue.count];
    match selector {
        0 => (!messages.is_empty()).then_some(0),
        1.. if flags & MSG_EXCEPT != 0 => messages.iter().position(|m| m.kind != selector),
        1.. => messages.iter().position(|m| m.kind == selector),
        _ => {
            let limit = selector.checked_neg().unwrap_or(i64::MAX);
            messages
                .iter()
                .enumerate()
                .filter(|(_, message)| message.kind <= limit)
                .min_by_key(|(_, message)| message.kind)
                .map(|(index, _)| index)
        }
    }
}

/// Takes a message off the queue; `capacity` is the size of the caller's
/// buffer. Returns the type and the length stored in `out`.
pub(super) fn message_receive(
    id: u64,
    selector: i64,
    flags: u64,
    capacity: usize,
    out: &mut [u8; MESSAGE_BYTES],
) -> Result<(i64, usize), u64> {
    let mut first = true;
    loop {
        {
            let mut table = QUEUE_TABLE.lock();
            let Some(slot) = queue_slot(&table, id) else {
                return Err(error(if first { 22 } else { 43 }));
            };
            let queue = &mut table[slot];
            if let Some(index) = select_message(queue, selector, flags) {
                let message = queue.messages[index];
                if message.length > capacity && flags & MSG_NOERROR == 0 {
                    return Err(error(7));
                }
                let length = message.length.min(capacity);
                out[..length].copy_from_slice(&message.data[..length]);
                queue.messages.copy_within(index + 1..queue.count, index);
                queue.count -= 1;
                return Ok((message.kind, length));
            }
        }
        if flags & IPC_NOWAIT != 0 {
            return Err(error(42));
        }
        if interrupted() {
            return Err(error(4));
        }
        first = false;
        yield_in_syscall();
    }
}

fn write_permissions(record: &mut [u8], key: i32, mode: u16, sequence: u64) {
    record[..4].copy_from_slice(&key.to_le_bytes());
    record[20..24].copy_from_slice(&u32::from(mode).to_le_bytes());
    record[24..26].copy_from_slice(&(sequence as u16).to_le_bytes());
}

pub(super) fn message_control(id: u64, command: u64, buffer: u64) -> u64 {
    let mut table = QUEUE_TABLE.lock();
    let Some(slot) = queue_slot(&table, id) else {
        return error(22);
    };
    match command & 0xff {
        IPC_RMID => {
            table[slot] = Queue::EMPTY;
            0
        }
        IPC_SET => {
            let mut record = [0u8; 120];
            if user::copy_from_user(buffer, &mut record) {
                0
            } else {
                error(14)
            }
        }
        IPC_STAT => {
            let queue = &table[slot];
            let mut record = [0u8; 120];
            write_permissions(&mut record, queue.key, queue.mode, queue.sequence);
            let bytes: usize = queue.messages[..queue.count]
                .iter()
                .map(|message| message.length)
                .sum();
            record[72..80].copy_from_slice(&(bytes as u64).to_le_bytes());
            record[80..88].copy_from_slice(&(queue.count as u64).to_le_bytes());
            record[88..96].copy_from_slice(&QUEUE_BYTES.to_le_bytes());
            if user::copy_to_user(buffer, &record) {
                0
            } else {
                error(14)
            }
        }
        _ => error(22),
    }
}

pub(super) fn semaphore_get(key: i32, count: u64, flags: u64) -> u64 {
    let mut table = SEMAPHORE_TABLE.lock();
    let existing = table.iter().position(|set| set.used && set.key == key);
    match get_decision(key, flags, existing) {
        Err(failure) => failure,
        Ok(Some(slot)) => {
            if count > table[slot].count as u64 {
                return error(22);
            }
            identifier(slot, table[slot].sequence)
        }
        Ok(None) => {
            if count == 0 || count > SEMAPHORES as u64 {
                return error(22);
            }
            let Some(slot) = table.iter().position(|set| !set.used) else {
                return error(28);
            };
            let sequence = next_sequence();
            table[slot] = SemaphoreSet {
                used: true,
                key,
                sequence,
                mode: (flags & 0o777) as u16,
                count: count as usize,
                ..SemaphoreSet::EMPTY
            };
            identifier(slot, sequence)
        }
    }
}

fn set_slot(table: &[SemaphoreSet; SEMAPHORE_SETS], id: u64) -> Option<usize> {
    let slot = (id % ID_STRIDE) as usize;
    let set = table.get(slot)?;
    (set.used && set.sequence == id / ID_STRIDE).then_some(slot)
}

/// One `sembuf`: semaphore number, operation, flags.
#[derive(Clone, Copy)]
pub(super) struct Operation {
    pub number: u16,
    pub amount: i16,
    pub flags: i16,
}

enum Outcome {
    Done,
    Blocked(bool),
    Failed(u64),
}

/// Applies the operations to a copy of the set; nothing is kept unless every
/// one of them goes through.
fn try_operations(set: &mut SemaphoreSet, operations: &[Operation]) -> Outcome {
    let mut values = set.values;
    for operation in operations {
        let number = usize::from(operation.number);
        if number >= set.count {
            return Outcome::Failed(error(27));
        }
        let no_wait = operation.flags as u16 & IPC_NOWAIT as u16 != 0;
        let amount = i32::from(operation.amount);
        if amount > 0 {
            let next = values[number] + amount;
            if next > SEMAPHORE_MAX {
                return Outcome::Failed(error(34));
            }
            values[number] = next;
        } else if amount == 0 {
            if values[number] != 0 {
                return Outcome::Blocked(no_wait);
            }
        } else {
            let next = values[number] + amount;
            if next < 0 {
                return Outcome::Blocked(no_wait);
            }
            values[number] = next;
        }
    }
    set.values = values;
    Outcome::Done
}

pub(super) fn semaphore_operate(
    id: u64,
    operations: &[Operation],
    timeout: Option<u64>,
) -> Result<(), u64> {
    if operations.is_empty() {
        return Err(error(22));
    }
    if operations.len() > OPERATIONS_MAX {
        return Err(error(7));
    }
    let deadline = timeout.map(|wait| crate::time::monotonic_nanoseconds().saturating_add(wait));
    let mut first = true;
    loop {
        {
            let mut table = SEMAPHORE_TABLE.lock();
            let Some(slot) = set_slot(&table, id) else {
                return Err(error(if first { 22 } else { 43 }));
            };
            match try_operations(&mut table[slot], operations) {
                Outcome::Done => return Ok(()),
                Outcome::Failed(failure) => return Err(failure),
                Outcome::Blocked(true) => return Err(error(11)),
                Outcome::Blocked(false) => {}
            }
        }
        if deadline.is_some_and(|limit| crate::time::monotonic_nanoseconds() >= limit) {
            return Err(error(11));
        }
        if interrupted() {
            return Err(error(4));
        }
        first = false;
        yield_in_syscall();
    }
}

pub(super) fn semaphore_control(id: u64, number: u64, command: u64, argument: u64) -> u64 {
    const GETPID: u64 = 11;
    const GETVAL: u64 = 12;
    const GETALL: u64 = 13;
    const GETNCNT: u64 = 14;
    const GETZCNT: u64 = 15;
    const SETVAL: u64 = 16;
    const SETALL: u64 = 17;
    const SEM_STAT: u64 = 18;
    let mut table = SEMAPHORE_TABLE.lock();
    let Some(slot) = set_slot(&table, id) else {
        return error(22);
    };
    let count = table[slot].count;
    let valid_number = (number as usize) < count && number < SEMAPHORES as u64;
    match command & 0xff {
        IPC_RMID => {
            table[slot] = SemaphoreSet::EMPTY;
            0
        }
        IPC_SET => 0,
        IPC_STAT | SEM_STAT => {
            let set = &table[slot];
            let mut record = [0u8; 72];
            write_permissions(&mut record, set.key, set.mode, set.sequence);
            record[64..72].copy_from_slice(&(set.count as u64).to_le_bytes());
            if user::copy_to_user(argument, &record) {
                0
            } else {
                error(14)
            }
        }
        GETVAL if valid_number => table[slot].values[number as usize] as u64,
        GETPID | GETNCNT | GETZCNT if valid_number => 0,
        SETVAL if valid_number => {
            let value = argument as i32;
            if !(0..=SEMAPHORE_MAX).contains(&value) {
                return error(34);
            }
            table[slot].values[number as usize] = value;
            0
        }
        GETALL => {
            let mut encoded = [0u8; SEMAPHORES * 2];
            for (index, value) in table[slot].values[..count].iter().enumerate() {
                encoded[index * 2..index * 2 + 2].copy_from_slice(&(*value as u16).to_le_bytes());
            }
            if user::copy_to_user(argument, &encoded[..count * 2]) {
                0
            } else {
                error(14)
            }
        }
        SETALL => {
            let mut encoded = [0u8; SEMAPHORES * 2];
            if !user::copy_from_user(argument, &mut encoded[..count * 2]) {
                return error(14);
            }
            let mut values = [0i32; SEMAPHORES];
            for (index, value) in values[..count].iter_mut().enumerate() {
                *value = i32::from(u16::from_le_bytes([
                    encoded[index * 2],
                    encoded[index * 2 + 1],
                ]));
                if *value > SEMAPHORE_MAX {
                    return error(34);
                }
            }
            table[slot].values = values;
            0
        }
        _ => error(22),
    }
}

pub(super) fn linux_msgget(key: u64, flags: u64) -> u64 {
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    message_queue_get(key as i32, flags)
}

pub(super) fn linux_msgsnd(id: u64, message: u64, size: u64, flags: u64) -> u64 {
    let Ok(size) = usize::try_from(size) else {
        return error(22);
    };
    if size > MESSAGE_BYTES {
        return error(22);
    }
    let mut record = [0u8; 8 + MESSAGE_BYTES];
    if !user::copy_from_user(message, &mut record[..8 + size]) {
        return error(14);
    }
    let kind = i64::from_le_bytes([
        record[0], record[1], record[2], record[3], record[4], record[5], record[6], record[7],
    ]);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    match message_send(id, kind, &record[8..8 + size], flags) {
        Ok(()) => 0,
        Err(failure) => failure,
    }
}

pub(super) fn linux_msgrcv(id: u64, message: u64, size: u64, selector: u64, flags: u64) -> u64 {
    let Ok(size) = usize::try_from(size) else {
        return error(22);
    };
    let mut data = [0u8; MESSAGE_BYTES];
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    match message_receive(id, selector as i64, flags, size, &mut data) {
        Ok((kind, length)) => {
            let mut record = [0u8; 8 + MESSAGE_BYTES];
            record[..8].copy_from_slice(&kind.to_le_bytes());
            record[8..8 + length].copy_from_slice(&data[..length]);
            if user::copy_to_user(message, &record[..8 + length]) {
                length as u64
            } else {
                error(14)
            }
        }
        Err(failure) => failure,
    }
}

pub(super) fn linux_msgctl(id: u64, command: u64, buffer: u64) -> u64 {
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    message_control(id, command, buffer)
}

pub(super) fn linux_semget(key: u64, count: u64, flags: u64) -> u64 {
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    semaphore_get(key as i32, count, flags)
}

/// `semop` and, with a timeout address, `semtimedop`.
pub(super) fn linux_semop(id: u64, address: u64, count: u64, timeout: u64) -> u64 {
    let Ok(count) = usize::try_from(count) else {
        return error(22);
    };
    if count > OPERATIONS_MAX {
        return error(7);
    }
    let mut encoded = [0u8; OPERATIONS_MAX * 6];
    if !user::copy_from_user(address, &mut encoded[..count * 6]) {
        return error(14);
    }
    let mut operations = [Operation {
        number: 0,
        amount: 0,
        flags: 0,
    }; OPERATIONS_MAX];
    for (index, operation) in operations[..count].iter_mut().enumerate() {
        let at = index * 6;
        *operation = Operation {
            number: u16::from_le_bytes([encoded[at], encoded[at + 1]]),
            amount: i16::from_le_bytes([encoded[at + 2], encoded[at + 3]]),
            flags: i16::from_le_bytes([encoded[at + 4], encoded[at + 5]]),
        };
    }
    let wait = if timeout == 0 {
        None
    } else {
        match read_timespec(timeout) {
            Some(duration) => Some(duration),
            None => return error(22),
        }
    };
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    match semaphore_operate(id, &operations[..count], wait) {
        Ok(()) => 0,
        Err(failure) => failure,
    }
}

pub(super) fn linux_semctl(id: u64, number: u64, command: u64, argument: u64) -> u64 {
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    semaphore_control(id, number, command, argument)
}

const SEGMENTS: usize = 4;
const SEGMENT_PAGES: usize = 64;
const SHM_RDONLY: u64 = 0o10000;

struct Segment {
    used: bool,
    key: i32,
    sequence: u64,
    mode: u16,
    size: usize,
    pages: usize,
    removed: bool,
    frames: [u64; SEGMENT_PAGES],
}

impl Segment {
    const EMPTY: Self = Self {
        used: false,
        key: 0,
        sequence: 0,
        mode: 0,
        size: 0,
        pages: 0,
        removed: false,
        frames: [0; SEGMENT_PAGES],
    };

    /// Mappings of the segment: the frames' owners besides the segment.
    fn attached(&self) -> u64 {
        u64::from(crate::arch::paging::code_page_refcount(self.frames[0]).saturating_sub(1))
    }

    fn free_frames(&mut self) {
        for frame in &self.frames[..self.pages] {
            crate::arch::paging::release_frame(*frame);
        }
        *self = Self::EMPTY;
    }
}

static SEGMENT_TABLE: TicketLock<[Segment; SEGMENTS]> = TicketLock::new([Segment::EMPTY; SEGMENTS]);

/// Frees the removed segments nobody is attached to any more (a process that
/// exits while attached leaves its segment to be found here).
fn reap_segments(table: &mut [Segment; SEGMENTS]) {
    for segment in table.iter_mut() {
        if segment.used && segment.removed && segment.attached() == 0 {
            segment.free_frames();
        }
    }
}

fn segment_slot(table: &[Segment; SEGMENTS], id: u64) -> Option<usize> {
    let slot = (id % ID_STRIDE) as usize;
    let segment = table.get(slot)?;
    (segment.used && segment.sequence == id / ID_STRIDE).then_some(slot)
}

pub(super) fn shared_memory_get(key: i32, size: u64, flags: u64) -> u64 {
    let mut table = SEGMENT_TABLE.lock();
    reap_segments(&mut table);
    let existing = table
        .iter()
        .position(|segment| segment.used && !segment.removed && segment.key == key);
    match get_decision(key, flags, existing) {
        Err(failure) => failure,
        Ok(Some(slot)) => {
            if size > table[slot].size as u64 {
                return error(22);
            }
            identifier(slot, table[slot].sequence)
        }
        Ok(None) => {
            let pages = size.div_ceil(4096) as usize;
            if size == 0 || pages > SEGMENT_PAGES {
                return error(22);
            }
            let Some(slot) = table.iter().position(|segment| !segment.used) else {
                return error(28);
            };
            let mut frames = [0u64; SEGMENT_PAGES];
            for index in 0..pages {
                let Some(frame) = crate::memory::allocate_global() else {
                    for taken in &frames[..index] {
                        crate::arch::paging::release_frame(*taken);
                    }
                    return error(12);
                };
                let address = frame.address();
                // SAFETY: a frame just taken from the allocator, mapped 1:1.
                unsafe { core::ptr::write_bytes(address as usize as *mut u8, 0, 4096) };
                frames[index] = address;
            }
            let sequence = next_sequence();
            table[slot] = Segment {
                used: true,
                key,
                sequence,
                mode: (flags & 0o777) as u16,
                size: size as usize,
                pages,
                removed: false,
                frames,
            };
            identifier(slot, sequence)
        }
    }
}

pub(super) fn linux_shmget(key: u64, size: u64, flags: u64) -> u64 {
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    shared_memory_get(key as i32, size, flags)
}

pub(super) fn linux_shmat(id: u64, address: u64, flags: u64) -> u64 {
    if address != 0 {
        return error(22);
    }
    let protection = if flags & SHM_RDONLY != 0 { 1 } else { 3 };
    let (frames, pages) = {
        let mut table = SEGMENT_TABLE.lock();
        reap_segments(&mut table);
        let Some(slot) = segment_slot(&table, id) else {
            return error(22);
        };
        let segment = &table[slot];
        if segment.removed {
            return error(22);
        }
        let mut frames = [0u64; SEGMENT_PAGES];
        for (held, frame) in segment.frames[..segment.pages].iter().enumerate() {
            if !crate::arch::paging::retain_frame(*frame) {
                for taken in &frames[..held] {
                    crate::arch::paging::release_frame(*taken);
                }
                return error(12);
            }
            frames[held] = *frame;
        }
        (frames, segment.pages)
    };
    let placed = crate::scheduler::linux_mmap_shared_for_current_task(
        pages,
        protection,
        Some(&frames[..pages]),
    );
    let mapped = match placed {
        Some(Some(address)) => address,
        failed => {
            for frame in &frames[..pages] {
                crate::arch::paging::release_frame(*frame);
            }
            return error(if failed.is_none() { 38 } else { 12 });
        }
    };
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    MMAPS.fetch_add(1, Ordering::Relaxed);
    mapped
}

pub(super) fn linux_shmdt(address: u64) -> u64 {
    let Some(Some(frame)) = crate::scheduler::linux_mmap_frame_for_current_task(address) else {
        return error(22);
    };
    let pages = {
        let table = SEGMENT_TABLE.lock();
        let found = table
            .iter()
            .find(|segment| segment.used && segment.frames[0] == frame);
        match found {
            Some(segment) => segment.pages,
            None => return error(22),
        }
    };
    if crate::scheduler::linux_munmap_for_current_task(address, pages) != Some(true) {
        return error(22);
    }
    reap_segments(&mut SEGMENT_TABLE.lock());
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

pub(super) fn linux_shmctl(id: u64, command: u64, buffer: u64) -> u64 {
    const SHM_LOCK: u64 = 11;
    const SHM_UNLOCK: u64 = 12;
    let mut table = SEGMENT_TABLE.lock();
    reap_segments(&mut table);
    let Some(slot) = segment_slot(&table, id) else {
        return error(22);
    };
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    match command & 0xff {
        IPC_RMID => {
            table[slot].removed = true;
            table[slot].key = 0;
            reap_segments(&mut table);
            0
        }
        IPC_SET => {
            let mut record = [0u8; 112];
            if user::copy_from_user(buffer, &mut record) {
                0
            } else {
                error(14)
            }
        }
        IPC_STAT => {
            let segment = &table[slot];
            let mut record = [0u8; 112];
            write_permissions(&mut record, segment.key, segment.mode, segment.sequence);
            record[48..56].copy_from_slice(&(segment.size as u64).to_le_bytes());
            record[88..96].copy_from_slice(&segment.attached().to_le_bytes());
            if user::copy_to_user(buffer, &record) {
                0
            } else {
                error(14)
            }
        }
        SHM_LOCK | SHM_UNLOCK => 0,
        _ => error(22),
    }
}

/// Segments driven without a process to attach them to: creation, the
/// attach count (as another owner of the frames), removal while attached
/// and the reaping once the last owner is gone.
fn shared_memory_self_test() -> bool {
    let missing = shared_memory_get(0x5eee, 4096, 0) == error(2);
    let id = shared_memory_get(0x5eee, 8192, IPC_CREAT | 0o600);
    let again = shared_memory_get(0x5eee, 100, 0) == id;
    let too_big = shared_memory_get(0x5eee, 12288, 0) == error(22);
    let exclusive = shared_memory_get(0x5eee, 4096, IPC_CREAT | IPC_EXCL) == error(17);
    let huge = shared_memory_get(0, (SEGMENT_PAGES as u64 + 1) * 4096, IPC_CREAT) == error(22);
    let (first, zeroed) = {
        let table = SEGMENT_TABLE.lock();
        let Some(slot) = segment_slot(&table, id) else {
            return false;
        };
        let segment = &table[slot];
        let zeroed = segment.pages == 2
            && segment.frames[..2].iter().all(|frame| {
                // SAFETY: segment frames are allocated and mapped 1:1.
                let bytes =
                    unsafe { core::slice::from_raw_parts(*frame as usize as *const u8, 4096) };
                bytes.iter().all(|byte| *byte == 0)
            });
        (segment.frames, zeroed)
    };
    let attach =
        crate::arch::paging::retain_frame(first[0]) && crate::arch::paging::retain_frame(first[1]);
    let counted = {
        let table = SEGMENT_TABLE.lock();
        segment_slot(&table, id).is_some_and(|slot| table[slot].attached() == 1)
    };
    let removed_attached = linux_shmctl_internal_remove(id);
    let kept = {
        let table = SEGMENT_TABLE.lock();
        segment_slot(&table, id).is_some()
    };
    crate::arch::paging::release_frame(first[0]);
    crate::arch::paging::release_frame(first[1]);
    reap_segments(&mut SEGMENT_TABLE.lock());
    let gone = {
        let table = SEGMENT_TABLE.lock();
        segment_slot(&table, id).is_none()
    };
    missing
        && again
        && too_big
        && exclusive
        && huge
        && zeroed
        && attach
        && counted
        && removed_attached
        && kept
        && gone
}

fn linux_shmctl_internal_remove(id: u64) -> bool {
    let mut table = SEGMENT_TABLE.lock();
    let Some(slot) = segment_slot(&table, id) else {
        return false;
    };
    table[slot].removed = true;
    table[slot].key = 0;
    reap_segments(&mut table);
    true
}

/// Queues and semaphore sets driven without user pointers.
pub(super) fn self_test() -> bool {
    let queue = message_queue_get(0, IPC_CREAT | 0o600);
    let missing = message_queue_get(0x5eed, 0) == error(2);
    let keyed = message_queue_get(0x5eed, IPC_CREAT | 0o600);
    let found = message_queue_get(0x5eed, 0) == keyed;
    let exclusive = message_queue_get(0x5eed, IPC_CREAT | IPC_EXCL) == error(17);
    let mut buffer = [0u8; MESSAGE_BYTES];
    let sent = message_send(queue, 5, b"five", 0).is_ok()
        && message_send(queue, 2, b"two", 0).is_ok()
        && message_send(queue, 9, b"nine", 0).is_ok();
    let bad_type = message_send(queue, 0, b"x", 0) == Err(error(22));
    let lowest = message_receive(queue, -6, 0, MESSAGE_BYTES, &mut buffer) == Ok((2, 3))
        && &buffer[..3] == b"two";
    let exact = message_receive(queue, 9, 0, MESSAGE_BYTES, &mut buffer) == Ok((9, 4));
    let except = message_receive(queue, 7, MSG_EXCEPT, 2, &mut buffer) == Err(error(7))
        && message_receive(queue, 7, MSG_EXCEPT | MSG_NOERROR, 2, &mut buffer) == Ok((5, 2));
    let empty = message_receive(queue, 0, IPC_NOWAIT, MESSAGE_BYTES, &mut buffer) == Err(error(42));
    let mut filled = true;
    for _ in 0..QUEUE_MESSAGES {
        filled &= message_send(queue, 1, b"x", IPC_NOWAIT).is_ok();
    }
    let full = message_send(queue, 1, b"x", IPC_NOWAIT) == Err(error(11));
    let removed = QUEUE_TABLE.lock()[(queue % ID_STRIDE) as usize].count == QUEUE_MESSAGES
        && message_control(queue, IPC_RMID, 0) == 0
        && message_control(keyed, IPC_RMID, 0) == 0;
    let gone = message_send(queue, 1, b"x", IPC_NOWAIT) == Err(error(22));

    let set = semaphore_get(0, 2, IPC_CREAT | 0o600);
    let raise = [Operation {
        number: 0,
        amount: 3,
        flags: 0,
    }];
    let take = [Operation {
        number: 0,
        amount: -2,
        flags: IPC_NOWAIT as i16,
    }];
    let too_much = [Operation {
        number: 0,
        amount: -2,
        flags: IPC_NOWAIT as i16,
    }];
    let both = [
        Operation {
            number: 1,
            amount: 1,
            flags: 0,
        },
        Operation {
            number: 0,
            amount: -5,
            flags: IPC_NOWAIT as i16,
        },
    ];
    let wait_zero = [Operation {
        number: 0,
        amount: 0,
        flags: IPC_NOWAIT as i16,
    }];
    let outside = [Operation {
        number: 2,
        amount: 1,
        flags: 0,
    }];
    let value = |number: u64| semaphore_control(set, number, 12, 0);
    let basics = semaphore_operate(set, &raise, None).is_ok()
        && value(0) == 3
        && semaphore_operate(set, &take, None).is_ok()
        && value(0) == 1
        && semaphore_operate(set, &too_much, None) == Err(error(11))
        && value(0) == 1;
    let atomic = semaphore_operate(set, &both, None) == Err(error(11)) && value(1) == 0;
    let zero = semaphore_operate(set, &wait_zero, None) == Err(error(11))
        && semaphore_control(set, 0, 16, 0) == 0
        && semaphore_operate(set, &wait_zero, None).is_ok();
    let range = semaphore_operate(set, &outside, None) == Err(error(27))
        && semaphore_control(set, 0, 16, 40_000) == error(34);
    let timed = semaphore_operate(
        set,
        &[Operation {
            number: 0,
            amount: -1,
            flags: 0,
        }],
        Some(0),
    ) == Err(error(11));
    let removed_set = semaphore_control(set, 0, IPC_RMID, 0) == 0
        && semaphore_operate(set, &raise, None) == Err(error(22));
    missing
        && found
        && exclusive
        && sent
        && bad_type
        && lowest
        && exact
        && except
        && empty
        && filled
        && full
        && removed
        && gone
        && basics
        && atomic
        && zero
        && range
        && timed
        && removed_set
        && shared_memory_self_test()
}
