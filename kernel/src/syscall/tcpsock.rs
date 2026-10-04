//! `AF_INET`/`SOCK_STREAM` sockets on top of the shared TCP table. A
//! descriptor of this kind has `tcp` set and its `handle` is the table index.
//! Blocking calls poll the network and yield until the engine can answer, and
//! return EINTR when a signal arrives; non-blocking descriptors get EAGAIN
//! (EINPROGRESS for `connect`) instead.

use super::*;
use crate::tcp::{self, EAGAIN, State};
use crate::tcpnet;

const EINPROGRESS: u64 = 115;
const EALREADY: u64 = 114;
const EISCONN: u64 = 106;
const CONNECT_TIMEOUT_NS: u64 = 30_000_000_000;
const EVENT_ERROR: u32 = 0x8;
const EVENT_HANGUP: u32 = 0x10;
const TCP_MAXSEG: u64 = 2;
use crate::sockopt::{IPPROTO_IPV6, IPPROTO_TCP};
const IPV6_V6ONLY: u64 = 26;
const SO_DOMAIN: u64 = 39;
const SO_PROTOCOL: u64 = 38;

pub(super) fn lookup(descriptor: u64) -> Option<ProcessFd> {
    lookup_process_fd(descriptor).filter(|process_fd| process_fd.tcp)
}

fn adopt(index: usize, close_on_exec: bool, nonblocking: bool, inet6: bool) -> Result<u64, u64> {
    let installed = install_process_fd_kind(
        index as u32,
        close_on_exec,
        false,
        false,
        false,
        false,
        false,
        false,
        true,
        true,
        false,
    );
    let Some(descriptor) = installed else {
        tcpnet::with_tcp(|tcp, sink, now| tcp.close(index, now, sink));
        return Err(24);
    };
    let mut descriptors = PROCESS_FDS.lock();
    descriptors[descriptor as usize].tcp = true;
    descriptors[descriptor as usize].nonblocking = nonblocking;
    descriptors[descriptor as usize].inet6 = inet6;
    Ok(descriptor)
}

pub(super) fn create(close_on_exec: bool, nonblocking: bool, inet6: bool) -> Result<u64, u64> {
    let index = tcpnet::with_tcp(|tcp, _, _| {
        let index = tcp.socket()?;
        tcp.set_dual(index, inet6);
        Some(index)
    })
    .ok_or(24u64)?;
    tcpnet::activate();
    adopt(index, close_on_exec, nonblocking, inet6)
}

/// Retries `attempt` until the engine can answer. A blocking call gives up
/// with EAGAIN once the socket's receive (`receive`) or send time-out has
/// passed, and with EINTR when a signal arrives.
fn blocking<T>(
    process_fd: ProcessFd,
    receive: bool,
    mut attempt: impl FnMut() -> Result<T, u64>,
) -> Result<T, u64> {
    let index = process_fd.handle as usize;
    let options = tcpnet::with_tcp(|tcp, _, _| tcp.options(index));
    let timeout = if receive {
        options.receive_timeout_ns
    } else {
        options.send_timeout_ns
    };
    let started = tcpnet::now();
    loop {
        tcpnet::poll();
        match attempt() {
            Err(EAGAIN) if !process_fd.nonblocking => {
                if interrupted() {
                    return Err(4);
                }
                if timeout != 0 && tcpnet::now().saturating_sub(started) >= timeout {
                    return Err(EAGAIN);
                }
                yield_in_syscall();
            }
            other => return other,
        }
    }
}

pub(super) fn connect_to(process_fd: ProcessFd, remote: crate::ip::Address, port: u16) -> u64 {
    let index = process_fd.handle as usize;
    let started = tcpnet::with_tcp(|tcp, sink, now| tcp.connect(index, remote, port, now, sink));
    match started {
        Ok(()) => {}
        Err(EAGAIN) => return error(EALREADY),
        Err(tcp::EISCONN) => return error(EISCONN),
        Err(failure) => return error(failure),
    }
    if process_fd.nonblocking {
        return error(EINPROGRESS);
    }
    let deadline = tcpnet::now().saturating_add(CONNECT_TIMEOUT_NS);
    loop {
        tcpnet::poll();
        let (state, failure) = tcpnet::with_tcp(|tcp, _, _| (tcp.state(index), tcp.error(index)));
        match state {
            Some(State::SynSent | State::SynReceived) => {}
            Some(State::Closed) | None => {
                let reason = tcpnet::with_tcp(|tcp, _, _| tcp.take_error(index));
                return error(if reason != 0 {
                    reason
                } else {
                    tcp::ECONNREFUSED
                });
            }
            Some(_) if failure != 0 => {
                return error(tcpnet::with_tcp(|tcp, _, _| tcp.take_error(index)));
            }
            Some(_) => return 0,
        }
        if interrupted() {
            return error(4);
        }
        if tcpnet::now() >= deadline {
            return error(tcp::ETIMEDOUT);
        }
        yield_in_syscall();
    }
}

pub(super) fn connect(process_fd: ProcessFd, address: u64, length: u64) -> u64 {
    let (remote, port) = match read_sockaddr(address, length, process_fd.inet6) {
        Ok(target) => target,
        Err(failure) => return failure,
    };
    connect_to(process_fd, remote, port)
}

pub(super) fn bind_port(address: u64, length: u64, inet6: bool) -> Option<u16> {
    let (family, minimum) = if inet6 { (10, 28) } else { (2, 16) };
    if length < minimum {
        return None;
    }
    let mut encoded = [0u8; 4];
    if !user::copy_from_user(address, &mut encoded)
        || u16::from_le_bytes([encoded[0], encoded[1]]) != family
    {
        return None;
    }
    Some(u16::from_be_bytes([encoded[2], encoded[3]]))
}

pub(super) fn bind(process_fd: ProcessFd, address: u64, length: u64) -> u64 {
    let Some(port) = bind_port(address, length, process_fd.inet6) else {
        return error(22);
    };
    bind_to(process_fd, port)
}

pub(super) fn bind_to(process_fd: ProcessFd, port: u16) -> u64 {
    if !super::bind_port_allowed(port) {
        return error(13);
    }
    let index = process_fd.handle as usize;
    match tcpnet::with_tcp(|tcp, _, _| tcp.bind(index, port)) {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

pub(super) fn listen(process_fd: ProcessFd) -> u64 {
    let index = process_fd.handle as usize;
    match tcpnet::with_tcp(|tcp, _, _| tcp.listen(index)) {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

pub(super) fn accept(process_fd: ProcessFd, address: u64, length_address: u64, flags: u64) -> u64 {
    let index = process_fd.handle as usize;
    if length_address != 0
        && (!user::range_accessible(length_address, 4, true)
            || !user::range_accessible(address, if process_fd.inet6 { 28 } else { 16 }, true))
    {
        return error(14);
    }
    let listening = tcpnet::with_tcp(|tcp, _, _| tcp.state(index)) == Some(State::Listen);
    if !listening {
        return error(22);
    }
    let child = blocking(process_fd, true, || {
        tcpnet::with_tcp(|tcp, _, _| tcp.accept(index)).ok_or(EAGAIN)
    });
    let child = match child {
        Ok(child) => child,
        Err(failure) => return error(failure),
    };
    let descriptor = match adopt(
        child,
        flags & O_CLOEXEC != 0,
        flags & 0x800 != 0,
        process_fd.inet6,
    ) {
        Ok(descriptor) => descriptor,
        Err(failure) => return error(failure),
    };
    if length_address != 0 {
        let peer = tcpnet::with_tcp(|tcp, _, _| tcp.peer(child));
        if let Some((remote, port)) = peer {
            let _ = write_sockaddr(address, length_address, &remote, port, process_fd.inet6);
        }
    }
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    descriptor
}

pub(super) fn read_bytes(
    process_fd: ProcessFd,
    buffer: &mut [u8],
    wait: bool,
) -> Result<usize, u64> {
    let index = process_fd.handle as usize;
    let attempt =
        |buffer: &mut [u8]| tcpnet::with_tcp(|tcp, sink, _| tcp.read(index, buffer, sink));
    if !wait {
        tcpnet::poll();
        return attempt(buffer);
    }
    blocking(process_fd, true, || attempt(&mut *buffer))
}

fn peek_bytes(process_fd: ProcessFd, buffer: &mut [u8], wait: bool) -> Result<usize, u64> {
    let index = process_fd.handle as usize;
    let attempt = |buffer: &mut [u8]| tcpnet::with_tcp(|tcp, _, _| tcp.peek(index, buffer));
    if !wait {
        tcpnet::poll();
        return attempt(buffer);
    }
    blocking(process_fd, true, || attempt(&mut *buffer))
}

/// `recv`/`recvfrom` on a stream socket with its flags: `MSG_PEEK` leaves the
/// data queued, `MSG_DONTWAIT` never blocks and `MSG_WAITALL` keeps reading
/// until the buffer is full, the stream ends or an error occurs.
pub(super) fn receive(process_fd: ProcessFd, address: u64, requested: u64, flags: u64) -> u64 {
    let Ok(requested) = usize::try_from(requested) else {
        return error(22);
    };
    if requested > MAX_IO || !user::range_accessible(address, requested, true) {
        return error(14);
    }
    let (peek, waitall, dontwait) = (flags & 2 != 0, flags & 0x100 != 0, flags & 0x40 != 0);
    let mut chunk = [0u8; IO_CHUNK];
    let mut total = 0usize;
    while total < requested {
        let amount = (requested - total).min(chunk.len());
        let wait = !dontwait && (total == 0 || waitall);
        let result = if peek {
            peek_bytes(process_fd, &mut chunk[..amount], wait)
        } else {
            read_bytes(process_fd, &mut chunk[..amount], wait)
        };
        let read = match result {
            Ok(read) => read,
            Err(failure) => {
                return if total == 0 {
                    error(failure)
                } else {
                    total as u64
                };
            }
        };
        if read == 0 {
            break;
        }
        if !user::copy_to_user(address + total as u64, &chunk[..read]) {
            return error(14);
        }
        total += read;
        if peek || (!waitall && read < amount) {
            break;
        }
    }
    total as u64
}

pub(super) fn write_bytes(process_fd: ProcessFd, data: &[u8]) -> Result<usize, u64> {
    let index = process_fd.handle as usize;
    let mut sent = 0usize;
    while sent < data.len() {
        let result = blocking(process_fd, false, || {
            tcpnet::with_tcp(|tcp, sink, now| tcp.write(index, &data[sent..], now, sink))
        });
        match result {
            Ok(count) => sent += count,
            Err(failure) if sent > 0 => {
                let _ = failure;
                return Ok(sent);
            }
            Err(failure) => return Err(failure),
        }
        if process_fd.nonblocking {
            break;
        }
    }
    Ok(sent)
}

pub(super) fn shutdown(process_fd: ProcessFd, how: u64) -> u64 {
    let index = process_fd.handle as usize;
    if tcpnet::with_tcp(|tcp, _, _| tcp.state(index))
        .is_none_or(|state| matches!(state, State::Closed | State::Listen | State::SynSent))
    {
        return error(tcp::ENOTCONN);
    }
    if how != 0 {
        tcpnet::with_tcp(|tcp, sink, now| tcp.shutdown_write(index, now, sink));
    }
    0
}

pub(super) fn has_data(process_fd: &ProcessFd) -> bool {
    tcpnet::poll();
    let index = process_fd.handle as usize;
    tcpnet::with_tcp(|tcp, _, _| tcp.readable(index))
}

pub(super) fn ready_events(process_fd: &ProcessFd) -> u32 {
    tcpnet::poll();
    let index = process_fd.handle as usize;
    tcpnet::with_tcp(|tcp, _, _| {
        let mut events = 0;
        if tcp.readable(index) {
            events |= EPOLLIN;
        }
        if tcp.writable(index) {
            events |= EPOLLOUT;
        }
        if tcp.error(index) != 0 {
            events |= EVENT_ERROR;
        }
        if tcp.hung_up(index) {
            events |= EVENT_HANGUP;
        }
        events
    })
}

pub(super) fn release(process_fd: &ProcessFd) {
    let index = process_fd.handle as usize;
    tcpnet::with_tcp(|tcp, sink, now| {
        let options = tcp.options(index);
        if options.linger_on && options.linger_seconds == 0 {
            tcp.abort(index, now, sink);
        } else {
            tcp.close(index, now, sink);
        }
    });
}

pub(super) fn set_option(process_fd: &ProcessFd, level: u64, option: u64, data: &[u8]) -> u64 {
    let index = process_fd.handle as usize;
    let mut options = tcpnet::with_tcp(|tcp, _, _| tcp.options(index));
    if let Err(failure) = options.set(level, option, data) {
        return error(failure);
    }
    if level == IPPROTO_IPV6 && !process_fd.inet6 {
        return error(92);
    }
    tcpnet::with_tcp(|tcp, _, _| {
        tcp.set_options(index, options);
        if level == IPPROTO_IPV6 && option == IPV6_V6ONLY {
            tcp.set_dual(index, !options.v6_only);
        }
    });
    0
}

/// The bytes `getsockopt` returns for a stream socket and how many there are.
pub(super) fn option_bytes(
    process_fd: &ProcessFd,
    level: u64,
    option: u64,
) -> Option<([u8; 16], usize)> {
    let index = process_fd.handle as usize;
    let integer = |value: u32| {
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&value.to_le_bytes());
        Some((bytes, 4))
    };
    match (level, option) {
        (SOL_SOCKET, SO_TYPE) => integer(1),
        (SOL_SOCKET, SO_ERROR) => {
            integer(tcpnet::with_tcp(|tcp, _, _| tcp.take_error(index)) as u32)
        }
        (SOL_SOCKET, SO_ACCEPTCONN) => integer(u32::from(
            tcpnet::with_tcp(|tcp, _, _| tcp.state(index)) == Some(State::Listen),
        )),
        (SOL_SOCKET, SO_DOMAIN) => integer(if process_fd.inet6 { 10 } else { 2 }),
        (SOL_SOCKET, SO_PROTOCOL) => integer(6),
        (IPPROTO_TCP, TCP_MAXSEG) => integer(tcp::MSS as u32),
        (IPPROTO_IPV6, _) if !process_fd.inet6 => None,
        _ => tcpnet::with_tcp(|tcp, _, _| tcp.options(index)).get(level, option),
    }
}

pub(super) fn option_value(process_fd: &ProcessFd, level: u64, option: u64) -> Option<u32> {
    option_bytes(process_fd, level, option)
        .filter(|(_, size)| *size == 4)
        .map(|(bytes, _)| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub(super) fn address(process_fd: &ProcessFd, peer: bool) -> Result<([u8; 28], usize), u64> {
    let index = process_fd.handle as usize;
    let (port, remote, state) = tcpnet::with_tcp(|tcp, _, _| {
        (
            tcp.local_port(index).unwrap_or(0),
            tcp.peer(index),
            tcp.state(index),
        )
    });
    let connected = !matches!(
        state,
        None | Some(State::Closed | State::Listen | State::SynSent)
    );
    let v6 = process_fd.inet6;
    if peer {
        let Some((remote, remote_port)) = remote.filter(|_| connected) else {
            return Err(tcp::ENOTCONN);
        };
        return Ok(sockaddr_bytes(&remote, remote_port, v6));
    }
    let local = match remote {
        Some((remote, _)) if tcpnet::is_local(remote) => remote,
        Some((remote, _)) => match crate::ip::as_v4(&remote) {
            Some(_) => crate::ip::v4(crate::net::local_address()),
            None => crate::ipv6::source_address(&remote).unwrap_or(crate::ip::UNSPECIFIED),
        },
        None if v6 => crate::ip::UNSPECIFIED,
        None => crate::ip::v4([0; 4]),
    };
    Ok(sockaddr_bytes(&local, port, v6))
}

pub(crate) fn self_test() -> bool {
    tcpnet::reset();
    let ok = scenario();
    tcpnet::reset();
    ok
}

fn pump(rounds: usize) {
    for _ in 0..rounds {
        tcpnet::poll();
    }
}

fn make(nonblocking: bool) -> Option<(u64, ProcessFd)> {
    let descriptor = create(false, nonblocking, false).ok()?;
    Some((descriptor, lookup(descriptor)?))
}

fn scenario() -> bool {
    const LOOPBACK: crate::ip::Address = crate::ip::v4([127, 0, 0, 1]);
    const PORT: u16 = 5001;
    let pattern = |offset: usize| (offset.wrapping_mul(13).wrapping_add(5)) as u8;

    let (Some((listener_fd, listener)), Some((client_fd, client))) = (make(true), make(true))
    else {
        return false;
    };
    let nothing_to_accept = accept(listener, 0, 0, 0) == error(22);
    let bound = bind_to(listener, PORT) == 0 && bind_to(client, PORT) == error(tcp::EADDRINUSE);
    let listening = listen(listener) == 0;
    let empty_accept = accept(listener, 0, 0, 0) == error(EAGAIN);
    let connecting = connect_to(client, LOOPBACK, PORT) == error(EINPROGRESS);
    pump(8);
    let server_fd = {
        let descriptor = accept(listener, 0, 0, 0);
        (descriptor as i64 >= 0).then_some(descriptor)
    };
    let Some(server_descriptor) = server_fd else {
        return false;
    };
    let Some(server) = lookup(server_descriptor) else {
        return false;
    };
    let established =
        ready_events(&client) & EPOLLOUT != 0 && ready_events(&server) & EPOLLOUT != 0;
    let quiet = ready_events(&server) & EPOLLIN == 0;
    let names = address(&client, true)
        .is_ok_and(|(bytes, _)| bytes[2..8] == [0x13, 0x89, 127, 0, 0, 1])
        && address(&server, false).is_ok_and(|(bytes, _)| bytes[2..4] == [0x13, 0x89])
        && address(&listener, true) == Err(tcp::ENOTCONN);
    let options = option_value(&client, SOL_SOCKET, SO_ERROR) == Some(0)
        && option_value(&listener, SOL_SOCKET, SO_ACCEPTCONN) == Some(1)
        && option_value(&client, SOL_SOCKET, SO_TYPE) == Some(1)
        && option_value(&client, 99, 1).is_none();

    let upload = 24_000usize;
    let download = 6_000usize;
    let mut buffer = [0u8; 700];
    let (mut sent, mut received, mut echoed, mut echo_received) = (0usize, 0usize, 0usize, 0usize);
    let mut intact = true;
    for _ in 0..4_000 {
        if sent < upload {
            let end = (sent + buffer.len()).min(upload);
            for (index, byte) in buffer[..end - sent].iter_mut().enumerate() {
                *byte = pattern(sent + index);
            }
            if let Ok(count) = write_bytes(client, &buffer[..end - sent]) {
                sent += count;
            }
        }
        if let Ok(count) = read_bytes(server, &mut buffer, false) {
            for (index, byte) in buffer[..count].iter().enumerate() {
                intact &= *byte == pattern(received + index);
            }
            received += count;
        }
        if echoed < download && received > echoed {
            let end = (echoed + 500).min(download);
            for (index, byte) in buffer[..end - echoed].iter_mut().enumerate() {
                *byte = pattern(100_000 + echoed + index);
            }
            if let Ok(count) = write_bytes(server, &buffer[..end - echoed]) {
                echoed += count;
            }
        }
        if let Ok(count) = read_bytes(client, &mut buffer, false) {
            for (index, byte) in buffer[..count].iter().enumerate() {
                intact &= *byte == pattern(100_000 + echo_received + index);
            }
            echo_received += count;
        }
        if received == upload && echo_received == download {
            break;
        }
    }
    let transfer = intact && sent == upload && received == upload && echo_received == download;

    let readable_after = {
        let _ = write_bytes(client, b"tail");
        pump(8);
        ready_events(&server) & EPOLLIN != 0
    };
    let mut tail = [0u8; 8];
    let tail_ok = read_bytes(server, &mut tail, false) == Ok(4) && &tail[..4] == b"tail";
    let would_block = read_bytes(server, &mut tail, false) == Err(EAGAIN);

    let shut = shutdown(client, 1) == 0;
    pump(8);
    let eof = read_bytes(server, &mut tail, false) == Ok(0)
        && ready_events(&server) & EPOLLIN != 0
        && ready_events(&server) & EVENT_HANGUP == 0;
    let half_close_write = write_bytes(server, b"late") == Ok(4);
    pump(8);
    let half_close_read = read_bytes(client, &mut tail, false) == Ok(4) && &tail[..4] == b"late";
    let closed_server = linux_close(server_descriptor) == 0;
    pump(8);
    let closed_client = linux_close(client_fd) == 0;
    pump(8);

    let refused = make(true).is_some_and(|(probe_fd, probe)| {
        let started = connect_to(probe, LOOPBACK, 5999) == error(EINPROGRESS);
        pump(8);
        let events = ready_events(&probe);
        let failed = events & EVENT_ERROR != 0;
        let reported = option_value(&probe, SOL_SOCKET, SO_ERROR) == Some(tcp::ECONNREFUSED as u32);
        let cleared = option_value(&probe, SOL_SOCKET, SO_ERROR) == Some(0);
        let closed = linux_close(probe_fd) == 0;
        started && failed && reported && cleared && closed
    });
    let closed_listener = linux_close(listener_fd) == 0;

    nothing_to_accept
        && bound
        && listening
        && empty_accept
        && connecting
        && established
        && quiet
        && names
        && options
        && transfer
        && readable_after
        && tail_ok
        && would_block
        && shut
        && eof
        && half_close_write
        && half_close_read
        && closed_server
        && closed_client
        && refused
        && closed_listener
}

#[cfg(feature = "boot-test")]
pub(crate) fn options_self_test() -> bool {
    tcpnet::reset();
    let ok = options_scenario();
    tcpnet::reset();
    ok
}

/// Socket options through the stream-socket layer: stored values read back,
/// `SO_REUSEADDR` lets a restarted server bind while its old connections
/// linger, a receive time-out ends a blocking read, a zero linger resets the
/// peer, `MSG_PEEK` leaves data in place, and an accepted connection survives
/// the listener being closed.
#[cfg(feature = "boot-test")]
fn options_scenario() -> bool {
    use crate::ip;
    const PORT: u16 = 5701;
    let int = |value: u32| value.to_le_bytes();
    let Some((probe_fd, probe)) = make(true) else {
        return false;
    };
    let stored = set_option(&probe, SOL_SOCKET, 2, &int(1)) == 0
        && set_option(&probe, IPPROTO_TCP, 1, &int(1)) == 0
        && set_option(&probe, SOL_SOCKET, 9, &int(1)) == 0
        && option_value(&probe, SOL_SOCKET, 2) == Some(1)
        && option_value(&probe, IPPROTO_TCP, 1) == Some(1)
        && option_value(&probe, SOL_SOCKET, 9) == Some(1)
        && option_value(&probe, SOL_SOCKET, SO_DOMAIN) == Some(2)
        && option_value(&probe, SOL_SOCKET, SO_PROTOCOL) == Some(6)
        && set_option(&probe, SOL_SOCKET, 999, &int(1)) == error(92)
        && set_option(&probe, IPPROTO_IPV6, 26, &int(1)) == error(92)
        && option_value(&probe, IPPROTO_IPV6, 26).is_none();
    let probe_closed = linux_close(probe_fd) == 0;

    let (Some((listener_fd, listener)), Some((client_fd, client))) = (make(true), make(false))
    else {
        return false;
    };
    let ready = bind_to(listener, PORT) == 0 && listen(listener) == 0;
    let connecting = connect_to(client, ip::v4([127, 0, 0, 1]), PORT);
    let _ = connecting;
    pump(8);
    let server_fd = accept(listener, 0, 0, 0);
    let Some(server) = lookup(server_fd) else {
        return false;
    };
    let _ = client;

    let mut linger = [0u8; 8];
    linger[..4].copy_from_slice(&1u32.to_le_bytes());
    let mut timeout = [0u8; 16];
    timeout[8..].copy_from_slice(&60_000i64.to_le_bytes());
    let timeouts = set_option(&client, SOL_SOCKET, 20, &timeout) == 0 && {
        let started = tcpnet::now();
        let mut buffer = [0u8; 8];
        let result = read_bytes(client, &mut buffer, true);
        let elapsed = tcpnet::now().saturating_sub(started);
        result == Err(EAGAIN) && (50_000_000..1_000_000_000).contains(&elapsed)
    };

    let _ = write_bytes(client, b"hello");
    pump(8);
    let mut buffer = [0u8; 16];
    let waiting = tcpnet::with_tcp(|tcp, _, _| tcp.available(server.handle as usize)) == 5;
    let peeked = peek_bytes(server, &mut buffer, false) == Ok(5) && &buffer[..5] == b"hello";
    let read_back = read_bytes(server, &mut buffer, false) == Ok(5) && &buffer[..5] == b"hello";
    let empty = tcpnet::with_tcp(|tcp, _, _| tcp.available(server.handle as usize)) == 0;

    let listener_closed = linux_close(listener_fd) == 0;
    pump(4);
    let survives = write_bytes(client, b"after") == Ok(5) && {
        pump(8);
        read_bytes(server, &mut buffer, false) == Ok(5) && &buffer[..5] == b"after"
    };

    let closed_first = linux_close(server_fd) == 0;
    pump(16);
    let closed_second = linux_close(client_fd) == 0;
    pump(16);
    let strict = make(true).is_some_and(|(fd, socket)| {
        let refused = bind_to(socket, PORT) == error(tcp::EADDRINUSE);
        let _ = linux_close(fd);
        refused
    });
    let reuse = make(true).is_some_and(|(fd, socket)| {
        let allowed = set_option(&socket, SOL_SOCKET, 2, &int(1)) == 0
            && bind_to(socket, PORT) == 0
            && listen(socket) == 0;
        let _ = linux_close(fd);
        allowed
    });

    let reset = match (make(true), make(true)) {
        (Some((listen_fd, listening)), Some((connect_fd, connecting))) => {
            let ok = bind_to(listening, 5702) == 0
                && listen(listening) == 0
                && connect_to(connecting, ip::v4([127, 0, 0, 1]), 5702) == error(EINPROGRESS);
            pump(8);
            let accepted = accept(listening, 0, 0, 0);
            let outcome = lookup(accepted).is_some_and(|peer| {
                let armed = set_option(&connecting, SOL_SOCKET, 13, &linger) == 0;
                let aborted = {
                    linger[..4].copy_from_slice(&1u32.to_le_bytes());
                    linger[4..].copy_from_slice(&0i32.to_le_bytes());
                    set_option(&connecting, SOL_SOCKET, 13, &linger) == 0
                };
                release(&connecting);
                pump(8);
                let mut sink = [0u8; 4];
                let seen = read_bytes(peer, &mut sink, false) == Err(tcp::ECONNRESET);
                let _ = linux_close(accepted);
                armed && aborted && seen
            });
            let _ = linux_close(listen_fd);
            let _ = linux_close(connect_fd);
            ok && outcome
        }
        _ => false,
    };
    pump(8);
    stored
        && probe_closed
        && ready
        && timeouts
        && waiting
        && peeked
        && read_back
        && empty
        && listener_closed
        && survives
        && closed_first
        && closed_second
        && strict
        && reuse
        && reset
}

#[cfg(feature = "boot-test")]
pub(crate) fn inet6_self_test() -> bool {
    tcpnet::reset();
    let ok = inet6_scenario();
    tcpnet::reset();
    ok
}

#[cfg(feature = "boot-test")]
fn make6(nonblocking: bool, inet6: bool) -> Option<(u64, ProcessFd)> {
    let descriptor = create(false, nonblocking, inet6).ok()?;
    Some((descriptor, lookup(descriptor)?))
}

/// `AF_INET6` stream sockets over the loopback address: a transfer in both
/// directions, an IPv4 client reaching a dual-stack listener (seen as an
/// IPv4-mapped peer), an `AF_INET` listener refusing an IPv6 client, and the
/// address layouts the socket calls report.
#[cfg(feature = "boot-test")]
fn inet6_scenario() -> bool {
    use crate::ip;
    const PORT: u16 = 5601;
    let pattern = |offset: usize| (offset.wrapping_mul(7).wrapping_add(11)) as u8;
    let (Some((listener_fd, listener)), Some((client_fd, client))) =
        (make6(true, true), make6(true, true))
    else {
        return false;
    };
    let ready = bind_to(listener, PORT) == 0 && listen(listener) == 0;
    let connecting = connect_to(client, ip::LOOPBACK6, PORT) == error(EINPROGRESS);
    pump(8);
    let server_descriptor = accept(listener, 0, 0, 0);
    if server_descriptor as i64 <= 0 {
        return false;
    }
    let Some(server) = lookup(server_descriptor) else {
        return false;
    };
    let names = address(&client, true).is_ok_and(|(bytes, size)| {
        size == 28
            && bytes[..2] == [10, 0]
            && bytes[2..4] == PORT.to_be_bytes()
            && bytes[8..24] == ip::LOOPBACK6
    }) && address(&server, true).is_ok_and(|(bytes, size)| {
        size == 28 && bytes[8..24] == ip::LOOPBACK6 && bytes[2..4] != PORT.to_be_bytes()
    }) && address(&server, false).is_ok_and(|(bytes, _)| bytes[8..24] == ip::LOOPBACK6);
    let options = option_value(&client, IPPROTO_IPV6, IPV6_V6ONLY) == Some(0)
        && option_value(&listener, SOL_SOCKET, SO_ACCEPTCONN) == Some(1);

    let total = 12_000usize;
    let mut buffer = [0u8; 600];
    let (mut sent, mut received, mut echoed, mut echo_received) = (0usize, 0usize, 0usize, 0usize);
    let mut intact = true;
    for _ in 0..4_000 {
        if sent < total {
            let end = (sent + buffer.len()).min(total);
            for (index, byte) in buffer[..end - sent].iter_mut().enumerate() {
                *byte = pattern(sent + index);
            }
            if let Ok(count) = write_bytes(client, &buffer[..end - sent]) {
                sent += count;
            }
        }
        if let Ok(count) = read_bytes(server, &mut buffer, false) {
            for (index, byte) in buffer[..count].iter().enumerate() {
                intact &= *byte == pattern(received + index);
            }
            received += count;
        }
        if echoed < received {
            let end = (echoed + 400).min(received);
            for (index, byte) in buffer[..end - echoed].iter_mut().enumerate() {
                *byte = pattern(500_000 + echoed + index);
            }
            if let Ok(count) = write_bytes(server, &buffer[..end - echoed]) {
                echoed += count;
            }
        }
        if let Ok(count) = read_bytes(client, &mut buffer, false) {
            for (index, byte) in buffer[..count].iter().enumerate() {
                intact &= *byte == pattern(500_000 + echo_received + index);
            }
            echo_received += count;
        }
        if received == total && echo_received == total {
            break;
        }
    }
    let transfer = intact && received == total && echo_received == total;

    let dual = make6(true, false).is_some_and(|(v4_fd, v4_client)| {
        let started = connect_to(v4_client, ip::v4([127, 0, 0, 1]), PORT) == error(EINPROGRESS);
        pump(8);
        let accepted = accept(listener, 0, 0, 0);
        let mapped = accepted as i64 > 0
            && lookup(accepted).is_some_and(|peer| {
                let seen = address(&peer, true).is_ok_and(|(bytes, size)| {
                    size == 28 && bytes[8..24] == ip::v4([127, 0, 0, 1])
                });
                let _ = linux_close(accepted);
                seen
            });
        let v4_view = address(&v4_client, true).is_ok_and(|(bytes, size)| {
            size == 16 && bytes[..2] == [2, 0] && bytes[4..8] == [127, 0, 0, 1]
        });
        let closed = linux_close(v4_fd) == 0;
        started && mapped && v4_view && closed
    });
    let v4_only = make6(true, false).is_some_and(|(only_fd, only)| {
        let ready = bind_to(only, 5602) == 0 && listen(only) == 0;
        let refused = make6(true, true).is_some_and(|(probe_fd, probe)| {
            let started = connect_to(probe, ip::LOOPBACK6, 5602) == error(EINPROGRESS);
            pump(8);
            let failed =
                option_value(&probe, SOL_SOCKET, SO_ERROR) == Some(tcp::ECONNREFUSED as u32);
            let closed = linux_close(probe_fd) == 0;
            started && failed && closed
        });
        let closed = linux_close(only_fd) == 0;
        ready && refused && closed
    });
    let none_pending = accept(listener, 0, 0, 0) == error(EAGAIN);
    let closed = linux_close(server_descriptor) == 0
        && linux_close(client_fd) == 0
        && linux_close(listener_fd) == 0;
    pump(8);
    ready && connecting && names && options && transfer && dual && v4_only && none_pending && closed
}

#[cfg(feature = "boot-test")]
pub(crate) struct NetEchoReport {
    pub accepted: bool,
    pub echoed: usize,
    pub closed: bool,
}

/// Listens on `port` through the socket layer and echoes everything a real
/// peer sends until it closes its end or the deadline passes.
#[cfg(feature = "boot-test")]
pub(crate) fn net_echo_test(port: u16, timeout_ns: u64) -> NetEchoReport {
    let mut report = NetEchoReport {
        accepted: false,
        echoed: 0,
        closed: false,
    };
    let Some((listener_fd, listener)) = make(true) else {
        return report;
    };
    if bind_to(listener, port) != 0 || listen(listener) != 0 {
        let _ = linux_close(listener_fd);
        return report;
    }
    let deadline = tcpnet::now().saturating_add(timeout_ns);
    let mut connection = None;
    while tcpnet::now() < deadline && connection.is_none() {
        tcpnet::poll();
        let descriptor = accept(listener, 0, 0, 0);
        if descriptor as i64 >= 0 {
            connection = lookup(descriptor).map(|process_fd| (descriptor, process_fd));
        }
    }
    let Some((connection_fd, peer)) = connection else {
        let _ = linux_close(listener_fd);
        return report;
    };
    report.accepted = true;
    let mut pending = [0u8; 2048];
    let (mut length, mut offset) = (0usize, 0usize);
    let mut peer_done = false;
    while tcpnet::now() < deadline {
        tcpnet::poll();
        if length == offset && !peer_done {
            match read_bytes(peer, &mut pending, false) {
                Ok(0) => peer_done = true,
                Ok(count) => {
                    length = count;
                    offset = 0;
                }
                Err(_) => {}
            }
        }
        if offset < length
            && let Ok(count) = write_bytes(peer, &pending[offset..length])
        {
            offset += count;
            report.echoed += count;
        }
        if peer_done && offset == length {
            report.closed = true;
            break;
        }
    }
    let _ = linux_close(connection_fd);
    let _ = linux_close(listener_fd);
    tcpnet::poll();
    report
}
