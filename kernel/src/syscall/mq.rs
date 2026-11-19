//! POSIX message queues (`mq_open` .. `mq_getsetattr`). A queue lives until
//! it is unlinked and no descriptor still names it; its descriptor is a
//! slot in the pipe table marked with the queue. Messages leave highest
//! priority first, in order of arrival within a priority.

use super::*;

pub(super) const READ: u8 = 1;
pub(super) const WRITE: u8 = 2;
pub(super) const NONBLOCK: u8 = 4;
const QUEUES: usize = 8;
const MESSAGES: usize = 16;
const MESSAGE_BYTES: usize = 1024;
const NAME: usize = 48;
const O_CREAT: u64 = 0o100;
const O_EXCL: u64 = 0o200;
const O_NONBLOCK: u64 = 0o4000;
const O_CLOEXEC: u64 = 0o2_000_000;
const SIGEV_SIGNAL: u32 = 0;
const SIGEV_NONE: u32 = 1;

#[derive(Clone, Copy)]
struct Message {
    priority: u32,
    length: usize,
    data: [u8; MESSAGE_BYTES],
}

#[derive(Clone, Copy)]
struct Queue {
    used: bool,
    name: [u8; NAME],
    name_len: usize,
    capacity: usize,
    message_size: usize,
    messages: [Message; MESSAGES],
    count: usize,
    notify_pid: u64,
    notify_signal: u64,
}

const EMPTY_MESSAGE: Message = Message {
    priority: 0,
    length: 0,
    data: [0; MESSAGE_BYTES],
};

const EMPTY: Queue = Queue {
    used: false,
    name: [0; NAME],
    name_len: 0,
    capacity: 0,
    message_size: 0,
    messages: [EMPTY_MESSAGE; MESSAGES],
    count: 0,
    notify_pid: 0,
    notify_signal: 0,
};

static TABLE: TicketLock<[Queue; QUEUES]> = TicketLock::new([EMPTY; QUEUES]);

/// A slot no name and no descriptor refers to any more.
fn reclaim(table: &mut [Queue; QUEUES]) -> Option<usize> {
    if let Some(free) = table.iter().position(|queue| !queue.used) {
        return Some(free);
    }
    let pipes = PIPES.lock();
    let index = table.iter().enumerate().position(|(index, queue)| {
        queue.name_len == 0
            && !pipes
                .iter()
                .any(|pipe| pipe.used && pipe.mqueue as usize == index + 1)
    })?;
    table[index].used = false;
    Some(index)
}

/// The queue and the access bits behind a descriptor.
fn descriptor(fd: u64) -> Result<(usize, u8), u64> {
    let process_fd = lookup_process_fd(fd).ok_or(error(9))?;
    if !process_fd.pipe {
        return Err(error(9));
    }
    match PIPES.lock().get(process_fd.handle as usize) {
        Some(pipe) if pipe.used && pipe.mqueue != 0 => {
            Ok((pipe.mqueue as usize - 1, pipe.mq_flags))
        }
        _ => Err(error(9)),
    }
}

fn read_name(address: u64, out: &mut [u8; NAME]) -> Result<usize, u64> {
    let mut buffer = [0u8; NAME + 1];
    let length = user::copy_string(address, &mut buffer).ok_or(error(14))?;
    if length == 0 {
        return Err(error(2));
    }
    if length > NAME {
        return Err(error(36));
    }
    if buffer[..length].contains(&b'/') {
        return Err(error(13));
    }
    out[..length].copy_from_slice(&buffer[..length]);
    Ok(length)
}

/// `mq_open(name, oflag, mode, attr)`; the C library has already taken the
/// leading slash off the name.
pub(super) fn open(name_address: u64, flags: u64, _mode: u64, attributes: u64) -> u64 {
    let mut name = [0u8; NAME];
    let name_len = match read_name(name_address, &mut name) {
        Ok(len) => len,
        Err(failure) => return failure,
    };
    let access = flags & 3;
    if access == 3 {
        return error(22);
    }
    let (mut capacity, mut message_size) = (10, MESSAGE_BYTES);
    if flags & O_CREAT != 0 && attributes != 0 {
        let mut attr = [0u8; 32];
        if !user::copy_from_user(attributes, &mut attr) {
            return error(14);
        }
        let maximum = i64::from_le_bytes(attr[8..16].try_into().unwrap_or([0; 8]));
        let size = i64::from_le_bytes(attr[16..24].try_into().unwrap_or([0; 8]));
        if maximum <= 0 || size <= 0 || maximum > MESSAGES as i64 || size > MESSAGE_BYTES as i64 {
            return error(22);
        }
        capacity = maximum as usize;
        message_size = size as usize;
    }
    let index = {
        let mut table = TABLE.lock();
        let existing = table
            .iter()
            .position(|queue| queue.used && queue.name[..queue.name_len] == name[..name_len]);
        match existing {
            Some(_) if flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL => return error(17),
            Some(index) => index,
            None if flags & O_CREAT == 0 => return error(2),
            None => {
                let Some(index) = reclaim(&mut table) else {
                    return error(28);
                };
                // Field by field: a whole queue is too big for the stack.
                let queue = &mut table[index];
                queue.used = true;
                queue.name = name;
                queue.name_len = name_len;
                queue.capacity = capacity;
                queue.message_size = message_size;
                queue.count = 0;
                queue.notify_pid = 0;
                queue.notify_signal = 0;
                index
            }
        }
    };
    let mut bits = 0;
    if access != 1 {
        bits |= READ;
    }
    if access != 0 {
        bits |= WRITE;
    }
    if flags & O_NONBLOCK != 0 {
        bits |= NONBLOCK;
    }
    let template = Pipe {
        mqueue: index as u16 + 1,
        mq_flags: bits,
        ..Pipe::EMPTY
    };
    match create_single_fd_pipe(template, flags & O_CLOEXEC != 0, false) {
        Ok(fd) => fd,
        Err(failure) => error(failure),
    }
}

/// `mq_unlink(name)`: the name goes at once, the queue with its last
/// descriptor.
pub(super) fn unlink(name_address: u64) -> u64 {
    let mut name = [0u8; NAME];
    let name_len = match read_name(name_address, &mut name) {
        Ok(len) => len,
        Err(failure) => return failure,
    };
    let mut table = TABLE.lock();
    match table
        .iter_mut()
        .find(|queue| queue.used && queue.name_len != 0 && queue.name[..queue.name_len] == name[..name_len])
    {
        Some(queue) => {
            queue.name_len = 0;
            0
        }
        None => error(2),
    }
}

/// Waits for `ready` until `deadline` (realtime nanoseconds, 0 for none).
fn wait_turn(nonblocking: bool, deadline: u128) -> Result<(), u64> {
    if nonblocking {
        return Err(error(11));
    }
    if deadline != 0 && crate::rtc::unix_nanoseconds() >= deadline {
        return Err(error(110));
    }
    if interrupted() {
        return Err(error(4));
    }
    yield_in_syscall();
    Ok(())
}

fn deadline_of(address: u64) -> Result<u128, u64> {
    if address == 0 {
        return Ok(0);
    }
    read_timespec(address).map(u128::from).ok_or(error(22))
}

/// `mq_timedsend(mqd, msg, len, prio, abs_timeout)`.
pub(super) fn send(fd: u64, message: u64, length: u64, priority: u64, timeout: u64) -> u64 {
    let (index, bits) = match descriptor(fd) {
        Ok(found) => found,
        Err(failure) => return failure,
    };
    if bits & WRITE == 0 {
        return error(9);
    }
    if priority >= 32_768 {
        return error(22);
    }
    let mut data = [0u8; MESSAGE_BYTES];
    if length as usize > MESSAGE_BYTES {
        return error(90);
    }
    if length != 0 && !user::copy_from_user(message, &mut data[..length as usize]) {
        return error(14);
    }
    let deadline = match deadline_of(timeout) {
        Ok(deadline) => deadline,
        Err(failure) => return failure,
    };
    loop {
        let notify = {
            let mut table = TABLE.lock();
            let queue = &mut table[index];
            if length as usize > queue.message_size {
                return error(90);
            }
            if queue.count < queue.capacity {
                let was_empty = queue.count == 0;
                let slot = &mut queue.messages[queue.count];
                slot.priority = priority as u32;
                slot.length = length as usize;
                slot.data = data;
                queue.count += 1;
                let target = (was_empty && queue.notify_pid != 0)
                    .then_some((queue.notify_pid, queue.notify_signal));
                if target.is_some() {
                    queue.notify_pid = 0;
                }
                Some(target)
            } else {
                None
            }
        };
        if let Some(target) = notify {
            if let Some((pid, signal)) = target
                && signal != 0
            {
                let own = crate::scheduler::current_task_pid_for_linux()
                    .unwrap_or_else(crate::process::current_pid);
                if matches!(
                    crate::scheduler::signal_other_task(pid, signal),
                    Some(RemoteSignal::Fatal)
                ) && pid != own
                {
                    let _ = crate::scheduler::kill_task_with(pid, signal as u8);
                }
            }
            return 0;
        }
        if let Err(failure) = wait_turn(bits & NONBLOCK != 0, deadline) {
            return failure;
        }
    }
}

/// `mq_timedreceive(mqd, msg, len, prio, abs_timeout)`: the message's
/// length.
pub(super) fn receive(fd: u64, buffer: u64, capacity: u64, priority: u64, timeout: u64) -> u64 {
    let (index, bits) = match descriptor(fd) {
        Ok(found) => found,
        Err(failure) => return failure,
    };
    if bits & READ == 0 {
        return error(9);
    }
    let deadline = match deadline_of(timeout) {
        Ok(deadline) => deadline,
        Err(failure) => return failure,
    };
    loop {
        let taken = {
            let mut table = TABLE.lock();
            let queue = &mut table[index];
            if (capacity as usize) < queue.message_size {
                return error(90);
            }
            // Highest priority; the earliest of those.
            let best = (0..queue.count).max_by_key(|at| (queue.messages[*at].priority, usize::MAX - at));
            best.map(|at| {
                let message = queue.messages[at];
                queue.messages.copy_within(at + 1..queue.count, at);
                queue.count -= 1;
                message
            })
        };
        if let Some(message) = taken {
            if !user::copy_to_user(buffer, &message.data[..message.length]) {
                return error(14);
            }
            if priority != 0 && !user::copy_to_user(priority, &message.priority.to_le_bytes()) {
                return error(14);
            }
            return message.length as u64;
        }
        if let Err(failure) = wait_turn(bits & NONBLOCK != 0, deadline) {
            return failure;
        }
    }
}

/// `mq_notify(mqd, sevp)`: a signal when a message arrives on an empty
/// queue, once; a null `sevp` takes the registration back.
pub(super) fn notify(fd: u64, event: u64) -> u64 {
    let index = match descriptor(fd) {
        Ok((index, _)) => index,
        Err(failure) => return failure,
    };
    let own =
        crate::scheduler::current_task_pid_for_linux().unwrap_or_else(crate::process::current_pid);
    let mut table = TABLE.lock();
    let queue = &mut table[index];
    if event == 0 {
        if queue.notify_pid == own {
            queue.notify_pid = 0;
        }
        return 0;
    }
    let mut sigevent = [0u8; 16];
    if !user::copy_from_user(event, &mut sigevent) {
        return error(14);
    }
    let signal = u32::from_le_bytes(sigevent[8..12].try_into().unwrap_or([0; 4]));
    let kind = u32::from_le_bytes(sigevent[12..16].try_into().unwrap_or([0; 4]));
    if queue.notify_pid != 0 && queue.notify_pid != own {
        return error(16);
    }
    match kind {
        SIGEV_NONE => {
            queue.notify_pid = own;
            queue.notify_signal = 0;
            0
        }
        SIGEV_SIGNAL if (1..=64).contains(&signal) => {
            queue.notify_pid = own;
            queue.notify_signal = u64::from(signal);
            0
        }
        // SIGEV_THREAD needs a netlink socket from the C library.
        _ => error(22),
    }
}

/// `mq_getsetattr(mqd, newattr, oldattr)`: only O_NONBLOCK can change.
pub(super) fn get_set_attributes(fd: u64, new: u64, old: u64) -> u64 {
    let process_fd = match lookup_process_fd(fd) {
        Some(found) if found.pipe => found,
        _ => return error(9),
    };
    let (index, bits) = match descriptor(fd) {
        Ok(found) => found,
        Err(failure) => return failure,
    };
    let mut wanted = None;
    if new != 0 {
        let mut attr = [0u8; 32];
        if !user::copy_from_user(new, &mut attr) {
            return error(14);
        }
        let flags = u64::from_le_bytes(attr[..8].try_into().unwrap_or([0; 8]));
        if flags & !O_NONBLOCK != 0 {
            return error(22);
        }
        wanted = Some(flags & O_NONBLOCK != 0);
    }
    if old != 0 {
        let (capacity, size, count) = {
            let table = TABLE.lock();
            let queue = &table[index];
            (queue.capacity, queue.message_size, queue.count)
        };
        let mut attr = [0u8; 32];
        let flags = if bits & NONBLOCK != 0 { O_NONBLOCK } else { 0 };
        attr[..8].copy_from_slice(&flags.to_le_bytes());
        attr[8..16].copy_from_slice(&(capacity as u64).to_le_bytes());
        attr[16..24].copy_from_slice(&(size as u64).to_le_bytes());
        attr[24..32].copy_from_slice(&(count as u64).to_le_bytes());
        if !user::copy_to_user(old, &attr) {
            return error(14);
        }
    }
    if let Some(nonblocking) = wanted
        && let Some(pipe) = PIPES.lock().get_mut(process_fd.handle as usize)
    {
        if nonblocking {
            pipe.mq_flags |= NONBLOCK;
        } else {
            pipe.mq_flags &= !NONBLOCK;
        }
    }
    0
}
