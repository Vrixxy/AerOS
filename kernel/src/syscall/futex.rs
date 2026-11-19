//! Futexes: wait queues on user words, shared by the threads of one process.
//! A waiter registers under (process, address) and gives up the CPU until a
//! wake-up, its time-out, or a signal ends the wait. Wake, requeue and
//! wake-op work on those queues; the priority-inheritance calls hand a lock
//! over in FIFO order and raise the owner's priority while a more important
//! task waits for it; robust lists are walked when a thread exits.

use super::*;

const WAITERS: usize = 64;
const WAITERS_BIT: u32 = 0x8000_0000;
const OWNER_DIED: u32 = 0x4000_0000;
const TID_MASK: u32 = 0x3fff_ffff;
const CLOCK_REALTIME_FLAG: u64 = 0x100;
const ROBUST_LIMIT: usize = 2048;

#[derive(Clone, Copy)]
struct Waiter {
    used: bool,
    group: u64,
    address: u64,
    bitset: u32,
    task: u64,
    woken: bool,
    /// The lock owner a priority-inheritance waiter waits for (0: plain wait).
    owner: u32,
    /// The owner's nice value before this waiter raised it.
    saved_nice: i8,
    sequence: u64,
}

impl Waiter {
    const EMPTY: Waiter = Waiter {
        used: false,
        group: 0,
        address: 0,
        bitset: 0,
        task: 0,
        woken: false,
        owner: 0,
        saved_nice: 0,
        sequence: 0,
    };
}

struct Table {
    waiters: [Waiter; WAITERS],
    sequence: u64,
}

static TABLE: TicketLock<Table> = TicketLock::new(Table {
    waiters: [Waiter::EMPTY; WAITERS],
    sequence: 0,
});

fn read_word(address: u64) -> Option<u32> {
    let mut bytes = [0u8; 4];
    user::copy_from_user(address, &mut bytes).then(|| u32::from_le_bytes(bytes))
}

fn write_word(address: u64, value: u32) -> bool {
    user::copy_to_user(address, &value.to_le_bytes())
}

/// Adds a waiter and returns its slot, dropping entries of tasks that are gone.
fn enqueue(group: u64, address: u64, bitset: u32, owner: u32, saved_nice: i8) -> Option<usize> {
    let task = crate::scheduler::current_tid();
    let mut table = TABLE.lock();
    for waiter in table.waiters.iter_mut() {
        if waiter.used && !crate::scheduler::task_is_live(waiter.task) {
            *waiter = Waiter::EMPTY;
        }
    }
    let slot = table.waiters.iter().position(|waiter| !waiter.used)?;
    table.sequence += 1;
    let sequence = table.sequence;
    table.waiters[slot] = Waiter {
        used: true,
        group,
        address,
        bitset,
        task,
        woken: false,
        owner,
        saved_nice,
        sequence,
    };
    Some(slot)
}

fn dequeue(slot: usize) {
    TABLE.lock().waiters[slot] = Waiter::EMPTY;
}

fn woken(slot: usize) -> bool {
    TABLE.lock().waiters[slot].woken
}

/// Wakes up to `count` plain waiters on the word, oldest first, whose bitset
/// shares a bit with `bitset`; returns how many.
pub(super) fn wake(group: u64, address: u64, count: usize, bitset: u32) -> usize {
    let mut table = TABLE.lock();
    let mut woken = 0;
    while woken < count {
        let next = table
            .waiters
            .iter()
            .enumerate()
            .filter(|(_, waiter)| {
                waiter.used
                    && !waiter.woken
                    && waiter.owner == 0
                    && waiter.group == group
                    && waiter.address == address
                    && waiter.bitset & bitset != 0
                    && crate::scheduler::task_is_live(waiter.task)
            })
            .min_by_key(|(_, waiter)| waiter.sequence)
            .map(|(index, _)| index);
        let Some(index) = next else { break };
        table.waiters[index].woken = true;
        woken += 1;
    }
    woken
}

fn requeue(from: (u64, u64), to: (u64, u64), wake_count: usize, move_count: usize) -> usize {
    let woken = wake(from.0, from.1, wake_count, u32::MAX);
    let mut table = TABLE.lock();
    let mut moved = 0;
    for waiter in table.waiters.iter_mut() {
        if moved == move_count {
            break;
        }
        if waiter.used
            && !waiter.woken
            && waiter.owner == 0
            && waiter.group == from.0
            && waiter.address == from.1
            && crate::scheduler::task_is_live(waiter.task)
        {
            waiter.group = to.0;
            waiter.address = to.1;
            moved += 1;
        }
    }
    woken + moved
}

/// Blocks until the waiter in `slot` is woken, the deadline passes (EAGAIN
/// style codes follow futex(2): ETIMEDOUT) or a signal arrives (EINTR).
fn block(slot: usize, deadline: Option<u64>) -> u64 {
    loop {
        yield_in_syscall();
        if woken(slot) {
            dequeue(slot);
            return 0;
        }
        if interrupted() {
            dequeue(slot);
            return error(4);
        }
        if deadline.is_some_and(|limit| crate::time::monotonic_nanoseconds() >= limit) {
            dequeue(slot);
            return error(110);
        }
    }
}

/// The absolute CLOCK_MONOTONIC time a `timespec` at `address` stands for;
/// `absolute` says the value is a time rather than a length of time.
fn deadline(address: u64, absolute: bool, realtime: bool) -> Result<Option<u64>, u64> {
    if address == 0 {
        return Ok(None);
    }
    let mut raw = [0u8; 16];
    if !user::copy_from_user(address, &mut raw) {
        return Err(error(14));
    }
    let seconds = i64::from_le_bytes(raw[..8].try_into().map_err(|_| error(22))?);
    let nanoseconds = i64::from_le_bytes(raw[8..].try_into().map_err(|_| error(22))?);
    if seconds < 0 || !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(error(22));
    }
    let value = (seconds as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nanoseconds as u64);
    let now = crate::time::monotonic_nanoseconds();
    Ok(Some(if !absolute {
        now.saturating_add(value)
    } else if realtime {
        let wall = crate::rtc::unix_nanoseconds().min(u128::from(u64::MAX)) as u64;
        now.saturating_add(value.saturating_sub(wall))
    } else {
        value
    }))
}

fn count_of(value: u64) -> usize {
    let signed = value as u32 as i32;
    if signed <= 0 { 0 } else { signed as usize }
}

/// What a futex word is known by: its address in this process, or, for a
/// shared futex (no `FUTEX_PRIVATE_FLAG`) in a shared mapping, its physical
/// address, which every process mapping it agrees on (group 0).
fn key(operation: u64, group: u64, address: u64) -> (u64, u64) {
    if operation & 0x80 == 0
        && let Some(physical) = crate::scheduler::shared_physical_current(address)
    {
        return (0, physical);
    }
    (group, address)
}

pub(super) fn operate(arguments: [u64; 6]) -> u64 {
    let [address, operation, value, timeout, address2, value3] = arguments;
    FUTEX_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    if operation & !(0x7f | 0x80 | CLOCK_REALTIME_FLAG) != 0 {
        return error(22);
    }
    if address & 3 != 0 {
        return error(22);
    }
    if !user::range_accessible(address, 4, true) {
        return error(14);
    }
    let group = crate::scheduler::current_group();
    let (word_group, word) = key(operation, group, address);
    let realtime = operation & CLOCK_REALTIME_FLAG != 0;
    match operation & 0x7f {
        0 | 9 => {
            let bitset = if operation & 0x7f == 9 {
                value3 as u32
            } else {
                u32::MAX
            };
            if bitset == 0 {
                return error(22);
            }
            let Some(observed) = read_word(address) else {
                return error(14);
            };
            if observed != value as u32 {
                return error(11);
            }
            let limit = match deadline(timeout, operation & 0x7f == 9, realtime) {
                Ok(limit) => limit,
                Err(failure) => return failure,
            };
            let Some(slot) = enqueue(word_group, word, bitset, 0, 0) else {
                return error(12);
            };
            block(slot, limit)
        }
        1 | 10 => {
            let bitset = if operation & 0x7f == 10 {
                value3 as u32
            } else {
                u32::MAX
            };
            if bitset == 0 {
                return error(22);
            }
            wake(word_group, word, count_of(value), bitset) as u64
        }
        3 | 4 => {
            if address2 & 3 != 0 || !user::range_accessible(address2, 4, true) {
                return error(if address2 & 3 != 0 { 22 } else { 14 });
            }
            if operation & 0x7f == 4 {
                match read_word(address) {
                    Some(observed) if observed == value3 as u32 => {}
                    Some(_) => return error(11),
                    None => return error(14),
                }
            }
            requeue(
                (word_group, word),
                key(operation, group, address2),
                count_of(value),
                count_of(timeout),
            ) as u64
        }
        5 => {
            if address2 & 3 != 0 || !user::range_accessible(address2, 4, true) {
                return error(if address2 & 3 != 0 { 22 } else { 14 });
            }
            let encoded = value3 as u32;
            let operation_code = (encoded >> 28) & 7;
            let comparison = (encoded >> 24) & 15;
            let mut argument = (((encoded >> 12) & 0xfff) as i32) << 20 >> 20;
            let comparand = ((encoded & 0xfff) as i32) << 20 >> 20;
            if encoded & 0x8000_0000 != 0 {
                argument = 1i32.wrapping_shl(argument as u32 & 31);
            }
            let Some(old) = read_word(address2) else {
                return error(14);
            };
            let old_signed = old as i32;
            let updated = match operation_code {
                0 => argument,
                1 => old_signed.wrapping_add(argument),
                2 => old_signed | argument,
                3 => old_signed & !argument,
                4 => old_signed ^ argument,
                _ => return error(38),
            };
            if !write_word(address2, updated as u32) {
                return error(14);
            }
            let holds = match comparison {
                0 => old_signed == comparand,
                1 => old_signed != comparand,
                2 => old_signed < comparand,
                3 => old_signed <= comparand,
                4 => old_signed > comparand,
                5 => old_signed >= comparand,
                _ => return error(38),
            };
            let mut woken = wake(word_group, word, count_of(value), u32::MAX);
            if holds {
                let (second_group, second) = key(operation, group, address2);
                woken += wake(second_group, second, count_of(timeout), u32::MAX);
            }
            woken as u64
        }
        6 => lock_pi(group, address, timeout, realtime),
        7 => unlock_pi(group, address),
        8 => {
            let me = crate::scheduler::current_tid() as u32;
            match read_word(address) {
                Some(0) => {
                    if write_word(address, me) {
                        0
                    } else {
                        error(14)
                    }
                }
                Some(word) if word & TID_MASK == me => error(35),
                Some(_) => error(11),
                None => error(14),
            }
        }
        _ => error(38),
    }
}

fn lock_pi(group: u64, address: u64, timeout: u64, realtime: bool) -> u64 {
    let me = crate::scheduler::current_tid() as u32;
    let limit = match deadline(timeout, true, realtime) {
        Ok(limit) => limit,
        Err(failure) => return failure,
    };
    let mut waited = false;
    loop {
        let Some(word) = read_word(address) else {
            return error(14);
        };
        let owner = word & TID_MASK;
        if owner == 0 || !crate::scheduler::task_is_live(u64::from(owner)) {
            let kept = if owner == 0 { 0 } else { OWNER_DIED };
            if write_word(address, me | kept | (word & WAITERS_BIT)) {
                return 0;
            }
            return error(14);
        }
        if owner == me {
            return if waited { 0 } else { error(35) };
        }
        // Only a thread of this process has its priority raised: the word is
        // the program's, and could name any task.
        let my_nice = crate::scheduler::nice_of(u64::from(me)).unwrap_or(0);
        let saved = if crate::scheduler::group_of_tid(u64::from(owner)) == Some(group) {
            crate::scheduler::boost_nice(u64::from(owner), my_nice).unwrap_or(0)
        } else {
            crate::scheduler::nice_of(u64::from(owner)).unwrap_or(0)
        };
        if !write_word(address, word | WAITERS_BIT) {
            return error(14);
        }
        let Some(slot) = enqueue(group, address, u32::MAX, owner, saved) else {
            return error(12);
        };
        match block(slot, limit) {
            0 => waited = true,
            failure => {
                crate::scheduler::restore_nice(u64::from(owner), saved);
                return failure;
            }
        }
    }
}

fn unlock_pi(group: u64, address: u64) -> u64 {
    let me = crate::scheduler::current_tid() as u32;
    let Some(word) = read_word(address) else {
        return error(14);
    };
    if word & TID_MASK != me {
        return error(1);
    }
    let mut table = TABLE.lock();
    let mut first: Option<usize> = None;
    let mut original = None::<i8>;
    let mut waiting = 0;
    for (index, waiter) in table.waiters.iter().enumerate() {
        if waiter.used
            && !waiter.woken
            && waiter.owner == me
            && waiter.group == group
            && waiter.address == address
            && crate::scheduler::task_is_live(waiter.task)
        {
            waiting += 1;
            original = Some(original.map_or(waiter.saved_nice, |nice| nice.max(waiter.saved_nice)));
            if first.is_none_or(|current| waiter.sequence < table.waiters[current].sequence) {
                first = Some(index);
            }
        }
    }
    let new_word = match first {
        Some(index) => {
            table.waiters[index].woken = true;
            let next = table.waiters[index].task as u32;
            for waiter in table.waiters.iter_mut() {
                if waiter.used && !waiter.woken && waiter.owner == me && waiter.address == address {
                    waiter.owner = next;
                }
            }
            next | if waiting > 1 { WAITERS_BIT } else { 0 }
        }
        None => 0,
    };
    drop(table);
    if let Some(nice) = original {
        crate::scheduler::restore_nice(u64::from(me), nice);
    }
    if write_word(address, new_word) {
        0
    } else {
        error(14)
    }
}

/// Marks the futexes a dying thread still holds as abandoned and wakes
/// someone waiting on each, by walking its robust list (`head` is the
/// `robust_list_head`).
pub(super) fn robust_cleanup(head: u64, tid: u32, group: u64) {
    let mut raw = [0u8; 24];
    if !user::copy_from_user(head, &mut raw) {
        return;
    }
    let first = u64::from_le_bytes(raw[..8].try_into().unwrap_or([0; 8]));
    let offset = i64::from_le_bytes(raw[8..16].try_into().unwrap_or([0; 8]));
    let pending = u64::from_le_bytes(raw[16..24].try_into().unwrap_or([0; 8]));
    let abandon = |entry: u64| {
        let address = entry.wrapping_add(offset as u64);
        let Some(word) = read_word(address) else {
            return;
        };
        if word & TID_MASK != tid {
            return;
        }
        if write_word(address, (word & WAITERS_BIT) | OWNER_DIED) && word & WAITERS_BIT != 0 {
            wake(group, address, 1, u32::MAX);
        }
    };
    let mut entry = first;
    let mut visited = 0;
    while entry != head && entry != 0 && visited < ROBUST_LIMIT {
        if entry != pending {
            abandon(entry);
        }
        let mut next = [0u8; 8];
        if !user::copy_from_user(entry, &mut next) {
            break;
        }
        entry = u64::from_le_bytes(next);
        visited += 1;
    }
    if pending != 0 {
        abandon(pending);
    }
}

// The futex2 calls (Linux 5.16 / 6.7): 32-bit words only, as in Linux today.
const FUTEX2_SIZE_MASK: u64 = 3;
const FUTEX2_SIZE_U32: u64 = 2;
const FUTEX2_PRIVATE: u64 = 0x80;
const WAITV_LIMIT: usize = 128;

fn futex2_operation(flags: u64, base: u64) -> Result<u64, u64> {
    if flags & !(FUTEX2_SIZE_MASK | FUTEX2_PRIVATE) != 0
        || flags & FUTEX2_SIZE_MASK != FUTEX2_SIZE_U32
    {
        return Err(error(22));
    }
    Ok(base | (flags & FUTEX2_PRIVATE))
}

fn clock_flag(clock: u64) -> Result<u64, u64> {
    match clock {
        0 => Ok(CLOCK_REALTIME_FLAG),
        1 => Ok(0),
        _ => Err(error(22)),
    }
}

/// `futex_wake(uaddr, mask, nr, flags)`.
pub(super) fn wake2(address: u64, mask: u64, count: u64, flags: u64) -> u64 {
    match futex2_operation(flags, 10) {
        Ok(operation) => operate([address, operation, count, 0, 0, mask]),
        Err(failure) => failure,
    }
}

/// `futex_wait(uaddr, val, mask, flags, timeout, clockid)`; the time-out is
/// absolute on `clockid`.
pub(super) fn wait2(
    address: u64,
    value: u64,
    mask: u64,
    flags: u64,
    timeout: u64,
    clock: u64,
) -> u64 {
    if value > u64::from(u32::MAX) {
        return error(22);
    }
    let operation = match futex2_operation(flags, 9) {
        Ok(operation) => operation,
        Err(failure) => return failure,
    };
    let clock = match clock_flag(clock) {
        Ok(flag) => flag,
        Err(failure) if timeout != 0 => return failure,
        Err(_) => 0,
    };
    operate([address, operation | clock, value, timeout, 0, mask])
}

struct WaitEntry {
    value: u64,
    address: u64,
    flags: u64,
}

fn waitv_entry(array: u64, index: usize) -> Result<WaitEntry, u64> {
    let mut raw = [0u8; 24];
    if !user::copy_from_user(array.wrapping_add(index as u64 * 24), &mut raw) {
        return Err(error(14));
    }
    let entry = WaitEntry {
        value: u64::from_le_bytes(raw[..8].try_into().unwrap_or_default()),
        address: u64::from_le_bytes(raw[8..16].try_into().unwrap_or_default()),
        flags: u64::from(u32::from_le_bytes(
            raw[16..20].try_into().unwrap_or_default(),
        )),
    };
    if raw[20..24] != [0; 4] || entry.value > u64::from(u32::MAX) || entry.address & 3 != 0 {
        return Err(error(22));
    }
    futex2_operation(entry.flags, 0)?;
    Ok(entry)
}

/// `futex_requeue(waiters[2], flags, nr_wake, nr_requeue)`: wakes from the
/// first word and moves the rest to the second, if the first still holds
/// its expected value.
pub(super) fn requeue2(waiters: u64, flags: u64, wake_count: u64, move_count: u64) -> u64 {
    if flags != 0 {
        return error(22);
    }
    let (from, to) = match (waitv_entry(waiters, 0), waitv_entry(waiters, 1)) {
        (Ok(from), Ok(to)) => (from, to),
        (Err(failure), _) | (_, Err(failure)) => return failure,
    };
    if !user::range_accessible(to.address, 4, true) {
        return error(14);
    }
    let operation = 4 | (from.flags & FUTEX2_PRIVATE);
    operate([
        from.address,
        operation,
        wake_count,
        move_count,
        to.address,
        from.value,
    ])
}

/// `futex_waitv(waiters, nr, flags, timeout, clockid)`: sleeps until any of
/// up to 128 words is woken and returns its index; EAGAIN when one already
/// differs from its expected value.
pub(super) fn waitv(waiters: u64, count: u64, flags: u64, timeout: u64, clock: u64) -> u64 {
    FUTEX_CALLS.fetch_add(1, Ordering::Relaxed);
    let count = count as usize;
    if flags != 0 || count == 0 || count > WAITV_LIMIT {
        return error(22);
    }
    let realtime = match clock_flag(clock) {
        Ok(flag) => flag != 0,
        Err(failure) => return failure,
    };
    let limit = match deadline(timeout, true, realtime) {
        Ok(limit) => limit,
        Err(failure) => return failure,
    };
    let group = crate::scheduler::current_group();
    let mut slots = [usize::MAX; WAITV_LIMIT];
    let release = |slots: &[usize]| {
        for slot in slots.iter().copied().filter(|slot| *slot != usize::MAX) {
            dequeue(slot);
        }
    };
    for index in 0..count {
        let entry = match waitv_entry(waiters, index) {
            Ok(entry) => entry,
            Err(failure) => {
                release(&slots[..index]);
                return failure;
            }
        };
        if !user::range_accessible(entry.address, 4, true) {
            release(&slots[..index]);
            return error(14);
        }
        let (word_group, word) = key(entry.flags & FUTEX2_PRIVATE, group, entry.address);
        let Some(slot) = enqueue(word_group, word, u32::MAX, 0, 0) else {
            release(&slots[..index]);
            return error(12);
        };
        slots[index] = slot;
        match read_word(entry.address) {
            Some(observed) if u64::from(observed) == entry.value => {}
            Some(_) => {
                release(&slots[..=index]);
                return error(11);
            }
            None => {
                release(&slots[..=index]);
                return error(14);
            }
        }
    }
    loop {
        yield_in_syscall();
        if let Some(index) = slots[..count].iter().position(|slot| woken(*slot)) {
            release(&slots[..count]);
            return index as u64;
        }
        if interrupted() {
            release(&slots[..count]);
            return error(4);
        }
        if limit.is_some_and(|limit| crate::time::monotonic_nanoseconds() >= limit) {
            release(&slots[..count]);
            return error(110);
        }
    }
}
