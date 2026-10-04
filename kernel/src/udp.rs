//! UDP port table: every bound port owns a small queue of received datagrams.
//! `tcpnet::poll` parses frames from the card and `net::send_udp` delivers
//! loopback traffic straight in, so a datagram that arrives while its owner is
//! busy is kept instead of lost. Kernel clients (DNS, NTP) bind implicitly when
//! they send; sockets bind explicitly. Port 0 asks for an ephemeral port.

use crate::ip::Address;
use crate::sync::TicketLock;

pub const MAX_PAYLOAD: usize = 1472;
const BINDINGS: usize = 24;
const QUEUE: usize = 4;
const EPHEMERAL_FIRST: u16 = 49_200;

pub const EADDRINUSE: u64 = 98;
pub const EMFILE: u64 = 24;

#[derive(Clone, Copy)]
struct Datagram {
    source: Address,
    source_port: u16,
    length: usize,
    data: [u8; MAX_PAYLOAD],
}

impl Datagram {
    const EMPTY: Self = Self {
        source: [0; 16],
        source_port: 0,
        length: 0,
        data: [0; MAX_PAYLOAD],
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Free,
    /// Created by a kernel client's first send; recycled oldest first.
    Kernel,
    Socket,
}

#[derive(Clone, Copy)]
struct Binding {
    owner: Owner,
    port: u16,
    head: usize,
    count: usize,
    last_used: u64,
    queue: [Datagram; QUEUE],
}

impl Binding {
    const EMPTY: Self = Self {
        owner: Owner::Free,
        port: 0,
        head: 0,
        count: 0,
        last_used: 0,
        queue: [Datagram::EMPTY; QUEUE],
    };
}

struct Table {
    bindings: [Binding; BINDINGS],
    next_port: u16,
    dropped: u64,
}

static TABLE: TicketLock<Table> = TicketLock::new(Table {
    bindings: [Binding::EMPTY; BINDINGS],
    next_port: EPHEMERAL_FIRST,
    dropped: 0,
});

fn port_taken(table: &Table, port: u16) -> bool {
    table
        .bindings
        .iter()
        .any(|binding| binding.owner != Owner::Free && binding.port == port)
}

fn claim(table: &mut Table, port: u16, owner: Owner) -> Result<usize, u64> {
    let slot = table
        .bindings
        .iter()
        .position(|binding| binding.owner == Owner::Free)
        .or_else(|| {
            table
                .bindings
                .iter()
                .enumerate()
                .filter(|(_, binding)| binding.owner == Owner::Kernel)
                .min_by_key(|(_, binding)| binding.last_used)
                .map(|(index, _)| index)
        })
        .ok_or(EMFILE)?;
    let chosen = if port != 0 {
        port
    } else {
        let mut candidate = table.next_port;
        let mut tries = 0;
        while port_taken(table, candidate) {
            candidate = if candidate == 65_535 {
                EPHEMERAL_FIRST
            } else {
                candidate + 1
            };
            tries += 1;
            if tries > 20_000 {
                return Err(EADDRINUSE);
            }
        }
        table.next_port = if candidate == 65_535 {
            EPHEMERAL_FIRST
        } else {
            candidate + 1
        };
        candidate
    };
    table.bindings[slot] = Binding {
        owner,
        port: chosen,
        last_used: crate::time::monotonic_nanoseconds(),
        ..Binding::EMPTY
    };
    Ok(slot)
}

/// Binds `port` (or an ephemeral one for 0) for a socket and returns its slot.
pub fn bind_socket(port: u16) -> Result<usize, u64> {
    let mut table = TABLE.lock();
    if port != 0 && port_taken(&table, port) {
        return Err(EADDRINUSE);
    }
    claim(&mut table, port, Owner::Socket)
}

/// Moves a socket's binding to another port; the old one is kept if that fails.
pub fn rebind(slot: usize, port: u16) -> Result<(), u64> {
    let mut table = TABLE.lock();
    let binding = table
        .bindings
        .get(slot)
        .filter(|binding| binding.owner == Owner::Socket)
        .ok_or(EADDRINUSE)?;
    if binding.port == port {
        return Ok(());
    }
    if port == 0 || port_taken(&table, port) {
        return Err(EADDRINUSE);
    }
    table.bindings[slot].port = port;
    Ok(())
}

pub fn slot_of_port(port: u16) -> Option<usize> {
    TABLE
        .lock()
        .bindings
        .iter()
        .position(|binding| binding.owner != Owner::Free && binding.port == port)
}

pub fn port_of(slot: usize) -> Option<u16> {
    let table = TABLE.lock();
    table
        .bindings
        .get(slot)
        .filter(|binding| binding.owner == Owner::Socket)
        .map(|binding| binding.port)
}

pub fn release(slot: usize) {
    let mut table = TABLE.lock();
    if let Some(binding) = table.bindings.get_mut(slot) {
        *binding = Binding::EMPTY;
    }
}

#[cfg(feature = "boot-test")]
pub fn reset() {
    let mut table = TABLE.lock();
    for binding in table.bindings.iter_mut() {
        *binding = Binding::EMPTY;
    }
    table.next_port = EPHEMERAL_FIRST;
    table.dropped = 0;
}

/// A kernel client is about to send from `port`: make sure replies have
/// somewhere to land. A port a socket already owns is left alone.
pub fn ensure_kernel_binding(port: u16) {
    let mut table = TABLE.lock();
    let now = crate::time::monotonic_nanoseconds();
    if let Some(binding) = table
        .bindings
        .iter_mut()
        .find(|binding| binding.owner != Owner::Free && binding.port == port)
    {
        binding.last_used = now;
        return;
    }
    let _ = claim(&mut table, port, Owner::Kernel);
}

/// Queues a datagram for whoever owns `port`; returns whether anyone did.
pub fn deliver(port: u16, source: Address, source_port: u16, payload: &[u8]) -> bool {
    if payload.len() > MAX_PAYLOAD {
        return false;
    }
    let mut table = TABLE.lock();
    let Some(index) = table
        .bindings
        .iter()
        .position(|binding| binding.owner != Owner::Free && binding.port == port)
    else {
        return false;
    };
    let binding = &mut table.bindings[index];
    if binding.count == QUEUE {
        table.dropped += 1;
        return true;
    }
    let slot = (binding.head + binding.count) % QUEUE;
    let datagram = &mut binding.queue[slot];
    datagram.source = source;
    datagram.source_port = source_port;
    datagram.length = payload.len();
    datagram.data[..payload.len()].copy_from_slice(payload);
    binding.count += 1;
    true
}

pub struct Received {
    pub length: usize,
    /// The datagram's real size, before it was cut to fit the caller.
    pub full: usize,
    pub source: Address,
    pub source_port: u16,
}

/// Takes the oldest datagram in the queue of `slot` that comes from `filter`
/// (any peer when `None`). A datagram longer than `out` is cut to fit.
pub fn take(slot: usize, filter: Option<(Address, u16)>, out: &mut [u8]) -> Option<Received> {
    take_or_peek(slot, filter, out, true)
}

/// Like `take`, but the datagram stays queued.
pub fn peek(slot: usize, filter: Option<(Address, u16)>, out: &mut [u8]) -> Option<Received> {
    take_or_peek(slot, filter, out, false)
}

/// Size of the datagram `take` would return next, for `FIONREAD`.
pub fn next_length(slot: usize) -> usize {
    let table = TABLE.lock();
    table
        .bindings
        .get(slot)
        .filter(|binding| binding.owner != Owner::Free && binding.count > 0)
        .map_or(0, |binding| binding.queue[binding.head].length)
}

fn take_or_peek(
    slot: usize,
    filter: Option<(Address, u16)>,
    out: &mut [u8],
    consume: bool,
) -> Option<Received> {
    let mut table = TABLE.lock();
    let binding = table.bindings.get_mut(slot)?;
    if binding.owner == Owner::Free {
        return None;
    }
    binding.last_used = crate::time::monotonic_nanoseconds();
    for offset in 0..binding.count {
        let index = (binding.head + offset) % QUEUE;
        let datagram = binding.queue[index];
        if filter.is_some_and(|peer| peer != (datagram.source, datagram.source_port)) {
            continue;
        }
        let length = datagram.length.min(out.len());
        out[..length].copy_from_slice(&datagram.data[..length]);
        if consume {
            for later in offset..binding.count - 1 {
                let from = (binding.head + later + 1) % QUEUE;
                let to = (binding.head + later) % QUEUE;
                binding.queue[to] = binding.queue[from];
            }
            binding.count -= 1;
        }
        return Some(Received {
            length,
            full: datagram.length,
            source: datagram.source,
            source_port: datagram.source_port,
        });
    }
    None
}

/// Calls `visit(port, from_a_socket, queued)` for every bound port.
pub fn each_binding(mut visit: impl FnMut(u16, bool, usize)) {
    let table = TABLE.lock();
    for binding in table.bindings.iter().filter(|b| b.owner != Owner::Free) {
        visit(binding.port, binding.owner == Owner::Socket, binding.count);
    }
}

pub fn pending(slot: usize) -> bool {
    TABLE
        .lock()
        .bindings
        .get(slot)
        .is_some_and(|binding| binding.owner != Owner::Free && binding.count > 0)
}

#[cfg(feature = "boot-test")]
pub fn dropped() -> u64 {
    TABLE.lock().dropped
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> bool {
    reset();
    let loopback = crate::ip::v4([127, 0, 0, 1]);
    let (Ok(first), Ok(second)) = (bind_socket(6001), bind_socket(6002)) else {
        return false;
    };
    let busy = bind_socket(6001) == Err(EADDRINUSE);
    let ephemeral = bind_socket(0).ok().and_then(port_of);
    let ephemeral_ok = ephemeral.is_some_and(|port| port >= EPHEMERAL_FIRST);

    let sent = crate::net::send_udp([127, 0, 0, 1], 6001, 6002, b"hello udp");
    let mut buffer = [0u8; 64];
    let received = take(second, None, &mut buffer);
    let delivered =
        sent && received.as_ref().is_some_and(|datagram| {
            datagram.length == 9
                && &buffer[..9] == b"hello udp"
                && datagram.source == loopback
                && datagram.source_port == 6001
        }) && !pending(second)
            && take(second, None, &mut buffer).is_none();

    let filtered = deliver(6002, crate::ip::v4([10, 0, 0, 9]), 53, b"other")
        && deliver(6002, loopback, 6001, b"wanted")
        && take(second, Some((loopback, 6001)), &mut buffer)
            .is_some_and(|datagram| datagram.length == 6 && &buffer[..6] == b"wanted")
        && pending(second)
        && take(second, None, &mut buffer).is_some_and(|datagram| {
            datagram.source == crate::ip::v4([10, 0, 0, 9]) && &buffer[..5] == b"other"
        });

    let before = dropped();
    let mut accepted = 0;
    for index in 0..QUEUE + 2 {
        if deliver(6001, loopback, 7000 + index as u16, b"x") {
            accepted += 1;
        }
    }
    let overflow_ok = accepted == QUEUE + 2 && dropped() == before + 2;
    let ordered = take(first, None, &mut buffer)
        .is_some_and(|datagram| datagram.source_port == 7000)
        && take(first, None, &mut buffer).is_some_and(|datagram| datagram.source_port == 7001);

    let truncated = deliver(6002, loopback, 1, &[7u8; 100])
        && take(second, None, &mut buffer[..10]).is_some_and(|datagram| datagram.length == 10);

    let unbound = !deliver(6999, loopback, 1, b"nobody")
        && !deliver(6002, loopback, 1, &[0u8; MAX_PAYLOAD + 1]);
    let moved = rebind(second, 6003).is_ok()
        && !deliver(6002, loopback, 1, b"old port")
        && deliver(6003, loopback, 1, b"new port")
        && rebind(second, 6001) == Err(EADDRINUSE)
        && port_of(second) == Some(6003);
    release(second);
    let released = !deliver(6003, loopback, 1, b"gone");

    for port in 20_000..20_000 + BINDINGS as u16 {
        ensure_kernel_binding(port);
    }
    let recycled =
        deliver(20_000 + BINDINGS as u16 - 1, loopback, 1, b"newest") && bind_socket(0).is_ok();
    reset();
    delivered
        && busy
        && ephemeral_ok
        && filtered
        && overflow_ok
        && ordered
        && truncated
        && unbound
        && moved
        && released
        && recycled
}
