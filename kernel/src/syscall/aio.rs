//! Linux AIO (`io_setup` .. `io_pgetevents`). Every request runs to the end
//! inside `io_submit` through the ordinary read, write and sync calls, and
//! its completion waits in the context's ring for `io_getevents`: what a
//! program sees is a device that is always fast.

use super::*;

const EINVAL: u64 = 22;
const EAGAIN: u64 = 11;
const EFAULT: u64 = 14;
const CONTEXTS: usize = 8;
const EVENTS: usize = 128;
const IOCB_BYTES: usize = 64;
const EVENT_BYTES: usize = 32;
const IOCB_FLAG_RESFD: u32 = 1;

#[derive(Clone, Copy)]
struct Event {
    data: u64,
    object: u64,
    result: i64,
}

#[derive(Clone, Copy)]
struct Context {
    id: u64,
    group: u64,
    capacity: usize,
    events: [Event; EVENTS],
    head: usize,
    count: usize,
}

const EMPTY_EVENT: Event = Event {
    data: 0,
    object: 0,
    result: 0,
};

const EMPTY: Context = Context {
    id: 0,
    group: 0,
    capacity: 0,
    events: [EMPTY_EVENT; EVENTS],
    head: 0,
    count: 0,
};

static CONTEXTS_TABLE: TicketLock<[Context; CONTEXTS]> = TicketLock::new([EMPTY; CONTEXTS]);
static NEXT_ID: AtomicU64 = AtomicU64::new(0x00a1_0000);

fn find(table: &mut [Context; CONTEXTS], id: u64) -> Option<&mut Context> {
    let group = crate::scheduler::current_group();
    table
        .iter_mut()
        .find(|context| context.id != 0 && context.id == id && context.group == group)
}

/// `io_setup(nr_events, ctx_idp)`.
pub(super) fn setup(requested: u64, id_address: u64) -> u64 {
    let mut current = [0u8; 8];
    if requested == 0 || requested > i32::MAX as u64 {
        return error(EINVAL);
    }
    if !user::copy_from_user(id_address, &mut current) {
        return error(EFAULT);
    }
    if u64::from_le_bytes(current) != 0 {
        return error(EINVAL);
    }
    let mut table = CONTEXTS_TABLE.lock();
    let Some(slot) = table.iter_mut().find(|context| context.id == 0) else {
        return error(EAGAIN);
    };
    let id = NEXT_ID.fetch_add(0x1000, Ordering::Relaxed);
    *slot = Context {
        id,
        group: crate::scheduler::current_group(),
        capacity: (requested as usize).min(EVENTS),
        ..EMPTY
    };
    if !user::copy_to_user(id_address, &id.to_le_bytes()) {
        *slot = EMPTY;
        return error(EFAULT);
    }
    0
}

/// `io_destroy(ctx)`.
pub(super) fn destroy(id: u64) -> u64 {
    let mut table = CONTEXTS_TABLE.lock();
    match find(&mut table, id) {
        Some(context) => {
            *context = EMPTY;
            0
        }
        None => error(EINVAL),
    }
}

/// Runs one control block; `Err` is the errno `io_submit` reports when it
/// is the first one.
fn run(block: &[u8; IOCB_BYTES]) -> Result<i64, u64> {
    let field = |at: usize| u64::from_le_bytes(block[at..at + 8].try_into().unwrap_or([0; 8]));
    let opcode = u16::from_le_bytes([block[16], block[17]]);
    let descriptor = u32::from_le_bytes(block[20..24].try_into().unwrap_or([0; 4])) as u64;
    let buffer = field(24);
    let length = field(32);
    let offset = field(40);
    let flags = u32::from_le_bytes(block[56..60].try_into().unwrap_or([0; 4]));
    if flags & IOCB_FLAG_RESFD != 0 || field(48) != 0 {
        return Err(EINVAL);
    }
    if (offset as i64) < 0 && matches!(opcode, 0 | 1 | 7 | 8) {
        return Err(EINVAL);
    }
    let result = match opcode {
        0 => linux_pread64(descriptor, buffer, length, offset),
        1 => linux_pwrite64(descriptor, buffer, length, offset),
        2 | 3 => linux_fsync(descriptor),
        6 => 0,
        7 => linux_preadv(descriptor, buffer, length, offset, 0),
        8 => linux_pwritev(descriptor, buffer, length, offset, 0),
        _ => return Err(EINVAL),
    };
    Ok(result as i64)
}

/// `io_submit(ctx, nr, iocbpp)`: the number of blocks taken.
pub(super) fn submit(id: u64, count: u64, list: u64) -> u64 {
    if (count as i64) < 0 {
        return error(EINVAL);
    }
    {
        let mut table = CONTEXTS_TABLE.lock();
        if find(&mut table, id).is_none() {
            return error(EINVAL);
        }
    }
    let mut taken = 0u64;
    while taken < count {
        let mut pointer = [0u8; 8];
        if !user::copy_from_user(list.wrapping_add(taken * 8), &mut pointer) {
            return if taken == 0 { error(EFAULT) } else { taken };
        }
        let object = u64::from_le_bytes(pointer);
        let mut block = [0u8; IOCB_BYTES];
        if !user::copy_from_user(object, &mut block) {
            return if taken == 0 { error(EFAULT) } else { taken };
        }
        {
            let mut table = CONTEXTS_TABLE.lock();
            let Some(context) = find(&mut table, id) else {
                return if taken == 0 { error(EINVAL) } else { taken };
            };
            if context.count >= context.capacity {
                return if taken == 0 { error(EAGAIN) } else { taken };
            }
        }
        // The I/O runs without the table lock: it may sleep or fault.
        let result = match run(&block) {
            Ok(result) => result,
            Err(errno) => return if taken == 0 { error(errno) } else { taken },
        };
        let mut table = CONTEXTS_TABLE.lock();
        let Some(context) = find(&mut table, id) else {
            return taken;
        };
        let slot = (context.head + context.count) % EVENTS;
        context.events[slot] = Event {
            data: u64::from_le_bytes(block[..8].try_into().unwrap_or([0; 8])),
            object,
            result,
        };
        context.count += 1;
        taken += 1;
    }
    taken
}

/// `io_getevents` / `io_pgetevents`: everything submitted has already
/// finished, so this hands back what is waiting without blocking.
pub(super) fn get_events(id: u64, minimum: u64, wanted: u64, events: u64) -> u64 {
    if (minimum as i64) < 0 || (wanted as i64) < 0 || minimum > wanted {
        return error(EINVAL);
    }
    let mut table = CONTEXTS_TABLE.lock();
    let Some(context) = find(&mut table, id) else {
        return error(EINVAL);
    };
    let mut delivered = 0u64;
    while delivered < wanted && context.count > 0 {
        let event = context.events[context.head];
        let mut bytes = [0u8; EVENT_BYTES];
        bytes[..8].copy_from_slice(&event.data.to_le_bytes());
        bytes[8..16].copy_from_slice(&event.object.to_le_bytes());
        bytes[16..24].copy_from_slice(&event.result.to_le_bytes());
        if !user::copy_to_user(events.wrapping_add(delivered * EVENT_BYTES as u64), &bytes) {
            return if delivered == 0 { error(EFAULT) } else { delivered };
        }
        context.head = (context.head + 1) % EVENTS;
        context.count -= 1;
        delivered += 1;
    }
    delivered
}

/// `io_cancel`: nothing is ever still in flight.
pub(super) fn cancel(id: u64) -> u64 {
    let mut table = CONTEXTS_TABLE.lock();
    if find(&mut table, id).is_none() {
        return error(EINVAL);
    }
    error(EAGAIN)
}
