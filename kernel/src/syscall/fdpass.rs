//! Descriptors passed over AF_UNIX connections (`SCM_RIGHTS`): `sendmsg`
//! copies the sender's descriptors into a bundle queued for the other end,
//! and the next `recvmsg` there installs them. A queued copy holds what it
//! refers to open, as the sender's own descriptor did.

use super::*;

const BUNDLES: usize = 16;
/// The most descriptors one message carries (Linux allows 253).
const PER_MESSAGE: usize = 16;
const SOL_SOCKET: u32 = 1;
const SCM_RIGHTS: u32 = 1;
const MSG_CTRUNC: u32 = 0x8;

#[derive(Clone, Copy)]
pub(super) struct Bundle {
    used: bool,
    pair: u32,
    /// Read by the pair's end B (written by end A), or the other way round.
    to_end_b: bool,
    fds: [ProcessFd; PER_MESSAGE],
    count: usize,
}

impl Bundle {
    const EMPTY: Self = Self {
        used: false,
        pair: 0,
        to_end_b: false,
        fds: [ProcessFd::EMPTY; PER_MESSAGE],
        count: 0,
    };
}

static IN_FLIGHT: TicketLock<[Bundle; BUNDLES]> = TicketLock::new([Bundle::EMPTY; BUNDLES]);

/// The `SCM_RIGHTS` descriptors in `sendmsg`'s control data, copied for the
/// peer of `process_fd` (an end of a connected AF_UNIX pair). Other control
/// messages (`SCM_CREDENTIALS`) are accepted and dropped.
pub(super) fn collect(process_fd: &ProcessFd, control: u64, length: u64) -> Result<Bundle, u64> {
    if !process_fd.unix_socket || unix_unbound(process_fd.handle) {
        return Err(95);
    }
    let inbox = UNIX_PAIRS
        .lock()
        .get(process_fd.handle as usize)
        .is_none_or(|pair| !pair.used || pair.inbox);
    if inbox {
        return Err(95);
    }
    let mut bundle = Bundle {
        used: true,
        pair: process_fd.handle,
        to_end_b: !process_fd.unix_end_b,
        ..Bundle::EMPTY
    };
    let length = length.min(4096);
    let mut offset = 0;
    while offset + 16 <= length {
        let mut header = [0u8; 16];
        if !user::copy_from_user(control.wrapping_add(offset), &mut header) {
            return Err(14);
        }
        let message_length = read_array_u64(&header, 0);
        let level = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        let kind = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
        if message_length < 16 || offset + message_length > length {
            return Err(22);
        }
        if level == SOL_SOCKET && kind == SCM_RIGHTS {
            let count = ((message_length - 16) / 4) as usize;
            if bundle.count + count > PER_MESSAGE {
                return Err(109);
            }
            for index in 0..count {
                let mut number = [0u8; 4];
                let at = control.wrapping_add(offset + 16 + index as u64 * 4);
                if !user::copy_from_user(at, &mut number) {
                    return Err(14);
                }
                let descriptor = u64::from(u32::from_le_bytes(number));
                let Some(passed) = lookup_process_fd(descriptor) else {
                    return Err(9);
                };
                bundle.fds[bundle.count] = ProcessFd {
                    close_on_exec: false,
                    ..passed
                };
                bundle.count += 1;
            }
        }
        offset += message_length.next_multiple_of(8);
    }
    if !IN_FLIGHT.lock().iter().any(|slot| !slot.used) {
        return Err(109);
    }
    Ok(bundle)
}

/// The data went out: the descriptors travel with it.
pub(super) fn queue(bundle: Bundle) {
    if bundle.count == 0 {
        return;
    }
    let mut slots = IN_FLIGHT.lock();
    if let Some(slot) = slots.iter_mut().find(|slot| !slot.used) {
        *slot = bundle;
    }
}

/// `recvmsg` on `process_fd`: installs the oldest bundle queued for it and
/// writes its `SCM_RIGHTS` message into the control buffer. Returns the
/// control length used and the message flags (`MSG_CTRUNC` when some did not
/// fit and were closed).
pub(super) fn deliver(
    process_fd: &ProcessFd,
    control: u64,
    room: u64,
    cloexec: bool,
) -> (u64, u32) {
    if !process_fd.unix_socket || unix_unbound(process_fd.handle) {
        return (0, 0);
    }
    let bundle = {
        let mut slots = IN_FLIGHT.lock();
        let Some(slot) = slots.iter_mut().find(|slot| {
            slot.used && slot.pair == process_fd.handle && slot.to_end_b == process_fd.unix_end_b
        }) else {
            return (0, 0);
        };
        core::mem::replace(slot, Bundle::EMPTY)
    };
    let fits = (room.saturating_sub(16) / 4) as usize;
    let mut installed = [0u32; PER_MESSAGE];
    let mut count = 0;
    let mut truncated = false;
    for passed in &bundle.fds[..bundle.count] {
        let slot = if count < fits {
            install_copy(*passed, cloexec)
        } else {
            None
        };
        match slot {
            Some(descriptor) => {
                installed[count] = descriptor as u32;
                count += 1;
            }
            None => {
                truncated = true;
                if !referenced(*passed) {
                    let _ = release_process_fd(*passed);
                }
            }
        }
    }
    if count == 0 {
        return (0, if truncated { MSG_CTRUNC } else { 0 });
    }
    let message_length = 16 + 4 * count as u64;
    let mut header = [0u8; 16];
    header[..8].copy_from_slice(&message_length.to_le_bytes());
    header[8..12].copy_from_slice(&SOL_SOCKET.to_le_bytes());
    header[12..16].copy_from_slice(&SCM_RIGHTS.to_le_bytes());
    let mut written = user::copy_to_user(control, &header);
    for (index, descriptor) in installed[..count].iter().enumerate() {
        written &= user::copy_to_user(
            control.wrapping_add(16 + index as u64 * 4),
            &descriptor.to_le_bytes(),
        );
    }
    if !written {
        return (0, MSG_CTRUNC);
    }
    (
        message_length.next_multiple_of(8).min(room),
        if truncated { MSG_CTRUNC } else { 0 },
    )
}

/// Whether a queued bundle holds a copy matching `matches`.
pub(super) fn holds(matches: impl Fn(&ProcessFd) -> bool) -> bool {
    IN_FLIGHT
        .lock()
        .iter()
        .filter(|slot| slot.used)
        .any(|slot| slot.fds[..slot.count].iter().any(&matches))
}

/// The connection `pair` is gone: what was queued on it is closed.
pub(super) fn discard(pair: u32) {
    loop {
        let bundle = {
            let mut slots = IN_FLIGHT.lock();
            let Some(slot) = slots.iter_mut().find(|slot| slot.used && slot.pair == pair) else {
                return;
            };
            core::mem::replace(slot, Bundle::EMPTY)
        };
        for passed in &bundle.fds[..bundle.count] {
            if !referenced(*passed) {
                let _ = release_process_fd(*passed);
            }
        }
    }
}
