//! `linux` in the Terminal: what is typed runs in the Debian guest. Each
//! command goes to the guest's agent (which runs it with `sh -c`, output to
//! a file), and the file comes back through the agent's page fetcher while
//! it grows, so long commands (`apt install`) stream into the scrollback
//! without holding up the desktop. Programs that need a real terminal
//! (vim, htop) are not supported yet.

use super::*;

/// Where a command's output collects in the guest.
const OUTPUT: &str = "/tmp/.aeros-term";
const OUTPUT_URL: &[u8] = b"file:///tmp/.aeros-term";
/// The agent's command mailbox holds 128 bytes, terminator included.
const COMMAND_BYTES: usize = 127;
const POLL_NS: u64 = 250_000_000;
const CWD_BYTES: usize = 96;

static FETCHED: crate::sync::TicketLock<[u8; 64_000]> = crate::sync::TicketLock::new([0; 64_000]);

#[derive(Clone, Copy)]
pub(super) struct LinuxShell {
    /// The Terminal is in Linux mode (`linux` typed alone).
    pub(super) active: bool,
    cwd: [u8; CWD_BYTES],
    cwd_len: usize,
    /// The running command's number, 0 when none runs.
    job: u32,
    sequence: u32,
    /// Output lines already in the scrollback.
    shown: usize,
    seen: u32,
    asked: bool,
    next_ns: u64,
    /// The running command is a `cd`: its output is the new directory.
    changing_directory: bool,
}

impl LinuxShell {
    pub(super) const EMPTY: Self = Self {
        active: false,
        cwd: [0; CWD_BYTES],
        cwd_len: 0,
        job: 0,
        sequence: 0,
        shown: 0,
        seen: 0,
        asked: false,
        next_ns: 0,
        changing_directory: false,
    };

    fn cwd(&self) -> &str {
        core::str::from_utf8(&self.cwd[..self.cwd_len]).unwrap_or("/root")
    }

    fn set_cwd(&mut self, path: &str) {
        let take = path.len().min(CWD_BYTES);
        self.cwd[..take].copy_from_slice(&path.as_bytes()[..take]);
        self.cwd_len = take;
    }

    pub(super) fn running(&self) -> bool {
        self.job != 0
    }

    /// The prompt in Linux mode (nothing while a command runs).
    pub(super) fn prompt(&self, out: &mut impl Write) {
        if !self.running() {
            let _ = write!(out, "root@debian:{}#", self.cwd());
        }
    }
}

/// Whether a submitted line belongs to Linux mode.
pub(super) fn claims(state: &DesktopState, line: &str) -> bool {
    state.linux_sh.active || line == "linux" || line.starts_with("linux ")
}

pub(super) fn submit(state: &mut DesktopState, line: &str) {
    let line = line.trim();
    if !state.linux_sh.active {
        if !crate::svm::linux_ready() {
            state.terminal_push_wrapped(format_line(format_args!("{}> {line}", state.display_name())).as_str());
            state.terminal_push_line("Linux is not running on this computer.");
            return;
        }
        if state.linux_sh.cwd_len == 0 {
            state.linux_sh.set_cwd("/root");
        }
        if line == "linux" {
            state.terminal_push_wrapped(format_line(format_args!("{}> linux", state.display_name())).as_str());
            state.linux_sh.active = true;
            state.terminal_push_line("Debian shell: commands run in Linux. Type exit to go back to AerOS.");
            return;
        }
        // `linux <command>`: one command, then back to the AerOS prompt.
        state.terminal_push_wrapped(format_line(format_args!("{}> {line}", state.display_name())).as_str());
        start(state, line["linux ".len()..].trim());
        return;
    }
    state.terminal_push_wrapped(format_line(format_args!("root@debian:{}# {line}", state.linux_sh.cwd())).as_str());
    if state.linux_sh.running() {
        state.terminal_push_line("A command is still running (Ctrl+C stops it).");
        return;
    }
    match line {
        "" => {}
        "exit" | "logout" => {
            state.linux_sh.active = false;
            state.terminal_push_line("Back in AerOS.");
        }
        _ => start(state, line),
    }
}

fn format_line(arguments: core::fmt::Arguments<'_>) -> shell::Text<{ TERMINAL_INPUT_MAX + 160 }> {
    let mut text = shell::Text::new();
    let _ = text.write_fmt(arguments);
    text
}

fn start(state: &mut DesktopState, command: &str) {
    if command.is_empty() {
        return;
    }
    let sequence = state.linux_sh.sequence.wrapping_add(1).max(1);
    let mut body: shell::Text<256> = shell::Text::new();
    let changing = command == "cd" || command.starts_with("cd ");
    if changing {
        let target = command[2..].trim();
        let _ = write!(body, "cd {} && pwd", if target.is_empty() { "~" } else { target });
    } else if let Some(rest) = command
        .strip_prefix("apt ")
        .or_else(|| command.strip_prefix("apt-get "))
    {
        // No terminal to answer questions on: say yes and take defaults.
        let tool = if command.starts_with("apt-get") { "apt-get" } else { "apt" };
        let asks = ["install", "remove", "purge", "upgrade", "full-upgrade", "autoremove", "dist-upgrade"]
            .iter()
            .any(|word| rest.split_whitespace().next() == Some(*word));
        let yes = if asks && !rest.contains("-y") { " -y" } else { "" };
        let _ = write!(body, "DEBIAN_FRONTEND=noninteractive {tool}{yes} {rest}");
    } else {
        let _ = body.push_str_checked(command);
    }
    let mut script: shell::Text<512> = shell::Text::new();
    let _ = write!(
        script,
        "cd '{}';echo @@S{sequence}>{OUTPUT};({})</dev/null>>{OUTPUT} 2>&1;echo @@E{sequence}_$?>>{OUTPUT}",
        state.linux_sh.cwd(),
        body.as_str()
    );
    if script.as_str().len() > COMMAND_BYTES {
        let room = COMMAND_BYTES.saturating_sub(script.as_str().len() - body.as_str().len());
        state.terminal_push_wrapped(format_line(format_args!(
            "That command is too long for the Linux bridge here (about {room} characters fit)."
        )).as_str());
        return;
    }
    crate::svm::linux_command(3, [0; 4], script.as_str().as_bytes());
    let shell = &mut state.linux_sh;
    shell.sequence = sequence;
    shell.job = sequence;
    shell.shown = 0;
    shell.asked = false;
    shell.changing_directory = changing;
    shell.next_ns = crate::time::monotonic_nanoseconds() + POLL_NS / 2;
}

/// Ctrl+C: interrupts the running command's whole process group.
pub(super) fn interrupt(state: &mut DesktopState) -> bool {
    let job = state.linux_sh.job;
    if job == 0 {
        return false;
    }
    let mut kill: shell::Text<96> = shell::Text::new();
    let _ = write!(kill, "kill -INT -- -$(pgrep -of '@@S{job}>')");
    crate::svm::linux_command(3, [0; 4], kill.as_str().as_bytes());
    state.terminal_push_line("^C");
    state.linux_sh.job = 0;
    true
}

/// Fetches the running command's output and shows what is new. True when
/// the scrollback changed.
pub(super) fn poll(state: &mut DesktopState, now: u64) -> bool {
    let job = state.linux_sh.job;
    if job == 0 || now < state.linux_sh.next_ns {
        return false;
    }
    if !state.linux_sh.asked {
        state.linux_sh.asked = crate::svm::linux_web_request(OUTPUT_URL, 200);
        state.linux_sh.next_ns = now + POLL_NS / 5;
        return false;
    }
    let mut buffer = FETCHED.lock();
    let mut seen = state.linux_sh.seen;
    let Some((status, length)) = crate::svm::linux_web_take(&mut seen, &mut buffer[..]) else {
        return false;
    };
    state.linux_sh.seen = seen;
    // 2 = the fetch has started: the file follows in a later answer, and
    // asking again now would cancel this one.
    if status == 2 {
        return false;
    }
    state.linux_sh.asked = false;
    state.linux_sh.next_ns = now + POLL_NS;
    let text = core::str::from_utf8(&buffer[..length]).unwrap_or("");
    let mut header = shell::Text::<16>::new();
    let _ = write!(header, "@@S{job}");
    let mut end = shell::Text::<16>::new();
    let _ = write!(end, "@@E{job}_");
    let mut lines = text.lines();
    // An older command's file, or w3m's message before the file exists.
    if lines.next().map(str::trim) != Some(header.as_str()) {
        return false;
    }
    let mut changed = false;
    let mut status = None;
    let mut last = "";
    for (index, line) in lines.enumerate() {
        if let Some(code) = line.trim().strip_prefix(end.as_str()) {
            status = Some(code.trim().parse::<i32>().unwrap_or(-1));
            break;
        }
        last = line;
        if index >= state.linux_sh.shown {
            if !state.linux_sh.changing_directory {
                state.terminal_push_wrapped(line.trim_end());
            }
            state.linux_sh.shown = index + 1;
            changed = true;
        }
    }
    if let Some(code) = status {
        if state.linux_sh.changing_directory {
            if code == 0 && last.starts_with('/') {
                state.linux_sh.set_cwd(last.trim());
            } else {
                state.terminal_push_line("cd: no such directory");
            }
        } else if code != 0 {
            state.terminal_push_wrapped(format_line(format_args!("[exit {code}]")).as_str());
        }
        state.linux_sh.job = 0;
        changed = true;
    }
    changed
}
