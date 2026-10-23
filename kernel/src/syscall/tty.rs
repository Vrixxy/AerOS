//! The Terminal's foreground jobs get a terminal: their descriptors 0, 1 and
//! 2 are still the shell's input and output pipes, but both pipes belong to a
//! `Tty`, so `isatty` and the termios, window-size and process-group ioctls
//! work on them, and what the Terminal types goes through a line discipline
//! first (echo, erase, Ctrl+C, Ctrl+D, raw mode).

use super::*;

const TTY_COUNT: usize = 4;
const LINE_MAX: usize = 1024;

const ICRNL: u32 = 0o400;
const INLCR: u32 = 0o100;
const IGNCR: u32 = 0o200;
const ISIG: u32 = 0o1;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;
const ECHOE: u32 = 0o20;
const ECHOK: u32 = 0o40;
const ECHONL: u32 = 0o100;
const ECHOCTL: u32 = 0o1000;
const IEXTEN: u32 = 0o100000;

const VINTR: usize = 0;
const VQUIT: usize = 1;
const VERASE: usize = 2;
const VKILL: usize = 3;
const VEOF: usize = 4;
const VMIN: usize = 6;
const VWERASE: usize = 14;
const CC_COUNT: usize = 19;

#[derive(Clone, Copy)]
struct Termios {
    iflag: u32,
    oflag: u32,
    cflag: u32,
    lflag: u32,
    line: u8,
    cc: [u8; CC_COUNT],
}

impl Termios {
    /// What a Linux terminal starts with (`stty sane`).
    const DEFAULT: Self = Self {
        iflag: ICRNL | 0o2000,
        oflag: 0o5,
        cflag: 0o2277,
        lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK | ECHOCTL | 0o4000 | IEXTEN,
        line: 0,
        cc: [
            3, 0x1c, 0x7f, 0x15, 4, 0, 1, 0, 0x11, 0x13, 0x1a, 0, 0x12, 0x0f, 0x17, 0x16, 0, 0, 0,
        ],
    };

    fn encode(&self) -> [u8; 36] {
        let mut bytes = [0u8; 36];
        bytes[..4].copy_from_slice(&self.iflag.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.oflag.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.cflag.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.lflag.to_le_bytes());
        bytes[16] = self.line;
        bytes[17..].copy_from_slice(&self.cc);
        bytes
    }

    fn decode(bytes: &[u8; 36]) -> Self {
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let mut cc = [0u8; CC_COUNT];
        cc.copy_from_slice(&bytes[17..]);
        Self {
            iflag: word(0),
            oflag: word(4),
            cflag: word(8),
            lflag: word(12),
            line: bytes[16],
            cc,
        }
    }
}

#[derive(Clone, Copy)]
struct Tty {
    used: bool,
    input: usize,
    output: usize,
    termios: Termios,
    rows: u16,
    columns: u16,
    foreground: u64,
    session: u64,
    line: [u8; LINE_MAX],
    line_len: usize,
}

impl Tty {
    const EMPTY: Self = Self {
        used: false,
        input: 0,
        output: 0,
        termios: Termios::DEFAULT,
        rows: 25,
        columns: 80,
        foreground: 0,
        session: 0,
        line: [0; LINE_MAX],
        line_len: 0,
    };
}

static TTYS: TicketLock<[Tty; TTY_COUNT]> = TicketLock::new([Tty::EMPTY; TTY_COUNT]);

/// Makes a terminal of an input and an output pipe (see `input_create` and
/// `capture_create`).
pub fn create(input: usize, output: usize) -> Option<usize> {
    let index = {
        let mut ttys = TTYS.lock();
        let index = ttys.iter().position(|tty| !tty.used)?;
        ttys[index] = Tty {
            used: true,
            input,
            output,
            ..Tty::EMPTY
        };
        index
    };
    let mut pipes = PIPES.lock();
    for slot in [input, output] {
        if let Some(pipe) = pipes.get_mut(slot) {
            pipe.tty = index as u8 + 1;
        }
    }
    if let Some(pipe) = pipes.get_mut(input) {
        pipe.canonical = true;
    }
    Some(index)
}

/// The job's process (and group, and session) once it has been started.
pub fn set_foreground(index: usize, pgid: u64) {
    if let Some(tty) = TTYS.lock().get_mut(index) {
        tty.foreground = pgid;
        tty.session = pgid;
    }
}

pub fn release(index: usize) {
    let Some(tty) = TTYS
        .lock()
        .get_mut(index)
        .map(|tty| core::mem::replace(tty, Tty::EMPTY))
    else {
        return;
    };
    let mut pipes = PIPES.lock();
    for slot in [tty.input, tty.output] {
        if let Some(pipe) = pipes.get_mut(slot)
            && pipe.tty == index as u8 + 1
        {
            pipe.tty = 0;
            pipe.canonical = false;
            pipe.nowait = false;
            pipe.eof_pending = 0;
        }
    }
}

/// Whether the program wants lines (the Terminal edits them itself) and
/// whether what is typed is shown.
pub fn mode(index: usize) -> (bool, bool) {
    TTYS.lock().get(index).map_or((true, true), |tty| {
        (
            tty.termios.lflag & ICANON != 0,
            tty.termios.lflag & ECHO != 0,
        )
    })
}

pub fn set_size(index: usize, rows: u16, columns: u16) {
    if let Some(tty) = TTYS.lock().get_mut(index) {
        tty.rows = rows;
        tty.columns = columns;
    }
}

fn push(pipe: usize, bytes: &[u8]) {
    let mut guard = PIPES.lock();
    let table = &mut *guard;
    if let Some(entry) = table.pipes.get_mut(pipe).filter(|entry| entry.used) {
        ring_put(&mut table.data[pipe], entry.head, &mut entry.length, bytes);
    }
}

fn echo_control(output: usize, byte: u8, lflag: u32) {
    if lflag & ECHOCTL != 0 && byte < 0x20 && byte != b'\t' && byte != b'\n' {
        push(output, &[b'^', byte + 0x40]);
    } else {
        push(output, &[byte]);
    }
}

/// What the Terminal typed, through the line discipline.
pub fn input(index: usize, bytes: &[u8]) {
    let mut signal = 0u64;
    let mut foreground = 0;
    {
        let mut ttys = TTYS.lock();
        let Some(tty) = ttys.get_mut(index).filter(|tty| tty.used) else {
            return;
        };
        let termios = tty.termios;
        let (iflag, lflag) = (termios.iflag, termios.lflag);
        for &raw in bytes {
            let byte = match raw {
                b'\r' if iflag & IGNCR != 0 => continue,
                b'\r' if iflag & ICRNL != 0 => b'\n',
                b'\n' if iflag & INLCR != 0 => b'\r',
                other => other,
            };
            if lflag & ISIG != 0 {
                let raised = [(VINTR, 2), (VQUIT, 3)]
                    .into_iter()
                    .find(|&(at, _)| termios.cc[at] != 0 && termios.cc[at] == byte);
                if let Some((_, number)) = raised {
                    tty.line_len = 0;
                    if lflag & ECHO != 0 {
                        echo_control(tty.output, byte, lflag);
                    }
                    signal = number;
                    foreground = tty.foreground;
                    continue;
                }
            }
            if lflag & ICANON == 0 {
                push(tty.input, &[byte]);
                if lflag & ECHO != 0 {
                    echo_control(tty.output, byte, lflag);
                }
                continue;
            }
            if byte == termios.cc[VERASE] || byte == 0x08 {
                if tty.line_len > 0 {
                    tty.line_len -= 1;
                    if lflag & ECHO != 0 && lflag & ECHOE != 0 {
                        push(tty.output, b"\x08 \x08");
                    }
                }
            } else if byte == termios.cc[VKILL]
                || (lflag & IEXTEN != 0 && byte == termios.cc[VWERASE])
            {
                let keep = if byte == termios.cc[VKILL] {
                    0
                } else {
                    let line = &tty.line[..tty.line_len];
                    let end = line.iter().rposition(|&c| c != b' ').map_or(0, |at| at + 1);
                    line[..end]
                        .iter()
                        .rposition(|&c| c == b' ')
                        .map_or(0, |at| at + 1)
                };
                if lflag & ECHO != 0 {
                    for _ in keep..tty.line_len {
                        push(tty.output, b"\x08 \x08");
                    }
                }
                tty.line_len = keep;
            } else if byte == termios.cc[VEOF] {
                let length = tty.line_len;
                tty.line_len = 0;
                if length == 0 {
                    let mut pipes = PIPES.lock();
                    if let Some(pipe) = pipes.get_mut(tty.input) {
                        pipe.eof_pending = pipe.eof_pending.saturating_add(1);
                    }
                } else {
                    let line = tty.line;
                    push(tty.input, &line[..length]);
                }
            } else {
                if tty.line_len < LINE_MAX {
                    tty.line[tty.line_len] = byte;
                    tty.line_len += 1;
                }
                if lflag & ECHO != 0 || (byte == b'\n' && lflag & ECHONL != 0) {
                    echo_control(tty.output, byte, lflag);
                }
                if byte == b'\n' || tty.line_len == LINE_MAX {
                    let (line, length) = (tty.line, tty.line_len);
                    tty.line_len = 0;
                    push(tty.input, &line[..length]);
                }
            }
        }
    }
    if signal != 0 {
        signal_group(foreground, signal);
    }
}

/// A signal for every process of the foreground group; a process it would
/// end is ended (as `Shell::job_interrupt` does).
fn signal_group(pgid: u64, signal: u64) {
    let mut members = [0u64; 16];
    let count = crate::scheduler::group_members(pgid, &mut members);
    for &id in &members[..count] {
        if matches!(
            crate::scheduler::signal_other_task(id, signal),
            Some(RemoteSignal::Fatal) | None
        ) {
            let _ = crate::scheduler::kill_task(id);
        }
    }
}

/// `TCSETS`: leaving canonical mode makes the half-typed line readable, and
/// `flush` (`TCSETSF`) throws away what has not been read.
fn set_termios(index: usize, termios: Termios, flush: bool) {
    let (input, line, length) = {
        let mut ttys = TTYS.lock();
        let tty = &mut ttys[index];
        let leaving = tty.termios.lflag & ICANON != 0 && termios.lflag & ICANON == 0;
        tty.termios = termios;
        let length = if leaving && !flush { tty.line_len } else { 0 };
        if leaving || flush {
            tty.line_len = 0;
        }
        (tty.input, tty.line, length)
    };
    let mut guard = PIPES.lock();
    let table = &mut *guard;
    if let Some(pipe) = table.pipes.get_mut(input) {
        pipe.canonical = termios.lflag & ICANON != 0;
        pipe.nowait = !pipe.canonical && termios.cc[VMIN] == 0;
        if flush {
            pipe.length = 0;
        }
        ring_put(
            &mut table.data[input],
            pipe.head,
            &mut pipe.length,
            &line[..length],
        );
    }
}

fn tty_of(process_fd: &ProcessFd) -> Option<usize> {
    if !process_fd.pipe {
        return None;
    }
    let index = PIPES.lock().get(process_fd.handle as usize)?.tty;
    index.checked_sub(1).map(usize::from)
}

/// The terminal ioctls on a terminal's pipe; `None` for any other descriptor.
pub(super) fn ioctl(process_fd: &ProcessFd, request: u64, address: u64) -> Option<u64> {
    let index = tty_of(process_fd)?;
    let copy_out = |bytes: &[u8]| {
        if user::copy_to_user(address, bytes) {
            0
        } else {
            error(14)
        }
    };
    let result = match request {
        0x5401 => {
            let termios = TTYS.lock()[index].termios;
            copy_out(&termios.encode())
        }
        0x5402..=0x5404 => {
            let mut bytes = [0u8; 36];
            if !user::copy_from_user(address, &mut bytes) {
                return Some(error(14));
            }
            set_termios(index, Termios::decode(&bytes), request == 0x5404);
            0
        }
        0x540f => {
            let foreground = TTYS.lock()[index].foreground as u32;
            copy_out(&foreground.to_le_bytes())
        }
        0x5429 => {
            let session = TTYS.lock()[index].session as u32;
            copy_out(&session.to_le_bytes())
        }
        0x5410 => {
            let mut bytes = [0u8; 4];
            if !user::copy_from_user(address, &mut bytes) {
                return Some(error(14));
            }
            let pgid = u64::from(u32::from_le_bytes(bytes));
            if crate::scheduler::group_members(pgid, &mut [0u64; 1]) == 0 {
                return Some(error(1));
            }
            TTYS.lock()[index].foreground = pgid;
            0
        }
        0x5413 => {
            let (rows, columns) = {
                let tty = &TTYS.lock()[index];
                (tty.rows, tty.columns)
            };
            let mut size = [0u8; 8];
            size[..2].copy_from_slice(&rows.to_le_bytes());
            size[2..4].copy_from_slice(&columns.to_le_bytes());
            copy_out(&size)
        }
        0x5414 => {
            let mut size = [0u8; 8];
            if !user::copy_from_user(address, &mut size) {
                return Some(error(14));
            }
            set_size(
                index,
                u16::from_le_bytes([size[0], size[1]]),
                u16::from_le_bytes([size[2], size[3]]),
            );
            0
        }
        0x540b => {
            if address == 0 || address == 2 {
                let input = {
                    let mut ttys = TTYS.lock();
                    ttys[index].line_len = 0;
                    ttys[index].input
                };
                if let Some(pipe) = PIPES.lock().get_mut(input) {
                    pipe.length = 0;
                }
            }
            0
        }
        0x5411 => copy_out(&0u32.to_le_bytes()),
        0x5409 | 0x540a | 0x540e | 0x5422 | 0x5425 => 0,
        _ => error(25),
    };
    Some(result)
}

/// The line discipline on its own: echo and erase in canonical mode, a line
/// at a time, Ctrl+D as one end of file, `TCSETS` into raw mode and back.
#[cfg(feature = "boot-test")]
pub(crate) fn self_test() -> bool {
    let (Some(keys), Some(output)) = (input_create(), capture_create()) else {
        return false;
    };
    let Some(index) = create(keys, output) else {
        capture_close(keys);
        input_close(keys);
        capture_close(output);
        input_close(output);
        return false;
    };
    let mut buffer = [0u8; 64];
    input(index, b"helo\x7flo\rsecond\r");
    let first = pipe_read(keys, &mut buffer) == Ok(6) && &buffer[..6] == b"hello\n";
    let second = pipe_read(keys, &mut buffer) == Ok(7) && &buffer[..7] == b"second\n";
    let echoed = capture_read(output, &mut buffer);
    let echo_ok = &buffer[..echoed] == b"helo\x08 \x08lo\nsecond\n";
    input(index, b"\x04");
    let eof_once = pipe_read(keys, &mut buffer) == Ok(0);
    input(index, b"x");
    let waits = pipe_read(keys, &mut buffer) == Err(11);
    let _ = capture_read(output, &mut buffer);
    let mut raw = TTYS.lock()[index].termios;
    raw.lflag &= !(ICANON | ECHO);
    raw.cc[VMIN] = 1;
    set_termios(index, raw, false);
    input(index, b"ab");
    let raw_ok = pipe_read(keys, &mut buffer) == Ok(3)
        && &buffer[..3] == b"xab"
        && capture_read(output, &mut buffer) == 0;
    let (canonical, echo) = mode(index);
    release(index);
    let released = PIPES
        .lock()
        .get(keys)
        .is_some_and(|pipe| pipe.tty == 0 && !pipe.canonical);
    capture_close(keys);
    input_close(keys);
    capture_close(output);
    input_close(output);
    first && second && echo_ok && eof_once && waits && raw_ok && !canonical && !echo && released
}
