use core::arch::asm;
use core::fmt::{self, Write};

use crate::ahci::AhciReport;
use crate::arch::CpuInfo;
use crate::e1000::NetworkReport;
use crate::fat::FatReport;
use crate::memory::{AllocatorStats, BootMemoryMap};
use crate::net::InternetReport;
use crate::pci::PciInventory;
use crate::{heap, process, rtc, scheduler, time, vfs};

const MAX_ARGUMENTS: usize = 16;
const MAX_ARGUMENT_BYTES: usize = 96;
pub(crate) const MAX_INPUT: usize = 192;
const MAX_PATH: usize = 256;
pub(crate) const MAX_OUTPUT: usize = 8192;
const HISTORY_ITEMS: usize = 16;
const ENVIRONMENT_ITEMS: usize = 12;
const STARTUP_SCRIPT_PATH: &str = "/home/.aershrc";

#[derive(Clone, Copy)]
pub struct SystemInfo<'a> {
    pub version: &'static str,
    pub cpu: &'a CpuInfo,
    pub memory: &'a BootMemoryMap,
    pub allocator: AllocatorStats,
    pub pci: &'a PciInventory,
    #[allow(dead_code)]
    pub storage: AhciReport,
    pub fat: FatReport,
    pub network: NetworkReport,
    pub internet: InternetReport,
    pub cpu_count: usize,
}

#[derive(Clone, Copy)]
pub struct ShellReport {
    pub commands: usize,
    pub unique: bool,
    pub parser: bool,
    pub privilege: bool,
    pub filesystem: bool,
    pub reauth: bool,
    pub redirection: bool,
    pub startup: bool,
    pub symlinks: bool,
    pub background_jobs: bool,
    pub firewall_command: bool,
    pub dmesg_command: bool,
    pub service_command: bool,
    pub text_tools: bool,
    pub priority_command: bool,
    pub crashes_command: bool,
    pub bench_command: bool,
    pub sigcheck_command: bool,
    pub pipelines: bool,
    pub strace_command: bool,
    pub fsck_command: bool,
    pub verified: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Handler {
    Notify,
    Help,
    Man,
    Clear,
    Echo,
    Pwd,
    Cd,
    Ls,
    Cat,
    Head,
    Tail,
    Wc,
    Stat,
    Touch,
    Mkdir,
    Rm,
    Ln,
    Readlink,
    Cp,
    Mv,
    Chmod,
    Uname,
    Hostname,
    Whoami,
    Id,
    Date,
    Uptime,
    Sleep,
    Ps,
    Kill,
    Jobs,
    Wait,
    Fw,
    Dmesg,
    Svc,
    Pkg,
    Fsck,
    Aerfs,
    Install,
    Acpi,
    Battery,
    Thermal,
    Swap,
    Measure,
    Update,
    Strace,
    Sigcheck,
    Bench,
    Crashes,
    Renice,
    Grep,
    Uniq,
    Tee,
    Find,
    Du,
    Sort,
    Basename,
    Dirname,
    Seq,
    True,
    False,
    Free,
    Lscpu,
    Lspci,
    Lsblk,
    Mount,
    Umount,
    Df,
    Sync,
    Which,
    Env,
    Export,
    Unset,
    History,
    Ip,
    Ping,
    Dns,
    Https,
    Route,
    Arp,
    Netstat,
    Reboot,
    Shutdown,
    Ear,
    Run,
    Av,
    Passwd,
}

#[derive(Clone, Copy)]
struct CommandSpec {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    privileged: bool,
    handler: Handler,
}

const fn command(
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    privileged: bool,
    handler: Handler,
) -> CommandSpec {
    CommandSpec {
        name,
        usage,
        summary,
        privileged,
        handler,
    }
}

static COMMANDS: [CommandSpec; 87] = [
    command(
        "notify",
        "notify <message...>",
        "show a desktop notification",
        false,
        Handler::Notify,
    ),
    command(
        "help",
        "help [command]",
        "list commands or show command help",
        false,
        Handler::Help,
    ),
    command(
        "man",
        "man <command>",
        "show command usage",
        false,
        Handler::Man,
    ),
    command(
        "clear",
        "clear",
        "clear the terminal",
        false,
        Handler::Clear,
    ),
    command("echo", "echo [text...]", "print text", false, Handler::Echo),
    command(
        "pwd",
        "pwd",
        "print the working directory",
        false,
        Handler::Pwd,
    ),
    command(
        "cd",
        "cd [path]",
        "change the working directory",
        false,
        Handler::Cd,
    ),
    command(
        "ls",
        "ls [-l] [path]",
        "list a directory",
        false,
        Handler::Ls,
    ),
    command(
        "cat",
        "cat <file>",
        "print a text file",
        false,
        Handler::Cat,
    ),
    command(
        "head",
        "head [-n count] <file>",
        "print the first lines",
        false,
        Handler::Head,
    ),
    command(
        "tail",
        "tail [-n count] <file>",
        "print the last lines",
        false,
        Handler::Tail,
    ),
    command(
        "wc",
        "wc <file>",
        "count lines, words, and bytes",
        false,
        Handler::Wc,
    ),
    command(
        "stat",
        "stat <path>",
        "show inode metadata",
        false,
        Handler::Stat,
    ),
    command(
        "touch",
        "touch <file>",
        "create a tmpfs file",
        false,
        Handler::Touch,
    ),
    command(
        "mkdir",
        "mkdir <directory>",
        "create a tmpfs directory",
        false,
        Handler::Mkdir,
    ),
    command(
        "rm",
        "rm [-d] <path>",
        "remove a tmpfs file or empty directory",
        false,
        Handler::Rm,
    ),
    command(
        "ln",
        "ln -s <target> <link>",
        "create a symbolic link",
        false,
        Handler::Ln,
    ),
    command(
        "readlink",
        "readlink <path>",
        "print a symbolic link's target",
        false,
        Handler::Readlink,
    ),
    command(
        "cp",
        "cp <source> <destination>",
        "copy a file into tmpfs",
        false,
        Handler::Cp,
    ),
    command(
        "mv",
        "mv <source> <destination>",
        "rename or move a tmpfs node",
        false,
        Handler::Mv,
    ),
    command(
        "chmod",
        "chmod <mode> <path>",
        "change tmpfs permissions",
        true,
        Handler::Chmod,
    ),
    command(
        "uname",
        "uname [-a]",
        "show operating-system information",
        false,
        Handler::Uname,
    ),
    command(
        "hostname",
        "hostname [name]",
        "show or set the host name",
        false,
        Handler::Hostname,
    ),
    command(
        "whoami",
        "whoami",
        "print the effective user",
        false,
        Handler::Whoami,
    ),
    command(
        "id",
        "id",
        "print user and privilege identity",
        false,
        Handler::Id,
    ),
    command(
        "passwd",
        "passwd",
        "change the account password",
        false,
        Handler::Passwd,
    ),
    command(
        "date",
        "date",
        "show UTC wall-clock time",
        false,
        Handler::Date,
    ),
    command(
        "uptime",
        "uptime",
        "show monotonic uptime",
        false,
        Handler::Uptime,
    ),
    command(
        "sleep",
        "sleep <milliseconds>",
        "wait for a bounded interval",
        false,
        Handler::Sleep,
    ),
    command("ps", "ps", "show process-table state", false, Handler::Ps),
    command(
        "kill",
        "kill <pid>",
        "request process termination",
        true,
        Handler::Kill,
    ),
    command(
        "jobs",
        "jobs",
        "list `run ... &` background jobs",
        false,
        Handler::Jobs,
    ),
    command(
        "wait",
        "wait <task_id>",
        "block until a task exits and print its status",
        false,
        Handler::Wait,
    ),
    command(
        "fw",
        "fw [deny <tcp|udp|any> <port>|clear]",
        "list or edit firewall rules",
        true,
        Handler::Fw,
    ),
    command(
        "sigcheck",
        "sigcheck <file> <signature-file> <public-key-hex>",
        "verify an Ed25519 signature over a file",
        false,
        Handler::Sigcheck,
    ),
    command(
        "bench",
        "bench [smp]",
        "run short performance micro-benchmarks (smp: hash on every processor)",
        false,
        Handler::Bench,
    ),
    command(
        "measure",
        "measure [seal]",
        "show the boot measurements and whether the boot image matches the sealed one",
        true,
        Handler::Measure,
    ),
    command(
        "update",
        "update <image> <signature> | update rollback",
        "install a signed boot image (signed by a key trusted with `pkg trust`), or go back",
        true,
        Handler::Update,
    ),
    command(
        "swap",
        "swap",
        "show the swap area: size, use and pages moved in and out",
        false,
        Handler::Swap,
    ),
    command(
        "acpi",
        "acpi [tree | eval <path> [args] | crs <path>]",
        "inspect the ACPI namespace: summary, devices, evaluate an object, resources",
        false,
        Handler::Acpi,
    ),
    command(
        "battery",
        "battery",
        "show batteries, the mains adapter and the lid as the firmware reports them",
        false,
        Handler::Battery,
    ),
    command(
        "thermal",
        "thermal",
        "show thermal zone temperatures and trip points, and the ACPI event counters",
        false,
        Handler::Thermal,
    ),
    command(
        "install",
        "install [<disk> --yes]",
        "list the disks the system can be installed on, or install onto one",
        true,
        Handler::Install,
    ),
    command(
        "aerfs",
        "aerfs format|mount <disk> [label]",
        "format a whole disk as AerFS (crash-safe, checksummed) or mount one, under /media",
        true,
        Handler::Aerfs,
    ),
    command(
        "fsck",
        "fsck [--repair] [mount-point]",
        "check a /home or /media filesystem, optionally repairing it",
        true,
        Handler::Fsck,
    ),
    command(
        "strace",
        "strace [lines]",
        "show the most recent Linux syscalls",
        true,
        Handler::Strace,
    ),
    command(
        "crashes",
        "crashes [save [path]]",
        "list processes killed by CPU faults, or save a crash report",
        true,
        Handler::Crashes,
    ),
    command(
        "renice",
        "renice <nice -20..19> <pid>",
        "change a process's scheduling priority",
        true,
        Handler::Renice,
    ),
    command(
        "grep",
        "grep [-i] [-n] [-c] <text> <file>",
        "print lines containing text",
        false,
        Handler::Grep,
    ),
    command(
        "find",
        "find [path] [-name text]",
        "list files below a directory",
        false,
        Handler::Find,
    ),
    command(
        "du",
        "du [path]",
        "total size of files below a path",
        false,
        Handler::Du,
    ),
    command(
        "uniq",
        "uniq [-c] [file]",
        "collapse adjacent duplicate lines",
        false,
        Handler::Uniq,
    ),
    command(
        "tee",
        "tee <file>",
        "copy piped input to a file and to the output",
        false,
        Handler::Tee,
    ),
    command(
        "sort",
        "sort [-r] <file>",
        "print a file's lines in order",
        false,
        Handler::Sort,
    ),
    command(
        "basename",
        "basename <path>",
        "last component of a path",
        false,
        Handler::Basename,
    ),
    command(
        "dirname",
        "dirname <path>",
        "path without its last component",
        false,
        Handler::Dirname,
    ),
    command(
        "seq",
        "seq [first] <last>",
        "print a range of numbers",
        false,
        Handler::Seq,
    ),
    command("true", "true", "succeed", false, Handler::True),
    command("false", "false", "fail", false, Handler::False),
    command(
        "svc",
        "svc [list|add <name> <path>|start|stop|restart|rm <name>]",
        "manage named background services",
        true,
        Handler::Svc,
    ),
    command(
        "pkg",
        "pkg [list|install <file> [sig]|remove|rollback|verify|path <name>|trust <key-hex>]",
        "install signed packages, roll back, verify",
        true,
        Handler::Pkg,
    ),
    command(
        "dmesg",
        "dmesg [lines]",
        "show the recent kernel log",
        true,
        Handler::Dmesg,
    ),
    command(
        "free",
        "free",
        "show physical and heap memory",
        false,
        Handler::Free,
    ),
    command(
        "lscpu",
        "lscpu",
        "show CPU capabilities",
        false,
        Handler::Lscpu,
    ),
    command(
        "lspci",
        "lspci",
        "list PCI functions",
        false,
        Handler::Lspci,
    ),
    command(
        "lsblk",
        "lsblk",
        "show native block devices",
        false,
        Handler::Lsblk,
    ),
    command(
        "mount",
        "mount [--bind <volume directory> <directory>]",
        "list mounted filesystems, or bind a volume directory over a directory",
        false,
        Handler::Mount,
    ),
    command(
        "umount",
        "umount <path>",
        "unmount a filesystem or a bind mount",
        true,
        Handler::Umount,
    ),
    command("df", "df", "show filesystem capacity", false, Handler::Df),
    command(
        "sync",
        "sync",
        "flush writable filesystems",
        true,
        Handler::Sync,
    ),
    command(
        "which",
        "which <command>",
        "locate a shell command",
        false,
        Handler::Which,
    ),
    command(
        "env",
        "env",
        "list environment variables",
        false,
        Handler::Env,
    ),
    command(
        "export",
        "export NAME=VALUE",
        "set an environment variable",
        false,
        Handler::Export,
    ),
    command(
        "unset",
        "unset <name>",
        "remove an environment variable",
        false,
        Handler::Unset,
    ),
    command(
        "history",
        "history",
        "show recent commands",
        false,
        Handler::History,
    ),
    command(
        "ip",
        "ip",
        "show network interface state",
        false,
        Handler::Ip,
    ),
    command(
        "ping",
        "ping [address]",
        "probe or report IPv4 reachability",
        false,
        Handler::Ping,
    ),
    command(
        "dns",
        "dns [name]",
        "resolve a DNS name",
        false,
        Handler::Dns,
    ),
    command(
        "https",
        "https <host> [path]",
        "fetch a page over TLS 1.3, checking the server's certificate",
        false,
        Handler::Https,
    ),
    command(
        "route",
        "route",
        "show the IPv4 route",
        false,
        Handler::Route,
    ),
    command(
        "arp",
        "arp",
        "show the gateway neighbor",
        false,
        Handler::Arp,
    ),
    command(
        "netstat",
        "netstat",
        "show network protocol state",
        false,
        Handler::Netstat,
    ),
    command(
        "reboot",
        "reboot",
        "restart the machine",
        true,
        Handler::Reboot,
    ),
    command(
        "shutdown",
        "shutdown",
        "halt the machine safely",
        true,
        Handler::Shutdown,
    ),
    command(
        "ear",
        "ear <command> [args...]",
        "execute one command as root",
        false,
        Handler::Ear,
    ),
    command(
        "run",
        "run <path> [args...]",
        "load and execute a real ELF binary as a scheduled process",
        false,
        Handler::Run,
    ),
    command(
        "av",
        "av <scan [--clean] [path]|status|log|realtime [on|off]|hash <file>|quarantine|restore <id>|delete <id>|import [path]|test>",
        "AerOS Shield antivirus: scan, quarantine, status",
        false,
        Handler::Av,
    ),
];

#[derive(Clone, Copy)]
struct Argument {
    bytes: [u8; MAX_ARGUMENT_BYTES],
    len: usize,
}

impl Argument {
    const EMPTY: Self = Self {
        bytes: [0; MAX_ARGUMENT_BYTES],
        len: 0,
    };

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

#[derive(Clone, Copy)]
struct Arguments {
    items: [Argument; MAX_ARGUMENTS],
    count: usize,
}

impl Arguments {
    fn parse(line: &str) -> Result<Self, &'static str> {
        let mut parsed = Self {
            items: [Argument::EMPTY; MAX_ARGUMENTS],
            count: 0,
        };
        let mut quote = 0u8;
        let mut escaped = false;
        let mut token = Argument::EMPTY;
        let mut started = false;
        for byte in line.bytes() {
            if escaped {
                push_argument_byte(&mut token, byte)?;
                started = true;
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                started = true;
                continue;
            }
            if quote != 0 {
                if byte == quote {
                    quote = 0;
                } else {
                    push_argument_byte(&mut token, byte)?;
                }
                started = true;
                continue;
            }
            if byte == b'\'' || byte == b'"' {
                quote = byte;
                started = true;
            } else if byte.is_ascii_whitespace() {
                if started {
                    parsed.push(token)?;
                    token = Argument::EMPTY;
                    started = false;
                }
            } else if byte.is_ascii_control() {
                return Err("control character in command");
            } else {
                push_argument_byte(&mut token, byte)?;
                started = true;
            }
        }
        if escaped || quote != 0 {
            return Err("unterminated quote or escape");
        }
        if started {
            parsed.push(token)?;
        }
        Ok(parsed)
    }

    fn push(&mut self, value: Argument) -> Result<(), &'static str> {
        if self.count == MAX_ARGUMENTS {
            return Err("too many arguments");
        }
        self.items[self.count] = value;
        self.count += 1;
        Ok(())
    }

    fn get(&self, index: usize) -> Option<&str> {
        self.items
            .get(index)
            .filter(|_| index < self.count)
            .map(Argument::as_str)
    }
}

/// A trailing `> path` or `>> path` on a command line: real shell output
/// redirection, not piping between commands (that would need every one of
/// the 54 built-in handlers to gain a stdin-reading mode, since they're
/// in-kernel functions writing into one shared output buffer rather than
/// real spawned processes connected by pipes - out of scope here).
#[derive(Clone, Copy)]
struct Redirection {
    path: Argument,
    append: bool,
}

/// Strips a trailing `>`/`>>` and its filename off `arguments` (so the
/// command being dispatched never sees them as its own argv) and reports
/// what to do with its output afterward.
fn extract_redirection(arguments: &mut Arguments) -> Option<Redirection> {
    if arguments.count < 2 {
        return None;
    }
    let operator = arguments.items[arguments.count - 2].as_str();
    let append = operator == ">>";
    if operator != ">" && !append {
        return None;
    }
    let path = arguments.items[arguments.count - 1];
    arguments.count -= 2;
    Some(Redirection { path, append })
}

const MAX_PIPELINE_STAGES: usize = 4;

enum InputFailure {
    Missing,
    InvalidPath,
    Vfs(vfs::VfsError),
}

/// Splits a command line at `|` characters that are not quoted or escaped.
/// `None` means the line has no pipe; otherwise the stages and how many there
/// are (one more than `MAX_PIPELINE_STAGES` means the line has too many).
fn split_pipeline(line: &str) -> Option<([&str; MAX_PIPELINE_STAGES + 1], usize)> {
    let mut stages = [""; MAX_PIPELINE_STAGES + 1];
    let (mut count, mut start) = (0usize, 0usize);
    let (mut quote, mut escaped) = (0u8, false);
    for (index, byte) in line.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if quote != 0 {
            if byte == quote {
                quote = 0;
            }
        } else if byte == b'\'' || byte == b'"' {
            quote = byte;
        } else if byte == b'|' {
            if count <= MAX_PIPELINE_STAGES {
                stages[count] = &line[start..index];
            }
            count += 1;
            start = index + 1;
        }
    }
    if count == 0 {
        return None;
    }
    if count <= MAX_PIPELINE_STAGES {
        stages[count] = &line[start..];
    }
    Some((stages, (count + 1).min(MAX_PIPELINE_STAGES + 1)))
}

fn push_argument_byte(argument: &mut Argument, byte: u8) -> Result<(), &'static str> {
    if argument.len == MAX_ARGUMENT_BYTES {
        return Err("argument too long");
    }
    argument.bytes[argument.len] = byte;
    argument.len += 1;
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
    truncated: bool,
}

impl<const N: usize> Text<N> {
    pub(crate) const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
            truncated: false,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.len = 0;
        self.truncated = false;
    }

    pub(crate) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    pub(crate) fn push_byte(&mut self, byte: u8) -> bool {
        if self.len == N {
            self.truncated = true;
            return false;
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        true
    }

    pub(crate) fn push_str_checked(&mut self, value: &str) -> bool {
        if value.len() > N.saturating_sub(self.len) {
            self.truncated = true;
            return false;
        }
        self.bytes[self.len..self.len + value.len()].copy_from_slice(value.as_bytes());
        self.len += value.len();
        true
    }

    /// Removes the last complete character, not just the last byte, so
    /// content holding multi-byte UTF-8 (e.g. a Notes document) backspaces
    /// correctly instead of leaving a truncated sequence behind. Behaves
    /// exactly like removing one byte for plain-ASCII content, since every
    /// ASCII byte is already a complete character on its own.
    pub(crate) fn backspace(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        self.len -= 1;
        while self.len > 0 && self.bytes[self.len] & 0xc0 == 0x80 {
            self.len -= 1;
        }
        true
    }
}

impl<const N: usize> Write for Text<N> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.push_str_checked(value) {
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}

#[derive(Clone, Copy)]
struct EnvironmentEntry {
    name: Text<24>,
    value: Text<96>,
    used: bool,
}

impl EnvironmentEntry {
    const EMPTY: Self = Self {
        name: Text::new(),
        value: Text::new(),
        used: false,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    None,
    Clear,
    Reboot,
    Halt,
    /// `ear <command>` needs the account password re-entered before it can
    /// run (see `Shell::require_reauth`/`execute_as_root`) - the caller is
    /// expected to prompt for it out-of-band and, once it checks out, call
    /// `note_authenticated` and re-submit the exact same command line.
    NeedsPassword,
    /// `passwd` was run - the caller (the real interactive terminal) is
    /// expected to walk the user through current/new/confirm out-of-band,
    /// exactly like `NeedsPassword` above. The shell has no notion of the
    /// account credential at all (that lives entirely in the desktop's own
    /// state), so unlike every other command this one does nothing itself.
    NeedsPasswordChange,
}

/// How long a successful `ear` password check keeps elevation available
/// without asking again - long enough that a short run of admin commands
/// (`ear av import`, then `ear av status`) does not re-prompt for every
/// one, short enough that walking away from an unlocked desktop does not
/// leave elevation open indefinitely. Same order of magnitude as `sudo`'s
/// own default timestamp cache.
const REAUTH_WINDOW_NS: u64 = 5 * 60 * 1_000_000_000;

const MAX_BACKGROUND_JOBS: usize = 8;

#[derive(Clone, Copy)]
struct BackgroundJob {
    job_id: u32,
    task_id: u64,
    command: Text<MAX_PATH>,
}

pub(crate) struct Shell<'a> {
    info: SystemInfo<'a>,
    cwd: Text<MAX_PATH>,
    hostname: Text<64>,
    environment: [EnvironmentEntry; ENVIRONMENT_ITEMS],
    history: [Text<MAX_INPUT>; HISTORY_ITEMS],
    history_count: usize,
    uid: u32,
    effective_uid: u32,
    may_elevate: bool,
    requires_reauth: bool,
    reauth_deadline_ns: u64,
    elevated_commands: u64,
    denied_commands: u64,
    executed_commands: u64,
    last_status: u8,
    expansion_status: u8,
    background_jobs: [Option<BackgroundJob>; MAX_BACKGROUND_JOBS],
    next_job_id: u32,
    /// Output of the previous pipeline stage, read by commands that are given
    /// no file name.
    pipe_input: Option<Text<MAX_OUTPUT>>,
}

impl<'a> Shell<'a> {
    pub(crate) fn new(info: SystemInfo<'a>, may_elevate: bool) -> Self {
        let mut shell = Self {
            info,
            cwd: Text::new(),
            hostname: Text::new(),
            environment: [EnvironmentEntry::EMPTY; ENVIRONMENT_ITEMS],
            history: [Text::new(); HISTORY_ITEMS],
            history_count: 0,
            uid: 1000,
            effective_uid: 1000,
            may_elevate,
            requires_reauth: false,
            reauth_deadline_ns: 0,
            elevated_commands: 0,
            denied_commands: 0,
            executed_commands: 0,
            last_status: 0,
            expansion_status: 0,
            background_jobs: [None; MAX_BACKGROUND_JOBS],
            next_job_id: 1,
            pipe_input: None,
        };
        shell.cwd.push_str_checked("/");
        shell.hostname.push_str_checked("aeros");
        shell.set_environment("HOME", "/");
        shell.set_environment("PATH", "/bin:/system/bin");
        shell.set_environment("SHELL", "/system/bin/aersh");
        shell.set_environment("USER", "aero");
        shell.set_environment("TERM", "aeros-fb");
        shell
    }

    pub(crate) fn is_elevated(&self) -> bool {
        self.effective_uid == 0
    }

    /// Opts this shell into the password-re-entry requirement below -
    /// `false` by default (unchanged from before this existed) so every
    /// other `Shell::new` caller (self-tests, boot-time internal use) keeps
    /// elevating on `may_elevate` alone, exactly as it always has. Only the
    /// real interactive terminal the desktop hands the user calls this.
    pub(crate) fn require_reauth(&mut self) {
        self.requires_reauth = true;
    }

    /// Called once the caller has independently verified the account
    /// password (see `Control::NeedsPassword`): opens the reauth window so
    /// `ear` elevates without asking again until it lapses.
    pub(crate) fn note_authenticated(&mut self) {
        self.reauth_deadline_ns = time::monotonic_nanoseconds().saturating_add(REAUTH_WINDOW_NS);
    }

    /// `back` counts from the most recently entered command (0 = most
    /// recent), matching the natural order a terminal's Up-arrow recall
    /// walks in. Lets a windowed UI browse history without exposing the
    /// underlying ring storage.
    pub(crate) fn history_entry(&self, back: usize) -> Option<&str> {
        let index = self.history_count.checked_sub(1)?.checked_sub(back)?;
        Some(self.history[index].as_str())
    }

    pub(crate) fn diagnostics(&self) -> (u64, u8, u64, u64) {
        (
            self.executed_commands,
            self.last_status,
            self.elevated_commands,
            self.denied_commands,
        )
    }

    pub(crate) fn execute(&mut self, line: &str, output: &mut Text<MAX_OUTPUT>) -> Control {
        output.clear();
        if line.trim().is_empty() {
            self.last_status = 0;
            return Control::None;
        }
        self.executed_commands = self.executed_commands.saturating_add(1);
        self.remember(line);
        if let Some((stages, count)) = split_pipeline(line) {
            return self.execute_pipeline(&stages[..count], output);
        }
        let mut arguments = match Arguments::parse(line) {
            Ok(arguments) => arguments,
            Err(failure) => {
                let _ = writeln!(output, "aersh: {failure}");
                self.last_status = 2;
                return Control::None;
            }
        };
        let redirect = extract_redirection(&mut arguments);
        let control = self.dispatch(&arguments, 0, output);
        if let Some(redirect) = redirect {
            self.apply_redirection(redirect, output);
        }
        if output.truncated {
            let _ = output.push_str_checked("\n[output truncated]\n");
        }
        control
    }

    /// Runs `a | b | c`: each stage's output becomes the next stage's input,
    /// and the last stage's status is the pipeline's. The stages are in-kernel
    /// handlers sharing buffers, not separate processes, so a stage runs to
    /// completion before the next starts and passes at most one output buffer
    /// along.
    fn execute_pipeline(&mut self, stages: &[&str], output: &mut Text<MAX_OUTPUT>) -> Control {
        if stages.len() > MAX_PIPELINE_STAGES || stages.iter().any(|stage| stage.trim().is_empty())
        {
            let _ = writeln!(output, "aersh: invalid pipeline");
            self.last_status = 2;
            return Control::None;
        }
        let mut control = Control::None;
        let mut carried: Option<Text<MAX_OUTPUT>> = None;
        for (index, stage) in stages.iter().enumerate() {
            let mut arguments = match Arguments::parse(stage) {
                Ok(arguments) => arguments,
                Err(failure) => {
                    let _ = writeln!(output, "aersh: {failure}");
                    self.last_status = 2;
                    self.pipe_input = None;
                    return Control::None;
                }
            };
            let redirect = extract_redirection(&mut arguments);
            let mut produced: Text<MAX_OUTPUT> = Text::new();
            self.pipe_input = carried.take();
            control = self.dispatch(&arguments, 0, &mut produced);
            self.pipe_input = None;
            if let Some(redirect) = redirect {
                self.apply_redirection(redirect, &mut produced);
            }
            if index + 1 == stages.len() {
                let _ = output.push_str_checked(produced.as_str());
                if produced.truncated {
                    output.truncated = true;
                }
            } else {
                carried = Some(produced);
            }
        }
        if output.truncated {
            let _ = output.push_str_checked("\n[output truncated]\n");
        }
        control
    }

    /// The text a file-reading command works on: the named file, or the
    /// previous pipeline stage's output when no file is named (or `-`).
    fn read_input(
        &self,
        name: Option<&str>,
        data: &mut [u8; 4096],
    ) -> Result<(usize, Text<MAX_PATH>), InputFailure> {
        match name {
            None | Some("-") => {
                let Some(piped) = &self.pipe_input else {
                    return Err(InputFailure::Missing);
                };
                let bytes = piped.as_str().as_bytes();
                let count = bytes.len().min(data.len());
                data[..count].copy_from_slice(&bytes[..count]);
                Ok((count, Text::new()))
            }
            Some(input) => {
                let mut path = Text::new();
                self.make_path(input, &mut path)
                    .map_err(|_| InputFailure::InvalidPath)?;
                let length = read_file(path.as_str(), data).map_err(InputFailure::Vfs)?;
                Ok((length, path))
            }
        }
    }

    /// A minimal `.bashrc`/`.profile` equivalent: runs `/home/.aershrc`
    /// line by line through the ordinary `execute()` path (so `#`-comments
    /// aside, anything valid at the prompt - `export`, `cd`, aliases-via-
    /// environment, ...) - if present, leaving `output` empty and doing
    /// nothing when it's absent (the common case) or unreadable. Goes
    /// through the normal AV-scanned `vfs::open_file` (not `open_file_raw`):
    /// unlike most internal reads, this file's content runs as commands
    /// automatically, so it gets the same on-open scan a user manually
    /// opening it would.
    pub(crate) fn run_startup_script(&mut self, output: &mut Text<MAX_OUTPUT>) {
        output.clear();
        let Ok(descriptor) = vfs::open_file(STARTUP_SCRIPT_PATH, false, false, false, 0, false)
        else {
            return;
        };
        let mut buffer = [0u8; 4096];
        let mut total = 0usize;
        loop {
            match vfs::read(descriptor, &mut buffer[total..]) {
                Ok(0) => break,
                Ok(count) => {
                    total += count;
                    if total == buffer.len() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = vfs::close(descriptor);
        let Ok(contents) = core::str::from_utf8(&buffer[..total]) else {
            return;
        };
        let mut line_output: Text<MAX_OUTPUT> = Text::new();
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            self.execute(line, &mut line_output);
            let _ = output.push_str_checked(line_output.as_str());
        }
    }

    /// Writes `output` (already populated by `dispatch`) to `redirect`'s
    /// path instead of leaving it for the terminal to display - real shell
    /// `>`/`>>` semantics, so nothing appears on screen once this runs,
    /// success or failure (a failure still replaces `output` with an error,
    /// matching how every other command reports one).
    fn apply_redirection(&mut self, redirect: Redirection, output: &mut Text<MAX_OUTPUT>) {
        let mut path: Text<MAX_PATH> = Text::new();
        let write_result = normalize_path(self.cwd.as_str(), redirect.path.as_str(), &mut path)
            .and_then(|()| {
                vfs::open_file(path.as_str(), true, false, !redirect.append, 0o644, true)
                    .map_err(|_| "cannot open file for writing")
            })
            .and_then(|descriptor| {
                let bytes = output.as_str().as_bytes();
                let wrote = vfs::write(descriptor, bytes, redirect.append) == Ok(bytes.len());
                let _ = vfs::close(descriptor);
                if wrote { Ok(()) } else { Err("write failed") }
            });
        output.clear();
        if let Err(failure) = write_result {
            let _ = writeln!(output, "aersh: {}: {failure}", redirect.path.as_str());
            self.last_status = 1;
        }
    }

    fn dispatch(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) -> Control {
        let Some(name) = arguments.get(offset) else {
            self.last_status = 0;
            return Control::None;
        };
        let Some(specification) = find_command(name) else {
            let _ = writeln!(output, "aersh: {name}: command not found");
            self.last_status = 127;
            return Control::None;
        };
        if specification.handler == Handler::Ear {
            return self.execute_as_root(arguments, offset, output);
        }
        if specification.privileged && self.effective_uid != 0 {
            let _ = writeln!(
                output,
                "aersh: {}: permission denied; use ear",
                specification.name
            );
            self.denied_commands = self.denied_commands.saturating_add(1);
            self.last_status = 126;
            return Control::None;
        }
        let control = self.run_handler(specification, arguments, offset + 1, output);
        if self.last_status == 0 && output.truncated {
            self.last_status = 1;
        }
        control
    }

    fn execute_as_root(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) -> Control {
        if arguments.get(offset + 1).is_none() {
            let _ = writeln!(output, "usage: ear <command> [args...]");
            self.last_status = 2;
            return Control::None;
        }
        if arguments.get(offset + 1) == Some("ear") {
            let _ = writeln!(output, "ear: nested elevation is not allowed");
            self.last_status = 2;
            return Control::None;
        }
        let attempted = arguments.get(offset + 1).unwrap_or("");
        if !self.may_elevate {
            let _ = writeln!(output, "ear: this session has no elevation capability");
            self.denied_commands = self.denied_commands.saturating_add(1);
            self.last_status = 126;
            crate::audit::record(
                "PRIVILEGE",
                format_args!("ear denied reason=no_capability command={attempted}"),
            );
            return Control::None;
        }
        if self.requires_reauth && time::monotonic_nanoseconds() >= self.reauth_deadline_ns {
            let _ = writeln!(output, "ear: password required");
            self.last_status = 1;
            crate::audit::record(
                "PRIVILEGE",
                format_args!("ear denied reason=reauth_required command={attempted}"),
            );
            return Control::NeedsPassword;
        }
        let previous = self.effective_uid;
        self.effective_uid = 0;
        self.elevated_commands = self.elevated_commands.saturating_add(1);
        crate::audit::record("PRIVILEGE", format_args!("ear granted command={attempted}"));
        let control = self.dispatch(arguments, offset + 1, output);
        self.effective_uid = previous;
        control
    }

    fn run_handler(
        &mut self,
        specification: &CommandSpec,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) -> Control {
        self.expansion_status = self.last_status;
        self.last_status = 0;
        match specification.handler {
            Handler::Help => self.command_help(arguments, offset, output),
            Handler::Man => self.command_man(arguments, offset, output),
            Handler::Clear => return Control::Clear,
            Handler::Echo => self.command_echo(arguments, offset, output),
            Handler::Notify => {
                let mut message: Text<64> = Text::new();
                for index in offset..arguments.count {
                    if index != offset {
                        let _ = write!(message, " ");
                    }
                    let _ = write!(message, "{}", arguments.get(index).unwrap_or(""));
                }
                crate::notify::push(0, "Terminal", message.as_str());
            }
            Handler::Pwd => {
                let _ = writeln!(output, "{}", self.cwd.as_str());
            }
            Handler::Cd => self.command_cd(arguments, offset, output),
            Handler::Ls => self.command_ls(arguments, offset, output),
            Handler::Cat => self.command_cat(arguments, offset, output),
            Handler::Head => self.command_head(arguments, offset, output),
            Handler::Tail => self.command_tail(arguments, offset, output),
            Handler::Wc => self.command_wc(arguments, offset, output),
            Handler::Stat => self.command_stat(arguments, offset, output),
            Handler::Touch => self.command_touch(arguments, offset, output),
            Handler::Mkdir => self.command_mkdir(arguments, offset, output),
            Handler::Rm => self.command_rm(arguments, offset, output),
            Handler::Ln => self.command_ln(arguments, offset, output),
            Handler::Readlink => self.command_readlink(arguments, offset, output),
            Handler::Cp => self.command_cp(arguments, offset, output),
            Handler::Mv => self.command_mv(arguments, offset, output),
            Handler::Chmod => self.command_chmod(arguments, offset, output),
            Handler::Av => self.command_av(arguments, offset, output),
            Handler::Uname => self.command_uname(arguments, offset, output),
            Handler::Hostname => self.command_hostname(arguments, offset, output),
            Handler::Whoami => {
                let _ = writeln!(output, "{}", self.user_name());
            }
            Handler::Id => self.command_id(output),
            Handler::Date => self.command_date(output),
            Handler::Uptime => self.command_uptime(output),
            Handler::Sleep => self.command_sleep(arguments, offset, output),
            Handler::Ps => self.command_ps(output),
            Handler::Kill => self.command_kill(arguments, offset, output),
            Handler::Jobs => self.command_jobs(output),
            Handler::Wait => self.command_wait(arguments, offset, output),
            Handler::Fw => self.command_fw(arguments, offset, output),
            Handler::Dmesg => self.command_dmesg(arguments, offset, output),
            Handler::Svc => self.command_svc(arguments, offset, output),
            Handler::Pkg => self.command_pkg(arguments, offset, output),
            Handler::Fsck => self.command_fsck(arguments, offset, output),
            Handler::Install => self.command_install(arguments, offset, output),
            Handler::Acpi => self.command_acpi(arguments, offset, output),
            Handler::Battery => self.command_battery(output),
            Handler::Thermal => self.command_thermal(output),
            Handler::Measure => {
                if arguments.get(offset) == Some("seal") {
                    let sealed = crate::measure::seal(crate::measure::REFERENCE_PATH);
                    let _ = writeln!(
                        output,
                        "{}",
                        if sealed { "sealed" } else { "nothing to seal" }
                    );
                } else {
                    crate::measure::entries(|entry| {
                        let _ = writeln!(
                            output,
                            "{:<10} {}",
                            entry.name,
                            crate::measure::hex_text(&entry.digest).as_str()
                        );
                    });
                    let _ = writeln!(
                        output,
                        "register   {}\nboot image {} the sealed one",
                        crate::measure::hex_text(&crate::measure::register()).as_str(),
                        crate::measure::compare(crate::measure::REFERENCE_PATH).label()
                    );
                }
            }
            Handler::Update => {
                const KEYS: &str = "/home/pkg";
                let outcome = match (arguments.get(offset), arguments.get(offset + 1)) {
                    (Some("rollback"), None) => {
                        Some(crate::update::rollback_boot_volume().map(|_| {
                            let mut text = crate::pkg::Buf::<160>::new();
                            let _ = write!(text, "rolled back; restart to use the previous image");
                            text
                        }))
                    }
                    (Some(image), Some(signature)) => Some(
                        crate::update::apply_to_boot_volume(KEYS, image, signature).map(|done| {
                            let mut text = crate::pkg::Buf::<160>::new();
                            let _ = write!(
                                text,
                                "installed {} bytes (sha256 {}){}; restart to use the new image",
                                done.bytes,
                                crate::measure::hex_text(&done.digest).as_str(),
                                if done.kept_previous {
                                    ", previous image kept"
                                } else {
                                    ""
                                }
                            );
                            text
                        }),
                    ),
                    _ => None,
                };
                match outcome {
                    None => self.usage_named(output, "update"),
                    Some(Ok(message)) => {
                        let _ = writeln!(output, "{}", message.as_str());
                    }
                    Some(Err(failure)) => self.fail(output, "update", failure.message()),
                }
            }
            Handler::Swap => {
                let stats = crate::swap::stats();
                if stats.slots == 0 {
                    let _ = writeln!(
                        output,
                        "no swap area (a partition formatted as swap is used)"
                    );
                } else {
                    let _ = writeln!(
                        output,
                        "{} KiB swap, {} KiB used; {} pages written, {} read back, {} failures",
                        stats.slots * 4,
                        stats.in_use * 4,
                        stats.written,
                        stats.read,
                        stats.failures
                    );
                }
            }
            Handler::Strace => self.command_strace(arguments, offset, output),
            Handler::Sigcheck => self.command_sigcheck(arguments, offset, output),
            Handler::Bench => match arguments.get(offset) {
                Some("smp") => crate::bench::run_smp(output),
                _ => crate::bench::run(output),
            },
            Handler::Crashes => self.command_crashes(arguments, offset, output),
            Handler::Renice => self.command_renice(arguments, offset, output),
            Handler::Grep => self.command_grep(arguments, offset, output),
            Handler::Uniq => self.command_uniq(arguments, offset, output),
            Handler::Tee => self.command_tee(arguments, offset, output),
            Handler::Find => self.command_find(arguments, offset, output),
            Handler::Du => self.command_du(arguments, offset, output),
            Handler::Sort => self.command_sort(arguments, offset, output),
            Handler::Basename => self.command_basename(arguments, offset, output, false),
            Handler::Dirname => self.command_basename(arguments, offset, output, true),
            Handler::Seq => self.command_seq(arguments, offset, output),
            Handler::True => {}
            Handler::False => self.last_status = 1,
            Handler::Free => self.command_free(output),
            Handler::Lscpu => self.command_lscpu(output),
            Handler::Lspci => self.command_lspci(output),
            Handler::Lsblk => self.command_lsblk(output),
            Handler::Mount => self.command_mount(arguments, offset, output),
            Handler::Umount => self.command_umount(arguments, offset, output),
            Handler::Df => self.command_df(output),
            Handler::Sync => self.command_sync(output),
            Handler::Which => self.command_which(arguments, offset, output),
            Handler::Env => self.command_env(output),
            Handler::Export => self.command_export(arguments, offset, output),
            Handler::Unset => self.command_unset(arguments, offset, output),
            Handler::History => self.command_history(output),
            Handler::Ip => self.command_ip(output),
            Handler::Ping => self.command_ping(arguments, offset, output),
            Handler::Dns => self.command_dns(arguments, offset, output),
            Handler::Https => self.command_https(arguments, offset, output),
            Handler::Aerfs => self.command_aerfs(arguments, offset, output),
            Handler::Route => self.command_route(output),
            Handler::Arp => self.command_arp(output),
            Handler::Netstat => self.command_netstat(output),
            Handler::Reboot => {
                let _ = writeln!(output, "Rebooting AerOS");
                return Control::Reboot;
            }
            Handler::Shutdown => {
                let _ = writeln!(output, "AerOS is safe to power off");
                return Control::Halt;
            }
            Handler::Ear => {}
            Handler::Run => self.command_run(arguments, offset, output),
            Handler::Passwd => return Control::NeedsPasswordChange,
        }
        Control::None
    }

    fn remember(&mut self, line: &str) {
        if self.history_count == HISTORY_ITEMS {
            self.history.copy_within(1.., 0);
            self.history_count -= 1;
        }
        let item = &mut self.history[self.history_count];
        item.clear();
        item.push_str_checked(line);
        self.history_count += 1;
    }

    fn user_name(&self) -> &'static str {
        if self.effective_uid == 0 {
            "root"
        } else {
            "aero"
        }
    }

    fn make_path(&self, input: &str, output: &mut Text<MAX_PATH>) -> Result<(), &'static str> {
        normalize_path(self.cwd.as_str(), input, output)
    }

    fn fail(&mut self, output: &mut Text<MAX_OUTPUT>, command: &str, failure: &str) {
        let _ = writeln!(output, "{command}: {failure}");
        self.last_status = 1;
    }

    fn usage(&mut self, output: &mut Text<MAX_OUTPUT>, specification: &CommandSpec) {
        let _ = writeln!(output, "usage: {}", specification.usage);
        self.last_status = 2;
    }

    fn set_environment(&mut self, name: &str, value: &str) -> bool {
        if !valid_environment_name(name) || name.len() > 24 || value.len() > 96 {
            return false;
        }
        let slot = self
            .environment
            .iter()
            .position(|entry| entry.used && entry.name.as_str() == name)
            .or_else(|| self.environment.iter().position(|entry| !entry.used));
        let Some(slot) = slot else {
            return false;
        };
        let entry = &mut self.environment[slot];
        entry.name.clear();
        entry.value.clear();
        entry.name.push_str_checked(name);
        entry.value.push_str_checked(value);
        entry.used = true;
        true
    }
}

/// What trying to run a program image (from anywhere) turned into.
enum RunOutcome {
    Blocked(crate::antivirus::Detection),
    NotExecutable,
    Ran(u64),
}

enum LaunchFailure {
    InvalidPath,
    PagingNotReady,
    Vfs(crate::vfs::VfsError),
}

/// The scan-then-spawn logic shared by every source `run` can load a program
/// from (the embedded `/bin/*` binaries, or a `/home`/`/media` file read
/// into `vfs::with_home_file`'s scratch buffer) - `data` only needs to
/// outlive this one call, since `spawn_process_with` copies it into the new
/// process's own pages before returning.
fn run_scanned(
    data: &[u8],
    state: &crate::arch::paging::PagingState,
    path: &str,
    argv: &[&[u8]],
) -> RunOutcome {
    if let Some(found) = crate::antivirus::scan_bytes(data)
        && found.class != crate::antivirus::Class::Info
    {
        return RunOutcome::Blocked(found);
    }
    match crate::scheduler::spawn_process_with(state, data, path, argv) {
        Some((id, _slot, _space)) => RunOutcome::Ran(id),
        None => RunOutcome::NotExecutable,
    }
}

impl Shell<'_> {
    fn command_help(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        if let Some(name) = arguments.get(offset) {
            self.show_manual(name, output);
            return;
        }
        let _ = writeln!(output, "AerOS command shell · {} commands", COMMANDS.len());
        for row in 0..COMMANDS.len().div_ceil(5) {
            for column in 0..5 {
                let index = row + column * COMMANDS.len().div_ceil(5);
                if let Some(specification) = COMMANDS.get(index) {
                    let _ = write!(output, "{:<12}", specification.name);
                }
            }
            let _ = writeln!(output);
        }
        let _ = writeln!(
            output,
            "Use man <command>. Use ear <command> for privileged operations."
        );
    }

    fn command_man(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let Some(name) = arguments.get(offset) else {
            self.usage_named(output, "man");
            return;
        };
        self.show_manual(name, output);
    }

    fn show_manual(&mut self, name: &str, output: &mut Text<MAX_OUTPUT>) {
        let Some(specification) = find_command(name) else {
            self.fail(output, "man", "no manual entry");
            return;
        };
        let _ = writeln!(
            output,
            "NAME\n  {} - {}",
            specification.name, specification.summary
        );
        let _ = writeln!(output, "USAGE\n  {}", specification.usage);
        if specification.privileged {
            let _ = writeln!(
                output,
                "PRIVILEGE\n  Run through ear from the local console."
            );
        }
    }

    fn command_echo(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        for index in offset..arguments.count {
            if index != offset {
                let _ = write!(output, " ");
            }
            let value = arguments.get(index).unwrap_or("");
            if let Some(name) = value.strip_prefix('$') {
                if name == "?" {
                    let _ = write!(output, "{}", self.expansion_status);
                } else if let Some(entry) = self
                    .environment
                    .iter()
                    .find(|entry| entry.used && entry.name.as_str() == name)
                {
                    let _ = write!(output, "{}", entry.value.as_str());
                }
            } else {
                let _ = write!(output, "{value}");
            }
        }
        let _ = writeln!(output);
    }

    fn command_cd(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let destination = arguments.get(offset).unwrap_or("/");
        let mut path = Text::new();
        if let Err(failure) = self.make_path(destination, &mut path) {
            self.fail(output, "cd", failure);
            return;
        }
        match vfs::metadata(path.as_str()) {
            Ok(metadata) if metadata.mode & 0o170000 == 0o040000 => self.cwd = path,
            Ok(_) => self.fail(output, "cd", "not a directory"),
            Err(failure) => self.fail_vfs(output, "cd", failure),
        }
    }

    fn command_ls(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let long = arguments.get(offset) == Some("-l");
        let path_argument = arguments
            .get(offset + usize::from(long))
            .unwrap_or(self.cwd.as_str());
        let mut path = Text::new();
        if let Err(failure) = self.make_path(path_argument, &mut path) {
            self.fail(output, "ls", failure);
            return;
        }
        let descriptor = match vfs::open_directory(path.as_str()) {
            Ok(descriptor) => descriptor,
            Err(vfs::VfsError::NotDirectory) => {
                self.list_single(path.as_str(), output, long);
                return;
            }
            Err(failure) => {
                self.fail_vfs(output, "ls", failure);
                return;
            }
        };
        loop {
            match vfs::next_directory_entry(descriptor) {
                Ok(Some(entry)) => {
                    let name =
                        core::str::from_utf8(&entry.name[..entry.name_len as usize]).unwrap_or("?");
                    let mut child: Text<MAX_PATH> = Text::new();
                    let _ = child.push_str_checked(path.as_str());
                    if child.as_str() != "/" {
                        child.push_byte(b'/');
                    }
                    child.push_str_checked(name);
                    let type_char = match entry.kind {
                        4 => 'd',
                        10 => 'l',
                        _ => '-',
                    };
                    if long {
                        if let Ok(metadata) = vfs::metadata(child.as_str()) {
                            let _ = writeln!(
                                output,
                                "{} {:03o} {:>6} {}",
                                type_char,
                                metadata.mode & 0o777,
                                metadata.size,
                                name
                            );
                        }
                    } else if entry.kind == 10 {
                        let mut target = [0u8; vfs::MAX_NAME];
                        let target_str = vfs::readlink(child.as_str(), &mut target)
                            .ok()
                            .and_then(|length| core::str::from_utf8(&target[..length]).ok())
                            .unwrap_or("?");
                        let _ = writeln!(output, "{name} -> {target_str}");
                    } else {
                        let _ =
                            writeln!(output, "{}{}", name, if entry.kind == 4 { "/" } else { "" });
                    }
                }
                Ok(None) => break,
                Err(failure) => {
                    self.fail_vfs(output, "ls", failure);
                    break;
                }
            }
        }
        let _ = vfs::close(descriptor);
    }

    fn list_single(&mut self, path: &str, output: &mut Text<MAX_OUTPUT>, long: bool) {
        match vfs::metadata(path) {
            Ok(metadata) if long => {
                let _ = writeln!(
                    output,
                    "- {:03o} {:>6} {}",
                    metadata.mode & 0o777,
                    metadata.size,
                    path
                );
            }
            Ok(_) => {
                let _ = writeln!(output, "{path}");
            }
            Err(failure) => self.fail_vfs(output, "ls", failure),
        }
    }

    fn command_cat(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let mut data = [0u8; 4096];
        match self.read_input(arguments.get(offset), &mut data) {
            Ok((length, _)) if textual(&data[..length]) => {
                write_file_bytes(output, &data[..length]);
                if length != 0 && data[length - 1] != b'\n' {
                    let _ = writeln!(output);
                }
            }
            Ok(_) => self.fail(output, "cat", "refusing to print binary data"),
            Err(failure) => self.input_failed(output, "cat", failure),
        }
    }

    fn input_failed(
        &mut self,
        output: &mut Text<MAX_OUTPUT>,
        command: &str,
        failure: InputFailure,
    ) {
        match failure {
            InputFailure::Missing => self.usage_named(output, command),
            InputFailure::InvalidPath => self.fail(output, command, "invalid path"),
            InputFailure::Vfs(failure) => self.fail_vfs(output, command, failure),
        }
    }

    fn command_head(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let (count, file_index) = parse_line_count(arguments, offset).unwrap_or((10, offset));
        let mut data = [0u8; 4096];
        match self.read_input(arguments.get(file_index), &mut data) {
            Ok((length, _)) if textual(&data[..length]) => {
                let end = prefix_lines(&data[..length], count);
                write_file_bytes(output, &data[..end]);
            }
            Ok(_) => self.fail(output, "head", "binary file"),
            Err(failure) => self.input_failed(output, "head", failure),
        }
    }

    fn command_tail(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let (count, file_index) = parse_line_count(arguments, offset).unwrap_or((10, offset));
        let mut data = [0u8; 4096];
        match self.read_input(arguments.get(file_index), &mut data) {
            Ok((length, _)) if textual(&data[..length]) => {
                let start = suffix_lines(&data[..length], count);
                write_file_bytes(output, &data[start..length]);
            }
            Ok(_) => self.fail(output, "tail", "binary file"),
            Err(failure) => self.input_failed(output, "tail", failure),
        }
    }

    fn command_wc(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let mut data = [0u8; 4096];
        match self.read_input(arguments.get(offset), &mut data) {
            Ok((length, path)) => {
                let lines = data[..length].iter().filter(|byte| **byte == b'\n').count();
                let mut words = 0;
                let mut inside = false;
                for byte in &data[..length] {
                    if byte.is_ascii_whitespace() {
                        inside = false;
                    } else if !inside {
                        words += 1;
                        inside = true;
                    }
                }
                let _ = writeln!(
                    output,
                    "{lines:>6} {words:>6} {length:>6} {}",
                    path.as_str()
                );
            }
            Err(failure) => self.input_failed(output, "wc", failure),
        }
    }

    fn command_stat(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(input) = arguments.get(offset) else {
            self.usage_named(output, "stat");
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "stat", "invalid path");
            return;
        }
        match vfs::metadata(path.as_str()) {
            Ok(metadata) => {
                let kind = if metadata.mode & 0o170000 == 0o040000 {
                    "directory"
                } else {
                    "file"
                };
                let _ = writeln!(output, "Path: {}", path.as_str());
                let _ = writeln!(
                    output,
                    "Type: {kind}  Inode: {}  Size: {}",
                    metadata.inode, metadata.size
                );
                let _ = writeln!(output, "Mode: {:04o}", metadata.mode & 0o7777);
            }
            Err(failure) => self.fail_vfs(output, "stat", failure),
        }
    }

    fn command_touch(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(input) = arguments.get(offset) else {
            self.usage_named(output, "touch");
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "touch", "invalid path");
            return;
        }
        match vfs::open_file(path.as_str(), true, false, false, 0o666, true) {
            Ok(descriptor) => {
                if let Err(failure) = vfs::close(descriptor) {
                    self.fail_vfs(output, "touch", failure);
                }
            }
            Err(failure) => self.fail_vfs(output, "touch", failure),
        }
    }

    /// Loads `path` as a real ELF binary and runs it as an actual scheduled
    /// process (its own address space, fork/exec-capable, real Linux-ABI
    /// syscalls) rather than the boot-time single-shot self-test runner.
    /// Works for the embedded `/bin/*` binaries and for anything under
    /// `/home`/`/media` (read through `vfs::with_home_file`, since those
    /// have no `'static` in-memory copy to hand out a zero-copy view into).
    fn command_run(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        if arguments.get(offset).is_none() {
            self.usage_named(output, "run");
            return;
        }
        // A trailing `&` backgrounds the process instead of blocking on it -
        // consumed here, before path/argv parsing, so it never reaches the
        // launched program as a real argument.
        let background =
            arguments.count > offset + 1 && arguments.get(arguments.count - 1) == Some("&");
        let end = if background {
            arguments.count - 1
        } else {
            arguments.count
        };
        let (path, outcome) = match self.launch_program(arguments, offset, end) {
            Ok(launched) => launched,
            Err(LaunchFailure::InvalidPath) => {
                self.fail(output, "run", "invalid path");
                return;
            }
            Err(LaunchFailure::PagingNotReady) => {
                self.fail(output, "run", "paging not ready");
                return;
            }
            Err(LaunchFailure::Vfs(failure)) => {
                self.fail_vfs(output, "run", failure);
                return;
            }
        };
        match outcome {
            RunOutcome::Blocked(found) => {
                let _ = writeln!(
                    output,
                    "run: blocked by AerOS Shield: {} ({})",
                    found.name,
                    found.class.label()
                );
                self.last_status = 126;
            }
            RunOutcome::NotExecutable => self.fail(output, "run", "not a valid executable"),
            RunOutcome::Ran(id) if background => {
                self.add_background_job(id, path.as_str(), output);
            }
            RunOutcome::Ran(id) => match crate::scheduler::wait_for_child(id) {
                Some(exit_code) => {
                    let _ = writeln!(output, "[{} exited with {}]", path.as_str(), exit_code);
                }
                None => self.fail(output, "run", "process did not exit"),
            },
        }
    }

    /// Resolves `arguments[offset]` as a program path, scans it and spawns it
    /// with `arguments[offset + 1..end]` as its argv.
    fn launch_program(
        &self,
        arguments: &Arguments,
        offset: usize,
        end: usize,
    ) -> Result<(Text<MAX_PATH>, RunOutcome), LaunchFailure> {
        let input = arguments.get(offset).ok_or(LaunchFailure::InvalidPath)?;
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            return Err(LaunchFailure::InvalidPath);
        }
        let Some(state) = crate::arch::paging::boot_state() else {
            return Err(LaunchFailure::PagingNotReady);
        };
        let mut argv: [&[u8]; 16] = [&[]; 16];
        argv[0] = path.as_str().as_bytes();
        let mut argc = 1;
        while argc < argv.len() && offset + argc < end {
            let Some(value) = arguments.get(offset + argc) else {
                break;
            };
            argv[argc] = value.as_bytes();
            argc += 1;
        }
        // `/home` and `/media` files have no zero-copy `'static` view, so they
        // are read into a scratch buffer and spawned from inside the closure.
        let outcome = if crate::datafs::route(path.as_str()).is_some() {
            vfs::with_home_file(path.as_str(), |data, _mode| {
                run_scanned(data, &state, path.as_str(), &argv[..argc])
            })
        } else {
            vfs::file(path.as_str())
                .map(|file| run_scanned(file.data, &state, path.as_str(), &argv[..argc]))
        }
        .map_err(LaunchFailure::Vfs)?;
        Ok((path, outcome))
    }

    /// Records a `run ... &` launch as a background job (`[N] task_id`,
    /// matching real shell output) instead of blocking on it. If every slot
    /// is already taken, evicts the oldest ALREADY-FINISHED job first (never
    /// a still-running one) to make room; if none are finished either, the
    /// new job simply isn't tracked (the process itself still runs and can
    /// still be waited on with `wait <task_id>` - it's only `jobs`'s own
    /// bookkeeping that's full).
    fn add_background_job(&mut self, task_id: u64, path: &str, output: &mut Text<MAX_OUTPUT>) {
        let job_id = self.next_job_id;
        self.next_job_id = self.next_job_id.wrapping_add(1).max(1);
        if self.background_jobs.iter().all(Option::is_some)
            && let Some(finished) = self
                .background_jobs
                .iter()
                .position(|job| job.is_some_and(|job| !job_is_running(job.task_id)))
        {
            self.background_jobs[finished] = None;
        }
        let Some(slot) = self.background_jobs.iter().position(Option::is_none) else {
            let _ = writeln!(output, "[{job_id}] {task_id}");
            return;
        };
        let mut command = Text::new();
        let _ = command.push_str_checked(path);
        self.background_jobs[slot] = Some(BackgroundJob {
            job_id,
            task_id,
            command,
        });
        let _ = writeln!(output, "[{job_id}] {task_id}");
    }

    fn command_av(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        use crate::antivirus as av;
        match arguments.get(offset) {
            None | Some("status") => {
                let status = av::status();
                let _ = writeln!(
                    output,
                    "AerOS Shield  realtime protection: {}",
                    if av::realtime_enabled() { "on" } else { "OFF" }
                );
                let _ = writeln!(
                    output,
                    "signatures:   {} ({} custom)",
                    status.signatures, status.dynamic_signatures
                );
                let _ = writeln!(output, "files scanned {}", status.files_scanned);
                let _ = writeln!(output, "threats found {}", status.threats_found);
                let _ = writeln!(
                    output,
                    "programs checked {} (blocked {})",
                    status.execs_checked, status.execs_blocked
                );
                let _ = writeln!(output, "in quarantine {}", status.quarantined);
            }
            Some("scan") => {
                let mut index = offset + 1;
                let clean = arguments.get(index) == Some("--clean");
                if clean {
                    index += 1;
                }
                let mut path = Text::new();
                if self
                    .make_path(arguments.get(index).unwrap_or("/"), &mut path)
                    .is_err()
                {
                    self.fail(output, "av", "invalid path");
                    return;
                }
                let mut report = av::Report::new();
                av::scan_path(path.as_str(), clean, &mut report);
                for finding in report.findings.iter().flatten() {
                    let _ = write!(
                        output,
                        "[{}] {}  {}",
                        finding.detection.class.label(),
                        finding.detection.name,
                        finding.path.as_str()
                    );
                    match finding.quarantined {
                        Some(id) => {
                            let _ = writeln!(output, "  -> #{id}");
                        }
                        None => {
                            let _ = writeln!(output);
                        }
                    }
                }
                let _ = writeln!(
                    output,
                    "scanned {} files: {} threats, {} notes, {} unreadable",
                    report.files, report.threats, report.infos, report.errors
                );
                if report.threats > 0 && !clean {
                    let _ = writeln!(
                        output,
                        "run \"av scan --clean {}\" to quarantine them",
                        path.as_str()
                    );
                }
                if report.threats > 0 {
                    self.last_status = 1;
                }
            }
            Some("hash") => {
                let Some(input) = arguments.get(offset + 1) else {
                    self.usage_named(output, "av");
                    return;
                };
                let mut path = Text::new();
                if self.make_path(input, &mut path).is_err() {
                    self.fail(output, "av", "invalid path");
                    return;
                }
                match av::scan_file(path.as_str()) {
                    Ok((found, digest)) => {
                        for byte in digest {
                            let _ = write!(output, "{byte:02x}");
                        }
                        let _ = writeln!(output, "  {}", path.as_str());
                        if let Some(found) = found {
                            let _ = writeln!(
                                output,
                                "verdict: {} ({})",
                                found.name,
                                found.class.label()
                            );
                        } else {
                            let _ = writeln!(output, "verdict: clean");
                        }
                    }
                    Err(failure) => self.fail_vfs(output, "av", failure),
                }
            }
            Some("quarantine") => {
                let mut any = false;
                av::quarantine_list(|entry| {
                    any = true;
                    let _ = writeln!(
                        output,
                        "#{} {}  from {}",
                        entry.id,
                        entry.name,
                        entry.original.as_str()
                    );
                });
                if !any {
                    let _ = writeln!(output, "quarantine is empty");
                }
            }
            Some(action @ ("restore" | "delete")) => {
                if self.effective_uid != 0 {
                    self.fail(output, "av", "needs elevation: use ear av ...");
                    return;
                }
                let Some(id) = arguments
                    .get(offset + 1)
                    .and_then(|value| value.parse::<u32>().ok())
                else {
                    self.usage_named(output, "av");
                    return;
                };
                let result = if action == "restore" {
                    av::restore(id)
                } else {
                    av::delete(id)
                };
                match result {
                    Ok(()) => {
                        let _ = writeln!(output, "{action}d #{id}");
                    }
                    Err(reason) => self.fail(output, "av", reason),
                }
            }
            Some("realtime") => match arguments.get(offset + 1) {
                Some(choice @ ("on" | "off")) => {
                    if self.effective_uid != 0 {
                        self.fail(output, "av", "needs elevation: use ear av realtime ...");
                        return;
                    }
                    av::set_realtime(choice == "on");
                    let _ = writeln!(output, "realtime protection {choice}");
                }
                None => {
                    let _ = writeln!(
                        output,
                        "realtime protection is {}",
                        if av::realtime_enabled() { "on" } else { "off" }
                    );
                }
                Some(_) => self.usage_named(output, "av"),
            },
            Some("log") => {
                let mut any = false;
                av::events(|event| {
                    any = true;
                    let _ = writeln!(
                        output,
                        "#{} {}: {}  {}",
                        event.seq,
                        event.kind,
                        event.name,
                        event.path.as_str()
                    );
                });
                if !any {
                    let _ = writeln!(output, "no events yet");
                }
            }
            Some("import") => {
                if self.effective_uid != 0 {
                    self.fail(output, "av", "needs elevation: use ear av import ...");
                    return;
                }
                let input = arguments
                    .get(offset + 1)
                    .unwrap_or(av::DEFAULT_SIGNATURE_PATH);
                let mut path = Text::new();
                if self.make_path(input, &mut path).is_err() {
                    self.fail(output, "av", "invalid path");
                    return;
                }
                match av::import_signatures(path.as_str()) {
                    Ok(report) => {
                        let _ = writeln!(
                            output,
                            "loaded {} rule(s) from {} ({} skipped)",
                            report.added,
                            path.as_str(),
                            report.skipped
                        );
                    }
                    Err(failure) => self.fail_vfs(output, "av", failure),
                }
            }
            Some("test") => {
                // Drops the harmless industry-standard antivirus test file.
                let test = av::eicar();
                match vfs::open_file("/tmp/eicar.com.txt", true, false, true, 0o644, true) {
                    Ok(descriptor) => {
                        let _ = vfs::write(descriptor, &test, false);
                        let _ = vfs::close(descriptor);
                        let _ =
                            writeln!(output, "wrote /tmp/eicar.com.txt (the standard test file)");
                        if av::realtime_enabled() && vfs::metadata("/tmp/eicar.com.txt").is_err() {
                            let _ =
                                writeln!(output, "realtime protection quarantined it: see av log");
                        } else {
                            let _ = writeln!(output, "now run: av scan /tmp");
                        }
                    }
                    Err(failure) => self.fail_vfs(output, "av", failure),
                }
            }
            Some(_) => self.usage_named(output, "av"),
        }
    }

    fn command_mkdir(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(input) = arguments.get(offset) else {
            self.usage_named(output, "mkdir");
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "mkdir", "invalid path");
            return;
        }
        if let Err(failure) = vfs::create_directory(path.as_str(), 0o777) {
            self.fail_vfs(output, "mkdir", failure);
        }
    }

    fn command_rm(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let directory = arguments.get(offset) == Some("-d");
        let Some(input) = arguments.get(offset + usize::from(directory)) else {
            self.usage_named(output, "rm");
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "rm", "invalid path");
            return;
        }
        if let Err(failure) = vfs::remove(path.as_str(), directory) {
            self.fail_vfs(output, "rm", failure);
        }
    }

    /// Only `-s` (symbolic) is supported - there is no hard-link equivalent
    /// in this VFS. `target` is stored exactly as typed, unresolved and
    /// un-path-joined against the current directory (matching real `ln -s`:
    /// a relative target is meant to be interpreted relative to `link`'s own
    /// directory at lookup time, not to the shell's cwd at creation time).
    fn command_ln(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        if arguments.get(offset) != Some("-s") {
            self.usage_named(output, "ln");
            return;
        }
        let (Some(target), Some(link)) = (arguments.get(offset + 1), arguments.get(offset + 2))
        else {
            self.usage_named(output, "ln");
            return;
        };
        let mut link_path = Text::new();
        if self.make_path(link, &mut link_path).is_err() {
            self.fail(output, "ln", "invalid path");
            return;
        }
        if let Err(failure) = vfs::symlink(link_path.as_str(), target) {
            self.fail_vfs(output, "ln", failure);
        }
    }

    fn command_readlink(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(input) = arguments.get(offset) else {
            self.usage_named(output, "readlink");
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "readlink", "invalid path");
            return;
        }
        let mut target = [0u8; vfs::MAX_NAME];
        match vfs::readlink(path.as_str(), &mut target) {
            Ok(length) => {
                if let Ok(text) = core::str::from_utf8(&target[..length]) {
                    let _ = writeln!(output, "{text}");
                }
            }
            Err(failure) => self.fail_vfs(output, "readlink", failure),
        }
    }

    fn command_cp(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let (Some(source), Some(destination)) = (arguments.get(offset), arguments.get(offset + 1))
        else {
            self.usage_named(output, "cp");
            return;
        };
        let mut source_path = Text::new();
        let mut destination_path = Text::new();
        if self.make_path(source, &mut source_path).is_err()
            || self.make_path(destination, &mut destination_path).is_err()
        {
            self.fail(output, "cp", "invalid path");
            return;
        }
        let mut data = [0u8; 4096];
        let length = match read_file(source_path.as_str(), &mut data) {
            Ok(length) => length,
            Err(failure) => {
                self.fail_vfs(output, "cp", failure);
                return;
            }
        };
        let descriptor =
            match vfs::open_file(destination_path.as_str(), true, false, true, 0o666, true) {
                Ok(descriptor) => descriptor,
                Err(failure) => {
                    self.fail_vfs(output, "cp", failure);
                    return;
                }
            };
        let result = vfs::write(descriptor, &data[..length], false);
        let close = vfs::close(descriptor);
        if let Err(failure) = result.and(close.map(|_| length)) {
            self.fail_vfs(output, "cp", failure);
        }
    }

    fn command_mv(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let (Some(source), Some(destination)) = (arguments.get(offset), arguments.get(offset + 1))
        else {
            self.usage_named(output, "mv");
            return;
        };
        let mut source_path = Text::new();
        let mut destination_path = Text::new();
        if self.make_path(source, &mut source_path).is_err()
            || self.make_path(destination, &mut destination_path).is_err()
        {
            self.fail(output, "mv", "invalid path");
            return;
        }
        if let Err(failure) = vfs::rename(source_path.as_str(), destination_path.as_str()) {
            self.fail_vfs(output, "mv", failure);
        }
    }

    fn command_chmod(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let (Some(mode), Some(input)) = (arguments.get(offset), arguments.get(offset + 1)) else {
            self.usage_named(output, "chmod");
            return;
        };
        let Some(mode) = parse_octal(mode).filter(|mode| *mode <= 0o777) else {
            self.fail(
                output,
                "chmod",
                "mode must be an octal value from 000 to 777",
            );
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "chmod", "invalid path");
            return;
        }
        if let Err(failure) = vfs::chmod(path.as_str(), mode as u16) {
            self.fail_vfs(output, "chmod", failure);
        }
    }

    fn fail_vfs(&mut self, output: &mut Text<MAX_OUTPUT>, command: &str, failure: vfs::VfsError) {
        self.fail(output, command, vfs_error(failure));
    }

    fn usage_named(&mut self, output: &mut Text<MAX_OUTPUT>, name: &str) {
        if let Some(specification) = find_command(name) {
            self.usage(output, specification);
        }
    }
}

impl Shell<'_> {
    fn command_uname(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        if arguments.get(offset).is_some_and(|value| value != "-a") {
            self.usage_named(output, "uname");
            return;
        }
        if arguments.get(offset) == Some("-a") {
            let _ = writeln!(
                output,
                "AerOS {} {} x86_64 independent",
                self.hostname.as_str(),
                self.info.version
            );
        } else {
            let _ = writeln!(output, "AerOS");
        }
    }

    fn command_hostname(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(name) = arguments.get(offset) else {
            let _ = writeln!(output, "{}", self.hostname.as_str());
            return;
        };
        if self.effective_uid != 0 {
            self.fail(
                output,
                "hostname",
                "permission denied; use ear hostname <name>",
            );
            self.denied_commands = self.denied_commands.saturating_add(1);
            return;
        }
        if name.is_empty()
            || name.len() > 63
            || name.starts_with('-')
            || name.ends_with('-')
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            self.fail(output, "hostname", "invalid host name");
            return;
        }
        self.hostname.clear();
        self.hostname.push_str_checked(name);
    }

    fn command_id(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let _ = writeln!(
            output,
            "uid={}({}) euid={}({}) capability.elevate={}",
            self.uid,
            if self.uid == 0 { "root" } else { "aero" },
            self.effective_uid,
            self.user_name(),
            self.may_elevate
        );
    }

    fn command_date(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let report = rtc::initialize();
        if !report.verified {
            self.fail(output, "date", "RTC is unavailable");
            return;
        }
        let _ = writeln!(
            output,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            report.year, report.month, report.day, report.hour, report.minute, report.second
        );
    }

    fn command_uptime(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let milliseconds = time::monotonic_nanoseconds() / 1_000_000;
        let seconds = milliseconds / 1000;
        let _ = writeln!(
            output,
            "up {}d {:02}:{:02}:{:02}.{:03}",
            seconds / 86_400,
            seconds / 3600 % 24,
            seconds / 60 % 60,
            seconds % 60,
            milliseconds % 1000
        );
    }

    fn command_sleep(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(milliseconds) = arguments.get(offset).and_then(parse_u64) else {
            self.usage_named(output, "sleep");
            return;
        };
        if milliseconds > 60_000 {
            self.fail(output, "sleep", "interval exceeds 60000 milliseconds");
            return;
        }
        let duration = milliseconds.saturating_mul(1_000_000);
        let start = time::monotonic_nanoseconds();
        while time::monotonic_nanoseconds().saturating_sub(start) < duration {
            unsafe {
                asm!("pause", options(nomem, nostack, preserves_flags));
            }
        }
    }

    fn command_ps(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let _ = writeln!(output, "PID  PPID PGID SID  STATE   NI  PAGES");
        let _ = writeln!(output, "0    0    0    0    running 0   0");
        scheduler::list_tasks(|task| {
            let _ = writeln!(
                output,
                "{:<4} {:<4} {:<4} {:<4} {:<7} {:<3} {}",
                task.id,
                task.parent_id,
                task.pgid,
                task.sid,
                if task.running { "running" } else { "ready" },
                task.nice,
                task.resident_pages
            );
        });
        let stats = process::stats();
        let _ = writeln!(
            output,
            "spawned={} reaped={} ready={} running={} zombies={}",
            stats.spawned, stats.reaped, stats.ready, stats.running, stats.zombies
        );
    }

    fn command_kill(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(pid) = arguments.get(offset).and_then(parse_u64) else {
            self.usage_named(output, "kill");
            return;
        };
        if pid == 0 {
            self.fail(output, "kill", "the kernel task cannot be terminated");
        } else if !process::request_exit(pid, 128 + 15) {
            self.fail(output, "kill", "no live process with that PID");
        }
    }

    /// Lists `run ... &` background jobs and their live/finished state. A
    /// finished job's real exit status is fetched (via `wait_for_child`,
    /// which reaps it) the first time it's observed as no longer running,
    /// then dropped from the table - a later `jobs` call won't show it
    /// again, matching real shells discarding a job once its exit is
    /// reported once.
    fn command_jobs(&mut self, output: &mut Text<MAX_OUTPUT>) {
        for slot in 0..self.background_jobs.len() {
            let Some(job) = self.background_jobs[slot] else {
                continue;
            };
            if job_is_running(job.task_id) {
                let _ = writeln!(
                    output,
                    "[{}]  Running   {}",
                    job.job_id,
                    job.command.as_str()
                );
            } else {
                let exit_code = scheduler::wait_for_child(job.task_id).unwrap_or(0);
                let _ = writeln!(
                    output,
                    "[{}]  Done({})  {}",
                    job.job_id,
                    exit_code,
                    job.command.as_str()
                );
                self.background_jobs[slot] = None;
            }
        }
    }

    /// Blocks until `task_id` exits (works for any task, not just a `run
    /// ... &` background job) and prints its real exit status - the
    /// explicit, synchronous counterpart to `jobs`'s passive polling.
    fn command_wait(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(task_id) = arguments.get(offset).and_then(parse_u64) else {
            self.usage_named(output, "wait");
            return;
        };
        if let Some(slot) = self
            .background_jobs
            .iter()
            .position(|job| job.is_some_and(|job| job.task_id == task_id))
        {
            self.background_jobs[slot] = None;
        }
        match scheduler::wait_for_child(task_id) {
            Some(exit_code) => {
                let _ = writeln!(output, "[{task_id} exited with {exit_code}]");
            }
            None => self.fail(output, "wait", "no such task"),
        }
    }

    /// `fw` (no args) lists the current rule count and how many packets have
    /// been dropped since boot; `fw deny <tcp|udp|any> <port>` adds a rule
    /// (`port 0` means "any port" for that protocol); `fw clear` removes
    /// every rule. Privileged (`ear`-gated) the same way `kill` is - a rule
    /// here affects every networking self-test and real traffic alike.
    fn command_fw(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        match arguments.get(offset) {
            None => {
                let _ = writeln!(
                    output,
                    "rules={} dropped={}",
                    crate::firewall::rule_count(),
                    crate::firewall::dropped_count()
                );
            }
            Some("clear") => {
                crate::firewall::clear_rules();
            }
            Some("deny") => {
                let protocol = match arguments.get(offset + 1) {
                    Some("tcp") => crate::firewall::Protocol::Tcp,
                    Some("udp") => crate::firewall::Protocol::Udp,
                    Some("any") => crate::firewall::Protocol::Any,
                    _ => {
                        self.usage_named(output, "fw");
                        return;
                    }
                };
                let Some(port) = arguments
                    .get(offset + 2)
                    .and_then(parse_u64)
                    .and_then(|value| u16::try_from(value).ok())
                else {
                    self.usage_named(output, "fw");
                    return;
                };
                if !crate::firewall::add_rule(protocol, port) {
                    self.fail(output, "fw", "rule table full");
                }
            }
            _ => self.usage_named(output, "fw"),
        }
    }

    fn command_sigcheck(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let (Some(file), Some(signature_file), Some(key_hex)) = (
            arguments.get(offset),
            arguments.get(offset + 1),
            arguments.get(offset + 2),
        ) else {
            self.usage_named(output, "sigcheck");
            return;
        };
        let Some(key) = crate::ed25519::decode_hex::<32>(key_hex) else {
            self.fail(output, "sigcheck", "public key must be 64 hex digits");
            return;
        };
        let (mut file_path, mut signature_path) = (Text::new(), Text::new());
        if self.make_path(file, &mut file_path).is_err()
            || self.make_path(signature_file, &mut signature_path).is_err()
        {
            self.fail(output, "sigcheck", "invalid path");
            return;
        }
        let mut signature = [0u8; 64];
        let mut raw = [0u8; 128];
        match read_file(signature_path.as_str(), &mut raw) {
            Ok(64) => signature.copy_from_slice(&raw[..64]),
            Ok(_) => {
                self.fail(
                    output,
                    "sigcheck",
                    "signature file must be exactly 64 bytes",
                );
                return;
            }
            Err(failure) => {
                self.fail_vfs(output, "sigcheck", failure);
                return;
            }
        }
        let Some(mut verifier) = crate::ed25519::Verifier::new(&key, &signature) else {
            let _ = writeln!(output, "BAD (malformed key or signature)");
            self.last_status = 1;
            return;
        };
        let handle = match vfs::open_file(file_path.as_str(), false, false, false, 0, false) {
            Ok(handle) => handle,
            Err(failure) => {
                self.fail_vfs(output, "sigcheck", failure);
                return;
            }
        };
        let mut chunk = [0u8; 1024];
        while let Ok(count) = vfs::read(handle, &mut chunk) {
            if count == 0 {
                break;
            }
            verifier.update(&chunk[..count]);
        }
        let _ = vfs::close(handle);
        if verifier.finish() {
            let _ = writeln!(output, "OK");
        } else {
            let _ = writeln!(output, "BAD");
            self.last_status = 1;
        }
    }

    fn command_acpi(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        use crate::acpi_ns;
        use crate::aml::NodeKind;
        let report = acpi_ns::report();
        if !report.loaded {
            self.fail(output, "acpi", "no ACPI namespace");
            return;
        }
        let Some(subcommand) = arguments.get(offset) else {
            let _ = writeln!(
                output,
                "{} tables, {} objects, {} devices, {} methods, {} load errors, APIC mode {}",
                report.tables,
                report.nodes,
                report.devices,
                report.methods,
                report.load_errors,
                if report.apic_mode { "on" } else { "off" }
            );
            return;
        };
        let mut path_buffer = [0u8; 96];
        let mut path_text = |index: usize| -> Option<([u8; 96], usize)> {
            let argument = arguments.get(index)?;
            let length = acpi_ns::absolute(argument, &mut path_buffer);
            Some((path_buffer, length))
        };
        match subcommand {
            "tree" => acpi_ns::with(|aml| {
                let mut shown = 0;
                for index in 0..aml.node_count() {
                    let node = index as u16;
                    if aml.node_kind(node) != NodeKind::Device || shown == 60 {
                        continue;
                    }
                    shown += 1;
                    let mut path = [0u8; 64];
                    let length = aml.node_path(node, &mut path);
                    let _ = write!(
                        output,
                        "{}",
                        core::str::from_utf8(&path[..length]).unwrap_or("?")
                    );
                    let mut id = [0u8; 8];
                    for name in [b"_HID", b"_CID"] {
                        if let Some(count) = aml.device_id(node, name, &mut id) {
                            let _ = write!(
                                output,
                                "  {}",
                                core::str::from_utf8(&id[..count]).unwrap_or("?")
                            );
                            break;
                        }
                    }
                    if let Some(status) = aml.child_node(node, b"_STA")
                        && let Ok(value) = aml.evaluate_node(status, &[])
                        && let Some(bits) = aml.integer(value)
                    {
                        let _ = write!(output, "  status {bits:#x}");
                    }
                    let _ = writeln!(output);
                }
            }),
            "eval" | "crs" => {
                let Some((buffer, length)) = path_text(offset + 1) else {
                    self.usage_named(output, "acpi");
                    return;
                };
                let Ok(path) = core::str::from_utf8(&buffer[..length]) else {
                    self.fail(output, "acpi", "bad path");
                    return;
                };
                let mut numbers = [0u64; 7];
                let mut count = 0;
                let mut position = offset + 2;
                while let Some(argument) = arguments.get(position) {
                    if count == numbers.len() {
                        break;
                    }
                    numbers[count] = parse_u64(argument).unwrap_or(0);
                    count += 1;
                    position += 1;
                }
                acpi_ns::with(|aml| match aml.evaluate(path, &numbers[..count]) {
                    Ok(value) if subcommand == "crs" => match aml.buffer(value) {
                        Some(template) => {
                            acpi_ns::describe_resources(template, output);
                            let _ = writeln!(output);
                        }
                        None => {
                            let _ = writeln!(output, "{path}: not a resource template");
                        }
                    },
                    Ok(value) => {
                        acpi_ns::describe_value(aml, value, output);
                        let _ = writeln!(output);
                    }
                    Err(error) => {
                        let _ = writeln!(output, "{path}: {error:?}");
                    }
                });
            }
            _ => self.usage_named(output, "acpi"),
        }
    }

    fn command_battery(&mut self, output: &mut Text<MAX_OUTPUT>) {
        use crate::{acpi_devices, acpi_ns};
        if !acpi_ns::report().loaded {
            self.fail(output, "battery", "no ACPI namespace");
            return;
        }
        crate::acpi_events::poll();
        acpi_ns::with(|aml| {
            let inventory = acpi_devices::scan(aml);
            if inventory.battery_count + inventory.adapter_count == 0 && inventory.lid.is_none() {
                let _ = writeln!(
                    output,
                    "no battery, adapter or lid device in the firmware tables"
                );
                return;
            }
            for node in &inventory.batteries[..inventory.battery_count] {
                let battery = acpi_devices::read_battery(aml, *node);
                let mut path = [0u8; 32];
                let length = aml.node_path(*node, &mut path);
                let name = core::str::from_utf8(&path[..length]).unwrap_or("?");
                if !battery.present {
                    let _ = writeln!(output, "{name}: no battery in the bay");
                    continue;
                }
                let state = if battery.charging {
                    "charging"
                } else if battery.discharging {
                    "discharging"
                } else {
                    "idle"
                };
                let (energy, power) = if battery.current_units {
                    ("mAh", "mA")
                } else {
                    ("mWh", "mW")
                };
                let _ = write!(output, "{name}: {state}");
                if let Some(percent) = battery.percent() {
                    let _ = write!(output, ", {percent}%");
                }
                if let Some(minutes) = battery.minutes() {
                    let _ = write!(output, ", {}h{:02}m", minutes / 60, minutes % 60);
                }
                if battery.critical {
                    let _ = write!(output, ", CRITICAL");
                }
                let _ = writeln!(output);
                let _ = writeln!(
                    output,
                    "  {} of {} {energy} (design {}), {} {power}, {} mV",
                    battery.remaining,
                    battery.full_capacity,
                    battery.design_capacity,
                    battery.rate,
                    battery.voltage_mv
                );
                if battery.model_length != 0 {
                    let _ = write!(
                        output,
                        "  model {}",
                        core::str::from_utf8(battery.model_text()).unwrap_or("?")
                    );
                    if let Some(cycles) = battery.cycles {
                        let _ = write!(output, ", {cycles} cycles");
                    }
                    let _ = writeln!(output);
                }
            }
            if let Some(online) = acpi_devices::on_mains(aml, &inventory) {
                let _ = writeln!(
                    output,
                    "mains adapter: {}",
                    if online { "online" } else { "offline" }
                );
            }
            if let Some(open) = acpi_devices::lid_open(aml, &inventory) {
                let _ = writeln!(output, "lid: {}", if open { "open" } else { "closed" });
            }
        });
    }

    fn command_thermal(&mut self, output: &mut Text<MAX_OUTPUT>) {
        use crate::{acpi_devices, acpi_events, acpi_ns};
        if !acpi_ns::report().loaded {
            self.fail(output, "thermal", "no ACPI namespace");
            return;
        }
        acpi_events::poll();
        acpi_ns::with(|aml| {
            let inventory = acpi_devices::scan(aml);
            let mut readable = 0;
            for node in &inventory.zones[..inventory.zone_count] {
                let Some(zone) = acpi_devices::read_zone(aml, *node) else {
                    continue;
                };
                readable += 1;
                let mut path = [0u8; 32];
                let length = aml.node_path(*node, &mut path);
                let _ = write!(
                    output,
                    "{}: {}.{} C",
                    core::str::from_utf8(&path[..length]).unwrap_or("?"),
                    zone.temperature / 10,
                    (zone.temperature % 10).abs()
                );
                for (label, limit) in [
                    ("passive", zone.passive),
                    ("hot", zone.hot),
                    ("critical", zone.critical),
                ] {
                    if let Some(limit) = limit {
                        let _ = write!(output, ", {label} {}", limit / 10);
                    }
                }
                let _ = writeln!(output);
            }
            if readable == 0 {
                let _ = writeln!(output, "no thermal zone with a readable temperature");
            }
        });
        let report = acpi_events::report();
        let _ = writeln!(
            output,
            "SCI {} routed {}, {} interrupts, {} events run, {} device notifications{}",
            report.sci,
            report.routed,
            acpi_events::interrupts(),
            acpi_events::events_run(),
            acpi_events::device_changes(),
            if acpi_events::line_shut_off() {
                ", LINE SHUT OFF (stuck)"
            } else {
                ""
            }
        );
    }

    fn command_install(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        use crate::installer::{self, Refusal, Target};
        let disks = crate::ahci::disk_count();
        let describe = |output: &mut Text<MAX_OUTPUT>, disk: usize| {
            let found = installer::inspect(Target::Ahci(disk));
            let _ = write!(output, "disk {disk}: {} MiB  ", found.sectors / 2048);
            if disk == crate::ahci::boot_disk() {
                let _ = writeln!(output, "boot disk, never touched");
            } else if found.blank {
                let _ = writeln!(output, "blank: the whole disk becomes the system partition");
            } else {
                match found.table {
                    Ok(count) => {
                        let _ = writeln!(
                            output,
                            "GPT with {count} partitions, largest free gap {} MiB (needs {} MiB)",
                            found.free_sectors / 2048,
                            installer::MIN_PARTITION_SECTORS / 2048
                        );
                    }
                    Err(Refusal::Unsupported) => {
                        let _ = writeln!(output, "partition table not supported (MBR): refused");
                    }
                    Err(Refusal::Damaged) => {
                        let _ = writeln!(output, "damaged partition table: refused");
                    }
                    Err(_) => {
                        let _ = writeln!(output, "holds data without a GPT: refused");
                    }
                }
            }
        };
        let Some(number) = arguments.get(offset) else {
            if disks == 0 {
                self.fail(output, "install", "no SATA disk found");
            }
            for disk in 0..disks {
                describe(output, disk);
            }
            return;
        };
        let Some(disk) = parse_u64(number).map(|value| value as usize) else {
            self.usage_named(output, "install");
            return;
        };
        if disk >= disks {
            self.fail(output, "install", "no such disk");
            return;
        }
        if disk == crate::ahci::boot_disk() {
            self.fail(output, "install", "the boot disk is never a target");
            return;
        }
        if arguments.get(offset + 1) != Some("--yes") {
            describe(output, disk);
            let _ = writeln!(
                output,
                "nothing was changed; add --yes to install (existing partitions stay as they are)"
            );
            return;
        }
        let report = installer::install_to(
            Target::Ahci(disk),
            true,
            installer::MIN_PARTITION_SECTORS,
            None,
        );
        if report.verified {
            let _ = writeln!(
                output,
                "installed on disk {disk}{}; {} existing partitions kept intact",
                if report.existing_table {
                    " in its free space"
                } else {
                    ""
                },
                report.preserved
            );
        } else if !report.gpt && !report.formatted {
            self.fail(output, "install", "refused: nothing was written");
        } else {
            self.fail(
                output,
                "install",
                "failed part way; the old partitions were not touched",
            );
        }
    }

    fn command_aerfs(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let (Some(action @ ("format" | "mount")), Some(name)) =
            (arguments.get(offset), arguments.get(offset + 1))
        else {
            self.usage_named(output, "aerfs");
            return;
        };
        let Some(disk) = crate::datafs::disk_by_name(name) else {
            self.fail(output, "aerfs", "no such disk (see lsblk)");
            return;
        };
        let result = if action == "format" {
            let label = arguments.get(offset + 2).unwrap_or("AEROS");
            crate::datafs::format_aerfs(disk, label.as_bytes())
        } else {
            crate::datafs::mount_aerfs(disk)
        };
        match result {
            Ok((mount, length)) => {
                let _ = writeln!(
                    output,
                    "{name} is AerFS, mounted at /media/{}",
                    core::str::from_utf8(&mount[..length]).unwrap_or("?")
                );
            }
            Err(failure) => self.fail_vfs(output, "aerfs", failure),
        }
    }

    fn command_fsck(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let repair = arguments.get(offset) == Some("--repair");
        let target = arguments
            .get(offset + usize::from(repair))
            .unwrap_or("/home");
        let mut path: Text<MAX_PATH> = Text::new();
        if self.make_path(target, &mut path).is_err() {
            self.fail(output, "fsck", "invalid path");
            return;
        }
        let Some((mount, _)) = crate::datafs::route(path.as_str()) else {
            self.fail(output, "fsck", "not a /home or /media mount point");
            return;
        };
        match crate::datafs::fsck(mount, repair) {
            Ok(report) => {
                let _ = writeln!(
                    output,
                    "files={} directories={} broken_chains={} cross_linked={} size_mismatches={} orphan_clusters={} duplicates={}{}",
                    report.files,
                    report.directories,
                    report.broken_chains,
                    report.cross_linked,
                    report.size_mismatches,
                    report.orphan_clusters,
                    report.duplicates,
                    if repair { " (repaired)" } else { "" }
                );
                if report.damaged() && !repair {
                    self.last_status = 1;
                }
            }
            Err(failure) => self.fail_vfs(output, "fsck", failure),
        }
    }

    /// Prints the last `lines` (default 16, at most 64) syscalls any Linux
    /// process made. Privileged because it shows other processes' activity.
    fn command_strace(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let wanted = match arguments.get(offset) {
            None => 16,
            Some(value) => match parse_u64(value) {
                Some(count) if (1..=64).contains(&count) => count as usize,
                _ => {
                    self.usage_named(output, "strace");
                    return;
                }
            },
        };
        let _ = writeln!(output, "syscalls since boot: {}", crate::trace::total());
        let skip = crate::trace::total().min(64).saturating_sub(wanted as u64) as usize;
        let mut index = 0usize;
        crate::trace::for_each(|entry| {
            if index >= skip {
                let _ = writeln!(
                    output,
                    "#{} pid={} {}({}) arg0={:#x} at={}ms",
                    entry.sequence,
                    entry.task,
                    crate::syscall::linux_syscall_name(entry.number),
                    entry.number,
                    entry.argument,
                    entry.at_ms
                );
            }
            index += 1;
        });
    }

    /// Lists crash records; crashes save [path] writes them plus the recent
    /// kernel log to a file (default /home/crash-report.txt) so they outlast
    /// a reboot.
    fn command_crashes(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        if arguments.get(offset) == Some("save") {
            self.save_crash_report(arguments.get(offset + 1), output);
            return;
        }
        write_crash_records(output);
    }

    fn save_crash_report(&mut self, target: Option<&str>, output: &mut Text<MAX_OUTPUT>) {
        let mut path: Text<MAX_PATH> = Text::new();
        if self
            .make_path(target.unwrap_or("/home/crash-report.txt"), &mut path)
            .is_err()
        {
            self.fail(output, "crashes", "invalid path");
            return;
        }
        let mut report: Text<MAX_OUTPUT> = Text::new();
        let _ = writeln!(
            report,
            "AerOS crash report, uptime {} ms",
            time::monotonic_nanoseconds() / 1_000_000
        );
        write_crash_records(&mut report);
        let _ = writeln!(report, "--- kernel log (last 40 lines) ---");
        append_log_tail(&mut report, 40);
        let bytes = report.as_str().as_bytes();
        let wrote = vfs::open_file(path.as_str(), true, false, true, 0o600, true)
            .map(|descriptor| {
                let result = vfs::write(descriptor, bytes, false);
                let _ = vfs::close(descriptor);
                result == Ok(bytes.len())
            })
            .unwrap_or(false);
        if wrote {
            let _ = writeln!(output, "saved {} bytes to {}", bytes.len(), path.as_str());
        } else {
            self.fail(output, "crashes", "cannot write report");
        }
    }

    fn command_uniq(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let counts = arguments.get(offset) == Some("-c");
        let mut data = [0u8; 4096];
        let length = match self.read_input(arguments.get(offset + usize::from(counts)), &mut data) {
            Ok((length, _)) if textual(&data[..length]) => length,
            Ok(_) => {
                self.fail(output, "uniq", "binary file");
                return;
            }
            Err(failure) => {
                self.input_failed(output, "uniq", failure);
                return;
            }
        };
        let text = &data[..length];
        let text = text.strip_suffix(b"\n").unwrap_or(text);
        if text.is_empty() {
            return;
        }
        let mut write_run = |line: &[u8], run: usize| {
            if counts {
                let _ = write!(output, "{run:>4} ");
            }
            write_file_bytes(output, line);
            let _ = writeln!(output);
        };
        let mut previous: Option<&[u8]> = None;
        let mut run = 0usize;
        for line in text.split(|byte| *byte == b'\n') {
            if previous == Some(line) {
                run += 1;
                continue;
            }
            if let Some(last) = previous {
                write_run(last, run);
            }
            previous = Some(line);
            run = 1;
        }
        if let Some(last) = previous {
            write_run(last, run);
        }
    }

    fn command_tee(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let Some(file) = arguments.get(offset) else {
            self.usage_named(output, "tee");
            return;
        };
        let mut path: Text<MAX_PATH> = Text::new();
        if self.make_path(file, &mut path).is_err() {
            self.fail(output, "tee", "invalid path");
            return;
        }
        let mut data = [0u8; 4096];
        let length = match self.read_input(None, &mut data) {
            Ok((length, _)) => length,
            Err(failure) => {
                self.input_failed(output, "tee", failure);
                return;
            }
        };
        let wrote = vfs::open_file(path.as_str(), true, false, true, 0o644, true)
            .map(|descriptor| {
                let result = vfs::write(descriptor, &data[..length], false);
                let _ = vfs::close(descriptor);
                result == Ok(length)
            })
            .unwrap_or(false);
        if !wrote {
            self.fail(output, "tee", "cannot write file");
            return;
        }
        write_file_bytes(output, &data[..length]);
    }

    fn command_renice(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let nice = arguments.get(offset).and_then(parse_i64);
        let pid = arguments.get(offset + 1).and_then(parse_u64);
        let (Some(nice), Some(pid)) = (nice, pid) else {
            self.usage_named(output, "renice");
            return;
        };
        if !(-20..=19).contains(&nice) {
            self.fail(output, "renice", "nice must be between -20 and 19");
            return;
        }
        if !scheduler::set_nice(pid, nice as i8) {
            self.fail(output, "renice", "no live process with that PID");
        }
    }

    fn command_grep(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let (mut ignore_case, mut numbers, mut count_only) = (false, false, false);
        let mut index = offset;
        while let Some(flag) = arguments.get(index) {
            match flag {
                "-i" => ignore_case = true,
                "-n" => numbers = true,
                "-c" => count_only = true,
                _ => break,
            }
            index += 1;
        }
        let Some(pattern) = arguments.get(index) else {
            self.usage_named(output, "grep");
            return;
        };
        if pattern.is_empty() {
            self.fail(output, "grep", "invalid argument");
            return;
        }
        let mut data = [0u8; 4096];
        let length = match self.read_input(arguments.get(index + 1), &mut data) {
            Ok((length, _)) if textual(&data[..length]) => length,
            Ok(_) => {
                self.fail(output, "grep", "binary file");
                return;
            }
            Err(InputFailure::Missing) => {
                self.usage_named(output, "grep");
                return;
            }
            Err(failure) => {
                self.input_failed(output, "grep", failure);
                return;
            }
        };
        let mut matches = 0;
        for (number, line) in data[..length].split(|byte| *byte == b'\n').enumerate() {
            if !contains_bytes(line, pattern.as_bytes(), ignore_case) {
                continue;
            }
            matches += 1;
            if !count_only {
                if numbers {
                    let _ = write!(output, "{}:", number + 1);
                }
                write_file_bytes(output, line);
                let _ = writeln!(output);
            }
        }
        if count_only {
            let _ = writeln!(output, "{matches}");
        }
        if matches == 0 {
            self.last_status = 1;
        }
    }

    fn command_find(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let mut index = offset;
        let mut root = self.cwd.as_str();
        if let Some(first) = arguments.get(index)
            && first != "-name"
        {
            root = first;
            index += 1;
        }
        let mut name = None;
        if arguments.get(index) == Some("-name") {
            name = arguments.get(index + 1);
            if name.is_none() {
                self.usage_named(output, "find");
                return;
            }
        }
        let mut path = Text::new();
        if self.make_path(root, &mut path).is_err() {
            self.fail(output, "find", "invalid path");
            return;
        }
        if let Err(failure) = vfs::metadata(path.as_str()) {
            self.fail_vfs(output, "find", failure);
            return;
        }
        walk_tree(path.as_str(), 0, &mut |child, _, _| {
            let last = child.rsplit('/').next().unwrap_or(child);
            if name.is_none_or(|wanted| last.contains(wanted)) {
                let _ = writeln!(output, "{child}");
            }
        });
    }

    fn command_du(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let mut path = Text::new();
        if self
            .make_path(
                arguments.get(offset).unwrap_or(self.cwd.as_str()),
                &mut path,
            )
            .is_err()
        {
            self.fail(output, "du", "invalid path");
            return;
        }
        let metadata = match vfs::metadata(path.as_str()) {
            Ok(metadata) => metadata,
            Err(failure) => {
                self.fail_vfs(output, "du", failure);
                return;
            }
        };
        let mut total = 0u64;
        if metadata.mode & 0o170000 == 0o040000 {
            walk_tree(path.as_str(), 0, &mut |_, _, size| total += size);
        } else {
            total = metadata.size;
        }
        let _ = writeln!(output, "{total} {}", path.as_str());
    }

    fn command_sort(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let reverse = arguments.get(offset) == Some("-r");
        let mut data = [0u8; 4096];
        let length = match self.read_input(arguments.get(offset + usize::from(reverse)), &mut data)
        {
            Ok((length, _)) if textual(&data[..length]) => length,
            Ok(_) => {
                self.fail(output, "sort", "binary file");
                return;
            }
            Err(failure) => {
                self.input_failed(output, "sort", failure);
                return;
            }
        };
        let mut lines = [(0usize, 0usize); 256];
        let mut count = 0;
        let mut start = 0;
        for line in data[..length].split(|byte| *byte == b'\n') {
            if !line.is_empty() && count < lines.len() {
                lines[count] = (start, start + line.len());
                count += 1;
            }
            start += line.len() + 1;
        }
        lines[..count].sort_unstable_by(|a, b| {
            let order = data[a.0..a.1].cmp(&data[b.0..b.1]);
            if reverse { order.reverse() } else { order }
        });
        for (start, end) in &lines[..count] {
            write_file_bytes(output, &data[*start..*end]);
            let _ = writeln!(output);
        }
    }

    fn command_basename(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
        directory: bool,
    ) {
        let Some(input) = arguments.get(offset) else {
            self.usage_named(output, if directory { "dirname" } else { "basename" });
            return;
        };
        let trimmed = input.trim_end_matches('/');
        let result = if directory {
            match trimmed.rfind('/') {
                _ if trimmed.is_empty() && input.starts_with('/') => "/",
                None => ".",
                Some(0) => "/",
                Some(split) => &trimmed[..split],
            }
        } else if trimmed.is_empty() && input.starts_with('/') {
            "/"
        } else {
            trimmed.rsplit('/').next().unwrap_or(trimmed)
        };
        let _ = writeln!(output, "{result}");
    }

    fn command_seq(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let first_value = arguments.get(offset).and_then(parse_u64);
        let second_value = arguments.get(offset + 1).and_then(parse_u64);
        let (first, last) = match (first_value, second_value) {
            (Some(last), None) if arguments.get(offset + 1).is_none() => (1, last),
            (Some(first), Some(last)) => (first, last),
            _ => {
                self.usage_named(output, "seq");
                return;
            }
        };
        if last.saturating_sub(first) > 1000 {
            self.fail(output, "seq", "range exceeds 1000 numbers");
            return;
        }
        for value in first..=last {
            let _ = writeln!(output, "{value}");
        }
    }

    fn command_pkg(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        use crate::pkg;
        const PKG_ROOT: &str = "/home/pkg";
        match (arguments.get(offset), arguments.get(offset + 1)) {
            (None | Some("list"), _) => {
                let result = pkg::for_each(PKG_ROOT, |entry| {
                    let _ = write!(
                        output,
                        "{:<24} {}",
                        entry.name.as_str(),
                        entry.current.as_str()
                    );
                    if let Some(previous) = &entry.previous {
                        let _ = write!(output, " (previous {})", previous.as_str());
                    }
                    let _ = writeln!(output);
                });
                if let Err(failure) = result {
                    self.fail(output, "pkg", failure.message());
                }
            }
            (Some("install"), Some(file)) => {
                let mut package = Text::new();
                if self.make_path(file, &mut package).is_err() {
                    self.fail(output, "pkg", "invalid path");
                    return;
                }
                let mut signature: Text<MAX_PATH> = Text::new();
                match arguments.get(offset + 2) {
                    Some(explicit) => {
                        if self.make_path(explicit, &mut signature).is_err() {
                            self.fail(output, "pkg", "invalid path");
                            return;
                        }
                    }
                    None => {
                        if write!(signature, "{}.sig", package.as_str()).is_err() {
                            self.fail(output, "pkg", "path too long");
                            return;
                        }
                    }
                }
                match pkg::install(PKG_ROOT, package.as_str(), signature.as_str()) {
                    Ok(installed) => {
                        let _ = writeln!(
                            output,
                            "installed {} {} ({} files)",
                            installed.name.as_str(),
                            installed.version.as_str(),
                            installed.files
                        );
                    }
                    Err(failure) => self.fail(output, "pkg", failure.message()),
                }
            }
            (Some("remove"), Some(name)) => {
                if let Err(failure) = pkg::remove(PKG_ROOT, name) {
                    self.fail(output, "pkg", failure.message());
                }
            }
            (Some("rollback"), Some(name)) => match pkg::rollback(PKG_ROOT, name) {
                Ok(version) => {
                    let _ = writeln!(output, "{name} is now {}", version.as_str());
                }
                Err(failure) => self.fail(output, "pkg", failure.message()),
            },
            (Some("verify"), Some(name)) => match pkg::verify(PKG_ROOT, name) {
                Ok(files) => {
                    let _ = writeln!(output, "{name}: {files} files verified");
                }
                Err(failure) => self.fail(output, "pkg", failure.message()),
            },
            (Some("path"), Some(name)) => match pkg::path_of(PKG_ROOT, name) {
                Ok(path) => {
                    let _ = writeln!(output, "{}", path.as_str());
                }
                Err(failure) => self.fail(output, "pkg", failure.message()),
            },
            (Some("trust"), Some(key_hex)) => {
                let Some(key) = crate::ed25519::decode_hex::<32>(key_hex) else {
                    self.fail(output, "pkg", "public key must be 64 hex digits");
                    return;
                };
                if let Err(failure) = pkg::trust(PKG_ROOT, &key) {
                    self.fail(output, "pkg", failure.message());
                }
            }
            _ => self.usage_named(output, "pkg"),
        }
    }

    fn command_svc(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        use crate::services;
        let name = arguments.get(offset + 1);
        match (arguments.get(offset), name) {
            (None | Some("list"), _) => {
                let mut names: [Option<Text<24>>; services::MAX_SERVICES] =
                    [None; services::MAX_SERVICES];
                let mut count = 0;
                services::for_each(|service| {
                    let mut entry = Text::new();
                    let _ = entry.push_str_checked(service.name());
                    names[count] = Some(entry);
                    count += 1;
                });
                for entry in names.iter().flatten() {
                    let Some(service) = svc_refresh(entry.as_str()) else {
                        continue;
                    };
                    let state = match (service.task_id, service.last_exit) {
                        (Some(task), _) => {
                            let mut text: Text<24> = Text::new();
                            let _ = write!(text, "running (task {task})");
                            text
                        }
                        (None, Some(code)) => {
                            let mut text: Text<24> = Text::new();
                            let _ = write!(text, "stopped (exit {code})");
                            text
                        }
                        (None, None) => {
                            let mut text: Text<24> = Text::new();
                            let _ = text.push_str_checked("never started");
                            text
                        }
                    };
                    let _ = writeln!(
                        output,
                        "{:<24} {:<22} {}",
                        service.name(),
                        state.as_str(),
                        service.path()
                    );
                }
            }
            (Some("add"), Some(name)) => {
                let Some(input) = arguments.get(offset + 2) else {
                    self.usage_named(output, "svc");
                    return;
                };
                let mut path = Text::new();
                if self.make_path(input, &mut path).is_err() {
                    self.fail(output, "svc", "invalid path");
                    return;
                }
                if let Err(failure) = services::add(name, path.as_str()) {
                    self.fail(output, "svc", service_error(failure));
                }
            }
            (Some("start"), Some(name)) => self.svc_start(name, output),
            (Some("stop"), Some(name)) => self.svc_stop(name, output),
            (Some("restart"), Some(name)) => {
                self.svc_stop(name, output);
                if self.last_status == 0 {
                    self.svc_start(name, output);
                }
            }
            (Some("rm"), Some(name)) => {
                self.svc_stop(name, output);
                if self.last_status == 0
                    && let Err(failure) = services::remove(name)
                {
                    self.fail(output, "svc", service_error(failure));
                }
            }
            _ => self.usage_named(output, "svc"),
        }
    }

    fn svc_start(&mut self, name: &str, output: &mut Text<MAX_OUTPUT>) {
        let Some(service) = svc_refresh(name) else {
            self.fail(output, "svc", "no such service");
            return;
        };
        if service.task_id.is_some() {
            self.fail(output, "svc", "already running");
            return;
        }
        let Ok(command) = Arguments::parse(service.path()) else {
            self.fail(output, "svc", "invalid path");
            return;
        };
        match self.launch_program(&command, 0, command.count) {
            Ok((_, RunOutcome::Ran(id))) => {
                let _ = crate::services::update(name, |entry| {
                    entry.task_id = Some(id);
                    entry.starts += 1;
                });
                let _ = writeln!(output, "started {name} (task {id})");
            }
            Ok((_, RunOutcome::Blocked(found))) => {
                let _ = writeln!(output, "svc: blocked by AerOS Shield: {}", found.name);
                self.last_status = 126;
            }
            Ok((_, RunOutcome::NotExecutable)) => {
                self.fail(output, "svc", "not a valid executable")
            }
            Err(LaunchFailure::InvalidPath) => self.fail(output, "svc", "invalid path"),
            Err(LaunchFailure::PagingNotReady) => self.fail(output, "svc", "paging not ready"),
            Err(LaunchFailure::Vfs(failure)) => self.fail_vfs(output, "svc", failure),
        }
    }

    fn svc_stop(&mut self, name: &str, output: &mut Text<MAX_OUTPUT>) {
        let Some(service) = svc_refresh(name) else {
            self.fail(output, "svc", "no such service");
            return;
        };
        let Some(task_id) = service.task_id else {
            return;
        };
        let _ = process::request_exit(task_id, 128 + 15);
        let exit_code = scheduler::wait_for_child(task_id).unwrap_or(0);
        let _ = crate::services::update(name, |entry| {
            entry.task_id = None;
            entry.last_exit = Some(exit_code);
        });
        let _ = writeln!(output, "stopped {name} (exit {exit_code})");
    }

    /// Prints the last `lines` (default 20, at most 200) lines of the kernel
    /// log. Privileged because the log includes kernel addresses.
    fn command_dmesg(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let wanted = match arguments.get(offset) {
            None => 20,
            Some(value) => match parse_u64(value) {
                Some(count) if (1..=200).contains(&count) => count as usize,
                _ => {
                    self.usage_named(output, "dmesg");
                    return;
                }
            },
        };
        append_log_tail(output, wanted);
    }

    fn command_free(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let kernel_heap = heap::HEAP.stats();
        let usable_kib = self.info.memory.usable_pages().saturating_mul(4);
        let physical_free_kib = self.info.allocator.free_pages.saturating_mul(4);
        let _ = writeln!(output, "             total KiB      free KiB      used KiB");
        let _ = writeln!(
            output,
            "physical {:>13} {:>13} {:>13}",
            usable_kib,
            physical_free_kib,
            usable_kib.saturating_sub(physical_free_kib)
        );
        let _ = writeln!(
            output,
            "heap     {:>13} {:>13} {:>13}",
            kernel_heap.total_bytes / 1024,
            kernel_heap.free_bytes / 1024,
            kernel_heap
                .total_bytes
                .saturating_sub(kernel_heap.free_bytes)
                / 1024
        );
        let _ = writeln!(
            output,
            "pressure {} (oom kills: {})",
            crate::oom::pressure().label(),
            crate::oom::kills()
        );
    }

    fn command_lscpu(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let cpu = self.info.cpu;
        let _ = writeln!(output, "Architecture: x86_64");
        let _ = writeln!(output, "CPU(s): {}", self.info.cpu_count);
        let _ = writeln!(output, "Vendor ID: {}", cpu.vendor());
        let _ = writeln!(
            output,
            "Features: nx={} syscall={} 1g-pages={} x2apic={} smep={} smap={} sse={} xsave={} avx={}",
            cpu.nx,
            cpu.syscall,
            cpu.one_gib_pages,
            cpu.x2apic,
            cpu.smep,
            cpu.smap,
            cpu.sse,
            cpu.xsave,
            cpu.avx
        );
    }

    fn command_lspci(&mut self, output: &mut Text<MAX_OUTPUT>) {
        for device in self.info.pci.devices() {
            let _ = writeln!(
                output,
                "{:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}:{:02x}:{:02x}",
                device.bus,
                device.slot,
                device.function,
                device.vendor,
                device.device,
                device.class,
                device.subclass,
                device.programming_interface
            );
        }
    }

    fn command_lsblk(&mut self, output: &mut Text<MAX_OUTPUT>) {
        use crate::fatfs::Disk;
        let mut disks = [None; 8];
        let mut count = 0;
        for index in 0..crate::ahci::disk_count().min(3) {
            disks[count] = Some(Disk::Ahci(index));
            count += 1;
        }
        for disk in [Disk::Nvme, Disk::Usb, Disk::Sd, Disk::Virtio] {
            disks[count] = Some(disk);
            count += 1;
        }
        let _ = writeln!(output, "NAME      SIZE MiB  TYPE  MOUNTPOINT");
        let mut listed = false;
        for disk in disks.into_iter().flatten() {
            let sectors = disk.sectors();
            if sectors == 0 {
                continue;
            }
            listed = true;
            let name = crate::datafs::device_name(disk);
            let mut mount_path: Text<32> = Text::new();
            let mut index = 0;
            while index < 4 {
                if let Some(mount) = crate::datafs::mount_info(index)
                    && mount.device == name
                {
                    let _ = mount_path.push_str_checked(mount.path());
                }
                index += 1;
            }
            let _ = writeln!(
                output,
                "{:<9} {:>8}  disk  {}",
                name,
                sectors * 512 / 1024 / 1024,
                mount_path.as_str()
            );
        }
        if !listed {
            self.fail(output, "lsblk", "no block device detected");
        }
    }

    fn command_mount(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        if let Some(option) = arguments.get(offset) {
            if option != "--bind" && option != "-B" {
                self.usage_named(output, "mount");
                return;
            }
            let (Some(source), Some(target)) =
                (arguments.get(offset + 1), arguments.get(offset + 2))
            else {
                self.usage_named(output, "mount");
                return;
            };
            if !self.is_elevated() {
                self.fail(output, "mount", "permission denied (use ear)");
                return;
            }
            let (mut source_path, mut target_path) = (Text::new(), Text::new());
            if self.make_path(source, &mut source_path).is_err()
                || self.make_path(target, &mut target_path).is_err()
            {
                self.fail(output, "mount", "invalid path");
                return;
            }
            match crate::mounts::bind(target_path.as_str(), source_path.as_str()) {
                Ok(()) => {
                    let _ = writeln!(
                        output,
                        "{} bound over {}",
                        source_path.as_str(),
                        target_path.as_str()
                    );
                }
                Err(crate::mounts::MountError::NotDirectory) => {
                    self.fail(output, "mount", "both paths must be directories")
                }
                Err(crate::mounts::MountError::Busy) => {
                    self.fail(output, "mount", "target is already a mount point")
                }
                Err(crate::mounts::MountError::Full) => {
                    self.fail(output, "mount", "bind table is full")
                }
                Err(crate::mounts::MountError::NotAllowed) => self.fail(
                    output,
                    "mount",
                    "source must be on /home or /media and the target in the root tree",
                ),
                Err(_) => self.fail(output, "mount", "invalid path"),
            }
            return;
        }
        let _ = writeln!(output, "initramfs on / type initramfs (ro)");
        let _ = writeln!(output, "tmpfs on /tmp type tmpfs (rw,nosuid,nodev)");
        let _ = writeln!(output, "proc on /proc type proc (ro)");
        let _ = writeln!(output, "sysfs on /sys type sysfs (ro)");
        let mut index = 0;
        while index < 4 {
            if let Some(mount) = crate::datafs::mount_info(index) {
                let _ = writeln!(
                    output,
                    "{} on {} type fat{} (rw)",
                    mount.device,
                    mount.path(),
                    if mount.fat32 { 32 } else { 16 }
                );
            }
            index += 1;
        }
        crate::mounts::each_bind(|target, source| {
            let _ = writeln!(output, "{source} on {target} type none (rw,bind)");
        });
        if self.info.fat.mounted {
            let _ = writeln!(
                output,
                "sda1 on /boot type fat{} (ro)",
                self.info.fat.fat_bits
            );
        }
    }

    fn command_umount(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(path) = arguments.get(offset) else {
            self.usage_named(output, "umount");
            return;
        };
        let mut resolved = Text::new();
        let path = if self.make_path(path, &mut resolved).is_ok() {
            resolved.as_str()
        } else {
            path
        };
        if crate::mounts::is_mount_point(path) {
            match crate::mounts::unbind(path) {
                Ok(()) => {
                    let _ = writeln!(output, "{path} unmounted");
                }
                Err(_) => self.fail(output, "umount", "not mounted"),
            }
            return;
        }
        if let Some(name) = path.strip_prefix("/media/") {
            match crate::datafs::eject(name.trim_end_matches('/')) {
                Ok(()) => {
                    let _ = writeln!(output, "{path} unmounted; it is safe to remove");
                }
                Err(vfs::VfsError::Busy) => self.fail(output, "umount", "target is busy"),
                Err(_) => self.fail(output, "umount", "not mounted"),
            }
            return;
        }
        match path {
            "/" | "/tmp" => self.fail(
                output,
                "umount",
                "filesystem is required by the running shell",
            ),
            "/boot" if self.info.fat.mounted => {
                self.fail(output, "umount", "read-only boot inspection is pinned")
            }
            _ => self.fail(output, "umount", "not mounted"),
        }
    }

    fn command_df(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let stats = vfs::stats();
        let tmp_capacity: usize = 8 * 4096;
        let _ = writeln!(
            output,
            "Filesystem   1K-blocks  Used  Available  Mounted on"
        );
        let _ = writeln!(
            output,
            "initramfs    {:>9}  {:>4}  {:>9}  /",
            stats.bytes.div_ceil(1024),
            stats.bytes.div_ceil(1024),
            0
        );
        let _ = writeln!(
            output,
            "tmpfs        {:>9}  {:>4}  {:>9}  /tmp",
            tmp_capacity / 1024,
            stats.mutable_bytes.div_ceil(1024),
            tmp_capacity.saturating_sub(stats.mutable_bytes) / 1024
        );
        let mut index = 0;
        while index < 4 {
            if let Some(mount) = crate::datafs::mount_info(index) {
                let _ = writeln!(
                    output,
                    "{:<12} {:>9}  {:>4}  {:>9}  {}",
                    mount.device,
                    mount.total_bytes / 1024,
                    (mount.total_bytes - mount.free_bytes) / 1024,
                    mount.free_bytes / 1024,
                    mount.path()
                );
            }
            index += 1;
        }
    }

    fn command_sync(&mut self, output: &mut Text<MAX_OUTPUT>) {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let _ = writeln!(
            output,
            "tmpfs synchronized; block-backed filesystems are read-only"
        );
    }

    fn command_which(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(name) = arguments.get(offset) else {
            self.usage_named(output, "which");
            return;
        };
        if find_command(name).is_some() {
            let _ = writeln!(output, "aersh builtin: {name}");
        } else {
            self.last_status = 1;
        }
    }

    fn command_env(&mut self, output: &mut Text<MAX_OUTPUT>) {
        for entry in self.environment.iter().filter(|entry| entry.used) {
            let _ = writeln!(output, "{}={}", entry.name.as_str(), entry.value.as_str());
        }
    }

    fn command_export(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(assignment) = arguments.get(offset) else {
            self.usage_named(output, "export");
            return;
        };
        let Some(index) = assignment.find('=') else {
            self.fail(output, "export", "expected NAME=VALUE");
            return;
        };
        if !self.set_environment(&assignment[..index], &assignment[index + 1..]) {
            self.fail(output, "export", "invalid name, value, or full environment");
        }
    }

    fn command_unset(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(name) = arguments.get(offset) else {
            self.usage_named(output, "unset");
            return;
        };
        if let Some(entry) = self
            .environment
            .iter_mut()
            .find(|entry| entry.used && entry.name.as_str() == name)
        {
            *entry = EnvironmentEntry::EMPTY;
        }
    }

    fn command_history(&mut self, output: &mut Text<MAX_OUTPUT>) {
        for (index, item) in self.history[..self.history_count].iter().enumerate() {
            let _ = writeln!(output, "{:>3}  {}", index + 1, item.as_str());
        }
    }
}

impl Shell<'_> {
    fn command_ip(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let link = self.info.network;
        let internet = self.info.internet;
        let _ = writeln!(output, "1: lo: <UP,LOOPBACK> mtu 65536");
        let _ = writeln!(output, "    inet 127.0.0.1/8");
        let _ = writeln!(
            output,
            "2: eth0: <{},BROADCAST> mtu 1500 driver e1000e",
            if link.link { "UP" } else { "DOWN" }
        );
        let _ = write!(output, "    link/ether ");
        write_mac(output, link.mac);
        let _ = writeln!(output);
        let _ = write!(output, "    inet ");
        write_ipv4(output, internet.local_ip);
        let _ = writeln!(output, "/24 dhcp lease {}s", internet.lease_seconds);
        let (link_local, global) = crate::net::ipv6_addresses(link.mac);
        let _ = write!(output, "    inet6 ");
        write_ipv6(output, link_local);
        let _ = writeln!(output, "/64 scope link");
        if let Some(address) = global {
            let _ = write!(output, "    inet6 ");
            write_ipv6(output, address);
            let _ = writeln!(output, "/64 scope global autoconf");
        }
        let _ = writeln!(output, "    inet6 ::1/128 scope host (lo)");
    }

    fn command_ping(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        if let Some(value) = arguments.get(offset)
            && value.contains(':')
        {
            let Some(address) = crate::ip::parse(value).filter(|a| !crate::ip::is_v4(a)) else {
                self.fail(output, "ping", "expected an IP address");
                return;
            };
            match crate::ipv6::ping(&address, 0xae61, 1, 3_000_000_000) {
                Some(elapsed) => {
                    let _ = write!(output, "reply from ");
                    write_ipv6(output, address);
                    let _ = writeln!(
                        output,
                        ": time={}.{:03} ms",
                        elapsed / 1_000_000,
                        elapsed / 1_000 % 1000
                    );
                }
                None => self.fail(output, "ping", "request timed out"),
            }
            return;
        }
        let destination = match arguments.get(offset) {
            Some(value) => match parse_ipv4(value) {
                Some(address) => address,
                None => {
                    self.fail(output, "ping", "expected an IPv4 address");
                    return;
                }
            },
            None => self.info.internet.gateway_ip,
        };
        let report = crate::net::ping(destination);
        if report.verified {
            let _ = write!(output, "reply from ");
            write_ipv4(output, report.destination);
            let _ = writeln!(
                output,
                ": bytes={} time={}.{:03} ms",
                report.bytes,
                report.elapsed_ns / 1_000_000,
                report.elapsed_ns / 1_000 % 1000
            );
        } else {
            self.fail(output, "ping", "request timed out");
        }
    }

    fn command_dns(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        if arguments.get(offset) == Some("-6") {
            let name = arguments.get(offset + 1).unwrap_or("localhost");
            match crate::net::resolve6(name) {
                Some(address) => {
                    let _ = write!(output, "{name} has IPv6 address ");
                    write_ipv6(output, address);
                    let _ = writeln!(output);
                }
                None => self.fail(output, "dns", "lookup failed"),
            }
            return;
        }
        let name = arguments.get(offset).unwrap_or("localhost");
        let lookup = crate::net::resolve(name);
        if lookup.verified {
            let _ = write!(output, "{name} has address ");
            write_ipv4(output, lookup.address);
            let _ = write!(output, " via ");
            write_ipv4(output, lookup.server);
            let _ = writeln!(output, " ({} answers)", lookup.answers);
        } else {
            self.fail(output, "dns", "lookup failed");
        }
    }

    fn command_https(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let Some(name) = arguments.get(offset) else {
            self.usage_named(output, "https");
            return;
        };
        let path = arguments.get(offset + 1).unwrap_or("/");
        let lookup = crate::net::resolve(name);
        if !lookup.verified {
            self.fail(output, "https", "lookup failed");
            return;
        }
        let mut response = [0u8; 4096];
        match crate::tlsnet::https_get(
            crate::ip::v4(lookup.address),
            443,
            name,
            path,
            &mut response,
            crate::tls::roots::ROOTS,
        ) {
            Ok(reply) => {
                let _ = writeln!(
                    output,
                    "HTTP {} ({} bytes, TLS 1.3, certificate verified)",
                    reply.status, reply.bytes
                );
                let text = &response[..reply.bytes.min(3000)];
                for &byte in text {
                    let shown = match byte {
                        b'\n' | b'\t' | 0x20..=0x7e => byte,
                        b'\r' => continue,
                        _ => b'.',
                    };
                    let _ = output.write_char(char::from(shown));
                }
                let _ = writeln!(output);
            }
            Err(failure) => self.fail(output, "https", failure.describe()),
        }
    }

    fn command_route(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let _ = writeln!(output, "Destination      Gateway          Interface");
        let _ = write!(output, "default          ");
        write_ipv4(output, self.info.internet.gateway_ip);
        let _ = writeln!(output, "       eth0");
        let local = self.info.internet.local_ip;
        let _ = writeln!(
            output,
            "{}.{}.{}.0/24    0.0.0.0          eth0",
            local[0], local[1], local[2]
        );
        let info = crate::ipv6::info();
        if let Some(router) = info.router {
            let _ = write!(output, "default          ");
            write_ipv6(output, router);
            let _ = writeln!(output, "  eth0");
        }
        if let Some(dns) = info.dns {
            let _ = write!(output, "nameserver       ");
            write_ipv6(output, dns);
            let _ = writeln!(output);
        }
        if let Some(global) = info.global {
            let mut prefix = global;
            prefix[8..].fill(0);
            write_ipv6(output, prefix);
            let _ = writeln!(output, "/64    ::               eth0");
        }
    }

    fn command_arp(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let _ = write!(output, "gateway ");
        write_ipv4(output, self.info.internet.gateway_ip);
        let _ = write!(output, " at ");
        write_mac(output, self.info.network.gateway_mac);
        let _ = writeln!(
            output,
            " on eth0 {}",
            if self.info.network.arp_reply {
                "REACHABLE"
            } else {
                "INCOMPLETE"
            }
        );
        if let (Some(router), Some(mac)) =
            (crate::ipv6::info().router, crate::ipv6::info().router_mac)
        {
            let _ = write!(output, "router ");
            write_ipv6(output, router);
            let _ = write!(output, " at ");
            write_mac(output, mac);
            let _ = writeln!(output, " on eth0 REACHABLE");
        }
        crate::ipv6::each_neighbor(|address, mac| {
            let _ = write!(output, "neighbor ");
            write_ipv6(output, *address);
            let _ = write!(output, " at ");
            write_mac(output, mac);
            let _ = writeln!(output, " on eth0 REACHABLE");
        });
    }

    fn command_netstat(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let stats = crate::syscall::stats();
        let _ = writeln!(output, "Protocol  State       Activity");
        let _ = writeln!(
            output,
            "udp       available   datagrams={} bytes={}",
            stats.datagrams, stats.network_bytes
        );
        let _ = writeln!(
            output,
            "icmp      available   echo={}",
            self.info.internet.echo_reply
        );
        crate::udp::each_binding(|port, socket, queued| {
            let _ = writeln!(
                output,
                "udp       {:<11} port={port} queued={queued}",
                if socket { "bound" } else { "client" }
            );
        });
        for index in 0..crate::tcp::SOCKETS {
            let Some((state, port, remote, remote_port)) =
                crate::tcpnet::with_tcp(|tcp, _, _| tcp.summary(index))
            else {
                continue;
            };
            let _ = write!(
                output,
                "{:<9} {:<11} port={port} peer=",
                if crate::ip::is_v4(&remote) {
                    "tcp"
                } else {
                    "tcp6"
                },
                state.name()
            );
            write_ipv6(output, remote);
            let _ = writeln!(output, ":{remote_port}");
        }
    }
}

fn write_ipv4<const N: usize>(output: &mut Text<N>, address: [u8; 4]) {
    let _ = write!(
        output,
        "{}.{}.{}.{}",
        address[0], address[1], address[2], address[3]
    );
}

fn write_mac<const N: usize>(output: &mut Text<N>, address: [u8; 6]) {
    let _ = write!(
        output,
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        address[0], address[1], address[2], address[3], address[4], address[5]
    );
}

fn parse_ipv4(value: &str) -> Option<[u8; 4]> {
    let mut address = [0u8; 4];
    let mut count = 0;
    for component in value.split('.') {
        if count == 4 || component.is_empty() {
            return None;
        }
        let parsed = parse_u64(component)?;
        if parsed > 255 {
            return None;
        }
        address[count] = parsed as u8;
        count += 1;
    }
    (count == 4).then_some(address)
}

/// Feeds one line to the parts of the shell that handle untrusted text, for
/// the fuzzer: the argument parser, pipeline splitter, redirection and path
/// normalizer. Returns a value only so the work cannot be optimized away.
#[cfg(feature = "boot-test")]
pub(crate) fn fuzz_line(line: &str) -> usize {
    let mut total = 0;
    if let Some((stages, count)) = split_pipeline(line) {
        for stage in &stages[..count] {
            if let Ok(mut arguments) = Arguments::parse(stage) {
                let _ = extract_redirection(&mut arguments);
                total += arguments.count;
            }
        }
    }
    if let Ok(mut arguments) = Arguments::parse(line) {
        let _ = extract_redirection(&mut arguments);
        total += arguments.count;
    }
    let mut path: Text<MAX_PATH> = Text::new();
    if normalize_path("/tmp", line, &mut path).is_ok() {
        total += path.as_str().len();
    }
    total
}
/// Runs random command lines through the real shell: read-only commands with
/// arguments from a pool of paths, flags, numbers and junk (some piped or
/// redirected), and the file-modifying ones only on
/// paths under `/tmp`. Commands that reboot, kill, spawn, change accounts or
/// firewall rules, touch the disks or take a long time are left out; the shell
/// runs as root so privileged commands are included. Returns
/// the number of lines executed.
#[cfg(feature = "boot-test")]
pub(crate) fn fuzz_commands(info: SystemInfo<'_>, rounds: u32) -> u32 {
    const READERS: [&str; 41] = [
        "echo", "pwd", "ls", "cat", "head", "tail", "wc", "stat", "readlink", "uname", "whoami",
        "id", "date", "uptime", "ps", "jobs", "grep", "find", "du", "uniq", "sort", "basename",
        "dirname", "seq", "true", "false", "free", "lscpu", "lspci", "lsblk", "mount", "df",
        "which", "env", "history", "ip", "route", "arp", "netstat", "help", "man",
    ];
    const WRITERS: [&str; 8] = ["touch", "mkdir", "rm", "ln", "cp", "mv", "tee", "chmod"];
    const PRIVILEGED_READERS: [&str; 3] = ["dmesg", "strace", "crashes"];
    const PATHS: [&str; 16] = [
        "/tmp/fz1",
        "/tmp/fz2",
        "/tmp/fzdir",
        "/tmp/fzdir/inner",
        "/bin/init",
        "/proc/meminfo",
        "/proc/self/maps",
        "/proc/self/status",
        "/sys/devices/system/cpu/online",
        "/bin",
        "/tmp",
        "..",
        ".",
        "/nonexistent/x",
        "/proc",
        "/sys/kernel",
    ];
    const TEMPORARY: [&str; 5] = [
        "/tmp/fz1",
        "/tmp/fz2",
        "/tmp/fzdir",
        "/tmp/fzdir/inner",
        "/tmp/fz3",
    ];
    const FLAGS: [&str; 10] = [
        "-n", "-l", "-i", "-c", "-r", "-s", "-d", "-name", "-a", "--",
    ];

    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut shell = Shell::new(info, true);
    // Running as root avoids `ear`'s deliberate two-second re-authentication.
    shell.effective_uid = 0;
    let mut executed = 0;
    for _ in 0..rounds {
        let mut line: Text<256> = Text::new();
        let stages = if next() % 5 == 0 { 2 } else { 1 };
        for stage in 0..stages {
            if stage > 0 {
                let _ = line.push_str_checked(" | ");
            }
            let pick = next();
            let writer = pick % 4 == 0;
            let name = if writer {
                WRITERS[(pick >> 8) as usize % WRITERS.len()]
            } else if pick % 23 == 1 {
                PRIVILEGED_READERS[(pick >> 8) as usize % PRIVILEGED_READERS.len()]
            } else {
                READERS[(pick >> 8) as usize % READERS.len()]
            };
            let _ = line.push_str_checked(name);
            for _ in 0..(next() % 4) {
                let token = next();
                let _ = line.push_str_checked(" ");
                if writer {
                    match token % 6 {
                        0 => {
                            let _ = line.push_str_checked(FLAGS[(token >> 8) as usize % 3 + 5]);
                        }
                        1 => {
                            let _ = write!(line, "{:o}", (token >> 8) % 1000);
                        }
                        _ => {
                            let _ = line.push_str_checked(
                                TEMPORARY[(token >> 8) as usize % TEMPORARY.len()],
                            );
                        }
                    }
                    continue;
                }
                match token % 8 {
                    0..=3 => {
                        let _ = line.push_str_checked(PATHS[(token >> 8) as usize % PATHS.len()]);
                    }
                    4 => {
                        let _ = write!(line, "{}", (token >> 8) % 1_000_000);
                    }
                    5 => {
                        let _ = line.push_str_checked(FLAGS[(token >> 8) as usize % FLAGS.len()]);
                    }
                    6 => {
                        for index in 0..1 + (token >> 8) % 12 {
                            let letter = b'a' + ((token >> (16 + index)) % 26) as u8;
                            line.push_byte(letter);
                        }
                    }
                    _ => {
                        for _ in 0..200 {
                            line.push_byte(b'a');
                        }
                    }
                }
            }
        }
        if next() % 10 == 0 {
            let _ = line.push_str_checked(" > /tmp/fz3");
        }
        let mut output = Text::new();
        let _ = shell.execute(line.as_str(), &mut output);
        executed += 1;
    }
    for path in [
        "/tmp/fzdir/inner",
        "/tmp/fzdir",
        "/tmp/fz1",
        "/tmp/fz2",
        "/tmp/fz3",
    ] {
        let _ = vfs::remove(path, path == "/tmp/fzdir" || path == "/tmp/fzdir/inner");
    }
    executed
}

/// An IPv6 address in the usual text form, the longest run of zero groups
/// (two or more) written as `::`.
fn write_ipv6(output: &mut Text<MAX_OUTPUT>, address: [u8; 16]) {
    crate::ip::format(&address, output);
}

fn ipv6_text_valid() -> bool {
    let render = |address: [u8; 16]| {
        let mut text: Text<MAX_OUTPUT> = Text::new();
        write_ipv6(&mut text, address);
        let mut copy: Text<64> = Text::new();
        copy.push_str_checked(text.as_str());
        copy
    };
    let mut link = [0u8; 16];
    link[0] = 0xfe;
    link[1] = 0x80;
    link[15] = 2;
    let mut slaac = [0u8; 16];
    slaac[0] = 0xfe;
    slaac[1] = 0xc0;
    slaac[8..].copy_from_slice(&[0x50, 0x54, 0x00, 0xff, 0xfe, 0x12, 0x34, 0x56]);
    let mut loopback = [0u8; 16];
    loopback[15] = 1;
    let mut isolated = [1u8; 16];
    isolated[2..4].fill(0);
    render(link).as_str() == "fe80::2"
        && render(slaac).as_str() == "fec0::5054:ff:fe12:3456"
        && render(loopback).as_str() == "::1"
        && render([0; 16]).as_str() == "::"
        && render(isolated).as_str() == "101:0:101:101:101:101:101:101"
}

pub(crate) fn normalize_path(
    current: &str,
    input: &str,
    output: &mut Text<MAX_PATH>,
) -> Result<(), &'static str> {
    if input.is_empty() || input.as_bytes().contains(&0) {
        return Err("invalid path");
    }
    let mut combined: Text<MAX_PATH> = Text::new();
    if input.starts_with('/') {
        combined.push_str_checked(input);
    } else {
        combined.push_str_checked(current);
        if current != "/" {
            combined.push_byte(b'/');
        }
        combined.push_str_checked(input);
    }
    if combined.truncated {
        return Err("path too long");
    }
    let mut components = [""; 16];
    let mut count = 0usize;
    for component in combined.as_str().split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            count = count.saturating_sub(1);
            continue;
        }
        if component.len() > 48 || count == components.len() {
            return Err("path component limit exceeded");
        }
        components[count] = component;
        count += 1;
    }
    output.clear();
    output.push_byte(b'/');
    for (index, component) in components[..count].iter().enumerate() {
        if index != 0 {
            output.push_byte(b'/');
        }
        output.push_str_checked(component);
    }
    if output.truncated {
        Err("path too long")
    } else {
        Ok(())
    }
}

fn parse_i64(value: &str) -> Option<i64> {
    match value.strip_prefix('-') {
        Some(digits) => i64::try_from(parse_u64(digits)?).ok().map(|n| -n),
        None => i64::try_from(parse_u64(value)?).ok(),
    }
}

fn parse_u64(value: &str) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    let mut result = 0u64;
    for byte in value.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?.checked_add((byte - b'0') as u64)?;
    }
    Some(result)
}

fn parse_octal(value: &str) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    let mut result = 0u64;
    for byte in value.bytes() {
        if !(b'0'..=b'7').contains(&byte) {
            return None;
        }
        result = result.checked_mul(8)?.checked_add((byte - b'0') as u64)?;
    }
    Some(result)
}

fn parse_line_count(arguments: &Arguments, offset: usize) -> Option<(usize, usize)> {
    if arguments.get(offset) != Some("-n") {
        return Some((10, offset));
    }
    let count = arguments.get(offset + 1).and_then(parse_u64)?;
    if count > 1000 {
        return None;
    }
    Some((count as usize, offset + 2))
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(crate) fn read_file(path: &str, destination: &mut [u8]) -> Result<usize, vfs::VfsError> {
    let descriptor = vfs::open_file(path, false, false, false, 0, false)?;
    let mut total = 0;
    let result = loop {
        if total == destination.len() {
            let metadata = match vfs::descriptor_metadata(descriptor) {
                Ok(metadata) => metadata,
                Err(failure) => break Err(failure),
            };
            if metadata.size as usize > destination.len() {
                break Err(vfs::VfsError::FileTooLarge);
            }
            break Ok(total);
        }
        match vfs::read(descriptor, &mut destination[total..]) {
            Ok(0) => break Ok(total),
            Ok(bytes) => total += bytes,
            Err(failure) => break Err(failure),
        }
    };
    let close = vfs::close(descriptor);
    result.and(close.map(|_| total))
}

pub(crate) fn textual(data: &[u8]) -> bool {
    data.iter().all(|byte| {
        *byte == b'\n' || *byte == b'\r' || *byte == b'\t' || (0x20..=0x7e).contains(byte)
    })
}

fn write_file_bytes(output: &mut Text<MAX_OUTPUT>, data: &[u8]) {
    for byte in data {
        match *byte {
            b'\r' => {}
            b'\t' => {
                let _ = output.push_str_checked("    ");
            }
            byte => {
                output.push_byte(byte);
            }
        }
    }
}

fn prefix_lines(data: &[u8], lines: usize) -> usize {
    if lines == 0 {
        return 0;
    }
    let mut seen = 0;
    for (index, byte) in data.iter().enumerate() {
        if *byte == b'\n' {
            seen += 1;
            if seen == lines {
                return index + 1;
            }
        }
    }
    data.len()
}

fn suffix_lines(data: &[u8], lines: usize) -> usize {
    if lines == 0 {
        return data.len();
    }
    let mut seen = 0;
    let skip_final = data.last() == Some(&b'\n');
    for (index, byte) in data.iter().enumerate().rev() {
        if *byte == b'\n' && !(skip_final && index + 1 == data.len()) {
            seen += 1;
            if seen == lines {
                return index + 1;
            }
        }
    }
    0
}

/// Whether `task_id` is still a live, non-exited scheduler task -
/// `scheduler::list_tasks` already excludes `Exited`/`Empty` slots, so
/// simple membership is exactly the non-blocking check `jobs` needs.
fn job_is_running(task_id: u64) -> bool {
    let mut found = false;
    scheduler::list_tasks(|summary| {
        if summary.id == task_id {
            found = true;
        }
    });
    found
}

const MAX_WALK_DEPTH: usize = 6;

/// Visits every entry below `path` (depth-first, bounded depth) with its full
/// path, whether it is a directory, and its size. Symlinks are reported but
/// not followed.
fn walk_tree(path: &str, depth: usize, visit: &mut dyn FnMut(&str, bool, u64)) {
    let Ok(descriptor) = vfs::open_directory(path) else {
        return;
    };
    while let Ok(Some(entry)) = vfs::next_directory_entry(descriptor) {
        let name = core::str::from_utf8(&entry.name[..entry.name_len as usize]).unwrap_or("?");
        if name == "." || name == ".." {
            continue;
        }
        let mut child: Text<MAX_PATH> = Text::new();
        let _ = child.push_str_checked(path);
        if path != "/" {
            child.push_byte(b'/');
        }
        child.push_str_checked(name);
        let is_directory = entry.kind == 4;
        let size = if is_directory {
            0
        } else {
            vfs::symlink_metadata(child.as_str()).map_or(0, |metadata| metadata.size)
        };
        visit(child.as_str(), is_directory, size);
        if is_directory && depth < MAX_WALK_DEPTH {
            walk_tree(child.as_str(), depth + 1, visit);
        }
    }
    let _ = vfs::close(descriptor);
}

fn contains_bytes(haystack: &[u8], needle: &[u8], ignore_case: bool) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.windows(needle.len()).any(|window| {
        window.iter().zip(needle).all(|(a, b)| {
            if ignore_case {
                a.eq_ignore_ascii_case(b)
            } else {
                a == b
            }
        })
    })
}

/// Appends the crash count and every recorded crash to `output`.
fn write_crash_records(output: &mut Text<MAX_OUTPUT>) {
    let _ = writeln!(output, "crashes since boot: {}", crate::crash::total());
    crate::crash::for_each(|record| {
        let _ = writeln!(
            output,
            "pid={} vector={} error={:#x} address={:#x} rip={:#x} at={}ms",
            record.pid, record.vector, record.error, record.address, record.rip, record.uptime_ms
        );
    });
}
/// Appends the last wanted lines of the kernel log to output.
fn append_log_tail(output: &mut Text<MAX_OUTPUT>, wanted: usize) {
    let mut buffer = [0u8; 4096];
    let length = crate::serial::log_tail(&mut buffer);
    let log = &buffer[..length];
    let log = log.strip_suffix(b"\n").unwrap_or(log);
    let mut start = 0;
    let mut seen = 0;
    for (index, byte) in log.iter().enumerate().rev() {
        if *byte == b'\n' {
            seen += 1;
            if seen == wanted {
                start = index + 1;
                break;
            }
        }
    }
    for line in log[start..].split(|byte| *byte == b'\n') {
        if let Ok(text) = core::str::from_utf8(line) {
            let _ = writeln!(output, "{text}");
        }
    }
}

/// Looks a service up and, if its task has exited, reaps it and records the
/// exit code so the table never reports a dead task as running.
fn svc_refresh(name: &str) -> Option<crate::services::Service> {
    let service = crate::services::get(name)?;
    if let Some(task_id) = service.task_id
        && !job_is_running(task_id)
    {
        let exit_code = scheduler::wait_for_child(task_id).unwrap_or(0);
        let _ = crate::services::update(name, |entry| {
            entry.task_id = None;
            entry.last_exit = Some(exit_code);
        });
        return crate::services::get(name);
    }
    Some(service)
}

fn service_error(failure: crate::services::ServiceError) -> &'static str {
    use crate::services::ServiceError;
    match failure {
        ServiceError::InvalidName => "invalid service name",
        ServiceError::PathTooLong => "path too long",
        ServiceError::Exists => "service already exists",
        ServiceError::TableFull => "service table full",
        ServiceError::Unknown => "no such service",
    }
}

pub(crate) fn vfs_error(failure: vfs::VfsError) -> &'static str {
    match failure {
        vfs::VfsError::InvalidPath => "invalid path",
        vfs::VfsError::Traversal => "path traversal rejected",
        vfs::VfsError::NameTooLong => "name too long",
        vfs::VfsError::DepthExceeded => "path depth exceeded",
        vfs::VfsError::NotFound => "not found",
        vfs::VfsError::NotDirectory => "not a directory",
        vfs::VfsError::IsDirectory => "is a directory",
        vfs::VfsError::Exists => "already exists",
        vfs::VfsError::PermissionDenied => "permission denied",
        vfs::VfsError::NodeLimit => "filesystem node limit reached",
        vfs::VfsError::HandleLimit => "open-handle limit reached",
        vfs::VfsError::BadDescriptor => "bad descriptor",
        vfs::VfsError::OffsetOverflow => "file offset overflow",
        vfs::VfsError::FileTooLarge => "file exceeds the 4096-byte tmpfs limit",
        vfs::VfsError::NotEmpty => "directory not empty",
        vfs::VfsError::Busy => "resource busy",
        vfs::VfsError::PersistNameUnsupported => {
            "name must be short and uppercase to persist under /data (e.g. NAME.TXT)"
        }
        vfs::VfsError::TooManyLinks => "too many levels of symbolic links",
    }
}

pub fn self_test(info: SystemInfo<'_>) -> ShellReport {
    let unique = COMMANDS.iter().enumerate().all(|(left, command)| {
        !command.name.is_empty()
            && command.usage.starts_with(command.name)
            && command.name.bytes().all(|byte| byte.is_ascii_lowercase())
            && COMMANDS[..left]
                .iter()
                .all(|candidate| candidate.name != command.name)
    });
    let parser =
        Arguments::parse("echo \"Aer OS\" 'ready now' path\\ value").is_ok_and(|arguments| {
            arguments.count == 4
                && arguments.get(0) == Some("echo")
                && arguments.get(1) == Some("Aer OS")
                && arguments.get(2) == Some("ready now")
                && arguments.get(3) == Some("path value")
        }) && ipv6_text_valid();
    let mut denied = Shell::new(info, false);
    let mut output = Text::new();
    denied.execute("ear whoami", &mut output);
    let denied_valid = denied.last_status == 126
        && denied.denied_commands == 1
        && output.as_str().contains("no elevation capability");
    let mut allowed = Shell::new(info, true);
    allowed.execute("ear whoami", &mut output);
    let mut saw_denied_audit = false;
    let mut saw_granted_audit = false;
    crate::audit::recent(|_sequence, line| {
        if line.contains("PRIVILEGE") && line.contains("ear denied reason=no_capability") {
            saw_denied_audit = true;
        }
        if line.contains("PRIVILEGE") && line.contains("ear granted command=whoami") {
            saw_granted_audit = true;
        }
    });
    let privilege = denied_valid
        && allowed.last_status == 0
        && allowed.effective_uid == allowed.uid
        && allowed.elevated_commands == 1
        && output.as_str() == "root\n"
        && saw_denied_audit
        && saw_granted_audit;

    // A shell that opts into `require_reauth` (the real interactive
    // terminal does; `allowed`/`denied` above deliberately don't, matching
    // every other caller unaffected by this) must not elevate on
    // `may_elevate` alone - it needs `note_authenticated` first, stays
    // elevatable for the reauth window afterward, and asks again once
    // that window is made to lapse.
    let mut reauth_shell = Shell::new(info, true);
    reauth_shell.require_reauth();
    let control = reauth_shell.execute("ear whoami", &mut output);
    let first_attempt_blocked = control == Control::NeedsPassword
        && reauth_shell.last_status == 1
        && reauth_shell.effective_uid == reauth_shell.uid
        && reauth_shell.elevated_commands == 0;
    reauth_shell.note_authenticated();
    let control = reauth_shell.execute("ear whoami", &mut output);
    let authenticated_runs = control == Control::None
        && reauth_shell.last_status == 0
        && reauth_shell.elevated_commands == 1
        && output.as_str() == "root\n";
    let control = reauth_shell.execute("ear whoami", &mut output);
    let stays_authenticated = control == Control::None
        && reauth_shell.last_status == 0
        && reauth_shell.elevated_commands == 2;
    reauth_shell.reauth_deadline_ns = 0;
    let control = reauth_shell.execute("ear whoami", &mut output);
    let reprompts_after_expiry =
        control == Control::NeedsPassword && reauth_shell.elevated_commands == 2;
    let reauth = first_attempt_blocked
        && authenticated_runs
        && stays_authenticated
        && reprompts_after_expiry;

    allowed.execute("ls /", &mut output);
    let listing = allowed.last_status == 0
        && output.as_str().contains("bin/")
        && output.as_str().contains("tmp/");
    allowed.execute("ps", &mut output);
    let ps_ok = allowed.last_status == 0
        && output.as_str().contains("PID  PPID PGID SID  STATE")
        && output.as_str().contains("spawned=");
    // `ear chmod` below now writes a real, persistent audit record (see
    // `execute_as_root`) - a genuine, expected side effect that grows
    // `/data/AUDIT.LOG`, not a leak. Measured separately so the leak check
    // below stays honest instead of just being loosened.
    let audit_log_size = || {
        vfs::metadata("/data/AUDIT.LOG")
            .map(|metadata| metadata.size)
            .unwrap_or(0)
    };
    let audit_before = audit_log_size();
    let before = vfs::stats();
    let operations = [
        "mkdir /tmp/.aersh-test",
        "touch /tmp/.aersh-test/empty",
        "cp /etc/aeros-release /tmp/.aersh-test/copy",
        "mv /tmp/.aersh-test/copy /tmp/.aersh-test/release",
        "ear chmod 600 /tmp/.aersh-test/release",
        "cat /tmp/.aersh-test/release",
        "rm /tmp/.aersh-test/empty",
        "rm /tmp/.aersh-test/release",
        "rm -d /tmp/.aersh-test",
    ];
    let mut mutation = true;
    for operation in operations {
        allowed.execute(operation, &mut output);
        mutation &= allowed.last_status == 0;
    }
    let after = vfs::stats();
    let audit_growth = audit_log_size().saturating_sub(audit_before) as usize;
    let filesystem = listing
        && mutation
        && before.nodes == after.nodes
        && before.directories == after.directories
        && before.files == after.files
        && before.bytes + audit_growth == after.bytes
        && before.mutable_files == after.mutable_files
        && before.mutable_bytes + audit_growth == after.mutable_bytes
        && before.open_handles == after.open_handles
        && ps_ok;

    // `>`/`>>` output redirection: real shell semantics mean `output` is
    // empty on success (the text went to the file, not the terminal), `>`
    // truncates, and `>>` appends.
    let redirect_path = "/tmp/.aersh-test-redirect";
    allowed.execute(
        "echo AEROS-REDIRECT-TEST > /tmp/.aersh-test-redirect",
        &mut output,
    );
    let redirect_write_ok = allowed.last_status == 0 && output.as_str().is_empty();
    allowed.execute("cat /tmp/.aersh-test-redirect", &mut output);
    let redirect_content_ok = output.as_str() == "AEROS-REDIRECT-TEST\n";
    allowed.execute(
        "echo AEROS-REDIRECT-APPEND >> /tmp/.aersh-test-redirect",
        &mut output,
    );
    let append_write_ok = allowed.last_status == 0 && output.as_str().is_empty();
    allowed.execute("cat /tmp/.aersh-test-redirect", &mut output);
    let append_content_ok = output.as_str() == "AEROS-REDIRECT-TEST\nAEROS-REDIRECT-APPEND\n";
    allowed.execute("rm /tmp/.aersh-test-redirect", &mut output);
    let redirect_cleanup_ok = allowed.last_status == 0 && vfs::metadata(redirect_path).is_err();
    let redirection = redirect_write_ok
        && redirect_content_ok
        && append_write_ok
        && append_content_ok
        && redirect_cleanup_ok;

    // `.aershrc` startup script: write one to `/home`, run it through a
    // fresh shell, and confirm both its accumulated output and its real
    // filesystem side effect (an exported env var wouldn't outlive the
    // shell, so a written file is the only way to observe it from outside).
    let startup = {
        let script = b"# aershrc self-test\necho AEROS-STARTUP-RAN\ntouch /tmp/.aershrc-ran\n";
        let written = vfs::open_file(STARTUP_SCRIPT_PATH, true, false, true, 0o644, true)
            .and_then(|descriptor| {
                let result = vfs::write(descriptor, script, false);
                let _ = vfs::close(descriptor);
                result
            })
            .is_ok_and(|count| count == script.len());
        let mut startup_shell = Shell::new(info, true);
        let mut startup_output: Text<MAX_OUTPUT> = Text::new();
        startup_shell.run_startup_script(&mut startup_output);
        let ran = written
            && startup_output.as_str().contains("AEROS-STARTUP-RAN")
            && vfs::metadata("/tmp/.aershrc-ran").is_ok();
        let _ = vfs::remove("/tmp/.aershrc-ran", false);
        let _ = vfs::remove(STARTUP_SCRIPT_PATH, false);
        let absent_is_noop = {
            let mut noop_shell = Shell::new(info, true);
            let mut noop_output: Text<MAX_OUTPUT> = Text::new();
            noop_shell.run_startup_script(&mut noop_output);
            noop_output.as_str().is_empty()
        };
        ran && absent_is_noop
    };

    // Symlinks: absolute-target follow-through-open, relative-target
    // follow-through-open (joined against the link's own directory, not the
    // shell's cwd), `readlink` returning the raw target unfollowed, `ls`
    // rendering `name -> target`, cycle detection, and the /data refusal.
    let symlinks = {
        allowed.execute("mkdir /tmp/.symlink-test", &mut output);
        allowed.execute("touch /tmp/.symlink-test/real", &mut output);
        allowed.execute("echo symlinked > /tmp/.symlink-test/real", &mut output);
        allowed.execute(
            "ln -s /tmp/.symlink-test/real /tmp/.symlink-test/abs-link",
            &mut output,
        );
        let absolute_follow_ok = allowed.last_status == 0;
        allowed.execute("cat /tmp/.symlink-test/abs-link", &mut output);
        let absolute_read_ok = output.as_str() == "symlinked\n";
        allowed.execute("ln -s real /tmp/.symlink-test/rel-link", &mut output);
        allowed.execute("cat /tmp/.symlink-test/rel-link", &mut output);
        let relative_read_ok = output.as_str() == "symlinked\n";
        allowed.execute("readlink /tmp/.symlink-test/abs-link", &mut output);
        let readlink_ok = output.as_str() == "/tmp/.symlink-test/real\n";
        allowed.execute("ls /tmp/.symlink-test", &mut output);
        let ls_ok = output
            .as_str()
            .contains("abs-link -> /tmp/.symlink-test/real");
        allowed.execute(
            "ln -s /tmp/.symlink-test/loop-b /tmp/.symlink-test/loop-a",
            &mut output,
        );
        allowed.execute(
            "ln -s /tmp/.symlink-test/loop-a /tmp/.symlink-test/loop-b",
            &mut output,
        );
        allowed.execute("cat /tmp/.symlink-test/loop-a", &mut output);
        let cycle_rejected = allowed.last_status != 0
            && output
                .as_str()
                .contains("too many levels of symbolic links");
        allowed.execute("ln -s /nowhere /data/nope", &mut output);
        let data_refused = allowed.last_status != 0;
        allowed.execute("rm /tmp/.symlink-test/loop-a", &mut output);
        allowed.execute("rm /tmp/.symlink-test/loop-b", &mut output);
        allowed.execute("rm /tmp/.symlink-test/rel-link", &mut output);
        allowed.execute("rm /tmp/.symlink-test/abs-link", &mut output);
        allowed.execute("rm /tmp/.symlink-test/real", &mut output);
        allowed.execute("rm -d /tmp/.symlink-test", &mut output);
        let cleanup_ok = allowed.last_status == 0 && vfs::metadata("/tmp/.symlink-test").is_err();
        absolute_follow_ok
            && absolute_read_ok
            && relative_read_ok
            && readlink_ok
            && ls_ok
            && cycle_rejected
            && data_refused
            && cleanup_ok
    };

    // Background jobs (`run ... &`, `jobs`, `wait`): launching backgrounds
    // instead of blocking (proven structurally - `command_run` never calls
    // `wait_for_child` on the backgrounding path at all), `wait <task_id>`
    // blocks until the real exit code (73, `/bin/init`'s known exit) comes
    // back, and the job is gone from `jobs` afterward (both `wait` and
    // `jobs` itself remove a job once its exit has been reported once).
    let background_jobs = {
        allowed.execute("run /bin/init &", &mut output);
        let launch_ok = allowed.last_status == 0 && output.as_str().starts_with('[');
        let task_id = output
            .as_str()
            .trim()
            .rsplit(' ')
            .next()
            .and_then(|token| token.parse::<u64>().ok());
        let wait_ok = if let Some(task_id) = task_id {
            let mut wait_command: Text<MAX_INPUT> = Text::new();
            let _ = write!(wait_command, "wait {task_id}");
            allowed.execute(wait_command.as_str(), &mut output);
            allowed.last_status == 0 && output.as_str().contains("exited with 73")
        } else {
            false
        };
        allowed.execute("jobs", &mut output);
        let jobs_after_ok = output.as_str().is_empty();
        launch_ok && wait_ok && jobs_after_ok
    };

    // `fw` is privileged (`ear`-gated, like `kill`/`chmod`), so every call
    // here goes through `ear` the same way those self-tests already do.
    let firewall_command = {
        crate::firewall::clear_rules();
        allowed.execute("ear fw", &mut output);
        let empty_ok = allowed.last_status == 0 && output.as_str().starts_with("rules=0 dropped=");
        allowed.execute("ear fw deny tcp 12345", &mut output);
        let deny_ok = allowed.last_status == 0;
        allowed.execute("ear fw", &mut output);
        let listed_ok = output.as_str().starts_with("rules=1 dropped=");
        allowed.execute("ear fw clear", &mut output);
        allowed.execute("ear fw", &mut output);
        let cleared_ok = output.as_str().starts_with("rules=0 dropped=");
        empty_ok && deny_ok && listed_ok && cleared_ok
    };

    let dmesg_command = {
        allowed.execute("dmesg", &mut output);
        let denied_ok = allowed.last_status != 0;
        allowed.execute("ear dmesg 5", &mut output);
        let shown_ok = allowed.last_status == 0
            && !output.as_str().is_empty()
            && output.as_str().lines().count() <= 5;
        allowed.execute("ear dmesg 100", &mut output);
        let tagged_ok = output.as_str().contains("AEROS_");
        allowed.execute("ear dmesg 0", &mut output);
        let bad_count_ok = allowed.last_status != 0;
        denied_ok && shown_ok && tagged_ok && bad_count_ok
    };

    let pipelines = {
        allowed.execute("echo pear > /tmp/.pp", &mut output);
        allowed.execute("echo apple >> /tmp/.pp", &mut output);
        allowed.execute("echo pear >> /tmp/.pp", &mut output);
        allowed.execute("echo fig >> /tmp/.pp", &mut output);
        allowed.execute("cat /tmp/.pp | sort", &mut output);
        let sort_ok = output.as_str() == "apple\nfig\npear\npear\n";
        allowed.execute("cat /tmp/.pp | sort | uniq", &mut output);
        let uniq_ok = output.as_str() == "apple\nfig\npear\n";
        allowed.execute("cat /tmp/.pp | sort | uniq -c", &mut output);
        let counted_ok = output.as_str() == "   1 apple\n   1 fig\n   2 pear\n";
        allowed.execute("cat /tmp/.pp | grep pear | wc", &mut output);
        let counts_ok = output.as_str().trim() == "2      2     10";
        allowed.execute("echo 'a|b' | cat", &mut output);
        let quoted_ok = output.as_str() == "a|b\n";
        allowed.execute("cat /tmp/.pp | tee /tmp/.pq | wc", &mut output);
        let tee_wc_ok = output.as_str().trim() == "4      4     20";
        allowed.execute("cat /tmp/.pq", &mut output);
        let tee_file_ok = output.as_str() == "pear\napple\npear\nfig\n";
        allowed.execute("cat /tmp/.pp |", &mut output);
        let trailing_ok = allowed.last_status == 2 && output.as_str().contains("invalid pipeline");
        allowed.execute("| cat", &mut output);
        let leading_ok = allowed.last_status == 2;
        allowed.execute("cat /tmp/.pp | cat | cat | cat | cat", &mut output);
        let too_long_ok = allowed.last_status == 2;
        allowed.execute("false | true", &mut output);
        let last_status_ok = allowed.last_status == 0;
        allowed.execute("true | false", &mut output);
        let failing_last_ok = allowed.last_status != 0;
        allowed.execute("cat", &mut output);
        let no_input_ok = allowed.last_status != 0;
        allowed.execute("rm /tmp/.pp", &mut output);
        allowed.execute("rm /tmp/.pq", &mut output);
        let checks = [
            sort_ok,
            uniq_ok,
            counted_ok,
            counts_ok,
            quoted_ok,
            tee_wc_ok,
            tee_file_ok,
            trailing_ok,
            leading_ok,
            too_long_ok,
            last_status_ok,
            failing_last_ok,
            no_input_ok,
        ];
        if checks.iter().any(|ok| !ok) {
            crate::serial::format(format_args!("AEROS_PIPELINE_CHECKS {checks:?}\n"));
        }
        checks.iter().all(|ok| *ok)
    };

    let sigcheck_command = {
        let key_hex = "5fcb5f6bd3f7f5e073b5df5cbc90fad2214d7b73749fce0929d4343d41407077";
        let signature = crate::ed25519::vector(1).map(|(_, signature)| signature);
        let wrote = signature.is_some_and(|signature| {
            let write_file = |path: &str, data: &[u8]| {
                vfs::open_file(path, true, false, true, 0o644, true)
                    .and_then(|handle| {
                        let result = vfs::write(handle, data, false);
                        let _ = vfs::close(handle);
                        result
                    })
                    .is_ok_and(|count| count == data.len())
            };
            write_file("/tmp/.sg-m", b"abc") && write_file("/tmp/.sg-s", &signature)
        });
        let mut command: Text<MAX_INPUT> = Text::new();
        let _ = write!(command, "sigcheck /tmp/.sg-m /tmp/.sg-s {key_hex}");
        allowed.execute(command.as_str(), &mut output);
        let good_ok = allowed.last_status == 0 && output.as_str() == "OK\n";
        allowed.execute("echo abd > /tmp/.sg-m", &mut output);
        allowed.execute(command.as_str(), &mut output);
        let tampered_ok = allowed.last_status != 0 && output.as_str() == "BAD\n";
        allowed.execute("sigcheck /tmp/.sg-m /tmp/.sg-s 1234", &mut output);
        let bad_key_ok = allowed.last_status != 0;
        allowed.execute("rm /tmp/.sg-m", &mut output);
        allowed.execute("rm /tmp/.sg-s", &mut output);
        wrote && good_ok && tampered_ok && bad_key_ok && crate::ed25519::self_test()
    };

    let bench_command = {
        allowed.execute("bench", &mut output);
        allowed.last_status == 0
            && crate::bench::LABELS.iter().all(|label| {
                output.as_str().lines().any(|line| {
                    line.starts_with(label)
                        && line
                            .split_whitespace()
                            .nth(1)
                            .and_then(|value| value.parse::<u64>().ok())
                            .is_some_and(|value| value > 0)
                })
            })
    };

    let fsck_command = {
        allowed.execute("fsck", &mut output);
        let denied_ok = allowed.last_status != 0;
        allowed.execute("ear fsck", &mut output);
        let clean_ok = allowed.last_status == 0
            && output.as_str().starts_with("files=")
            && output
                .as_str()
                .contains("broken_chains=0 cross_linked=0 size_mismatches=0");
        allowed.execute("ear fsck --repair /home", &mut output);
        let repair_ok = allowed.last_status == 0 && output.as_str().contains("(repaired)");
        allowed.execute("ear fsck /tmp", &mut output);
        let bad_target_ok = allowed.last_status != 0;
        denied_ok && clean_ok && repair_ok && bad_target_ok
    };

    let strace_command = {
        allowed.execute("strace", &mut output);
        let denied_ok = allowed.last_status != 0;
        allowed.execute("ear strace 3", &mut output);
        let listed_ok = allowed.last_status == 0
            && output.as_str().starts_with("syscalls since boot: ")
            && output.as_str().lines().count() == 4
            && output.as_str().contains("pid=");
        allowed.execute("ear strace 99", &mut output);
        let range_ok = allowed.last_status != 0;
        denied_ok && listed_ok && range_ok && crate::trace::self_test()
    };

    let crashes_command = {
        allowed.execute("crashes", &mut output);
        let denied_ok = allowed.last_status != 0;
        allowed.execute("ear crashes", &mut output);
        let listed_ok = allowed.last_status == 0
            && output.as_str().starts_with("crashes since boot: ")
            && output.as_str().contains("vector=6 ");
        allowed.execute("ear crashes save /tmp/.cr", &mut output);
        let saved_ok = allowed.last_status == 0 && output.as_str().starts_with("saved ");
        allowed.execute("cat /tmp/.cr | grep vector=6", &mut output);
        let record_saved_ok = output.as_str().contains("vector=6 ");
        allowed.execute("cat /tmp/.cr | grep kernel", &mut output);
        let log_saved_ok = output.as_str().contains("kernel log");
        allowed.execute("rm /tmp/.cr", &mut output);
        denied_ok
            && listed_ok
            && saved_ok
            && record_saved_ok
            && log_saved_ok
            && crate::crash::self_test()
    };

    let priority_command = {
        allowed.execute("renice 5 1", &mut output);
        let denied_ok = allowed.last_status != 0;
        allowed.execute("run /bin/init &", &mut output);
        let task_id = output
            .as_str()
            .trim()
            .rsplit(' ')
            .next()
            .and_then(|token| token.parse::<u64>().ok());
        let changed_ok = task_id.is_some_and(|task_id| {
            let mut command: Text<MAX_INPUT> = Text::new();
            let _ = write!(command, "ear renice -7 {task_id}");
            allowed.execute(command.as_str(), &mut output);
            allowed.last_status == 0 && scheduler::nice_of(task_id) == Some(-7)
        });
        if let Some(task_id) = task_id {
            let mut wait_command: Text<MAX_INPUT> = Text::new();
            let _ = write!(wait_command, "wait {task_id}");
            allowed.execute(wait_command.as_str(), &mut output);
        }
        allowed.execute("ear renice 40 1", &mut output);
        let range_ok = allowed.last_status != 0;
        allowed.execute("ear renice 5 999999", &mut output);
        let missing_ok = allowed.last_status != 0;
        denied_ok && changed_ok && range_ok && missing_ok && scheduler::priority_self_test()
    };

    let text_tools = {
        allowed.execute("echo banana > /tmp/.tt", &mut output);
        allowed.execute("echo apple >> /tmp/.tt", &mut output);
        allowed.execute("echo Cherry >> /tmp/.tt", &mut output);
        allowed.execute("grep an /tmp/.tt", &mut output);
        let grep_ok = allowed.last_status == 0 && output.as_str() == "banana\n";
        allowed.execute("grep -c -i CHERRY /tmp/.tt", &mut output);
        let count_ok = output.as_str() == "1\n";
        allowed.execute("grep -n apple /tmp/.tt", &mut output);
        let numbered_ok = output.as_str() == "2:apple\n";
        allowed.execute("grep zzz /tmp/.tt", &mut output);
        let no_match_ok = allowed.last_status != 0;
        allowed.execute("sort /tmp/.tt", &mut output);
        let sort_ok = output.as_str() == "Cherry\napple\nbanana\n";
        allowed.execute("sort -r /tmp/.tt", &mut output);
        let reverse_ok = output.as_str() == "banana\napple\nCherry\n";
        allowed.execute("find /tmp -name .tt", &mut output);
        let find_ok = output.as_str() == "/tmp/.tt\n";
        allowed.execute("du /tmp/.tt", &mut output);
        let du_ok = output.as_str() == "20 /tmp/.tt\n";
        allowed.execute("basename /a/b/c.txt", &mut output);
        let basename_ok = output.as_str() == "c.txt\n";
        allowed.execute("dirname /a/b/c.txt", &mut output);
        let dirname_ok = output.as_str() == "/a/b\n";
        allowed.execute("seq 2 4", &mut output);
        let seq_ok = output.as_str() == "2\n3\n4\n";
        allowed.execute("false", &mut output);
        let false_ok = allowed.last_status != 0;
        allowed.execute("true", &mut output);
        let true_ok = allowed.last_status == 0;
        allowed.execute("rm /tmp/.tt", &mut output);
        grep_ok
            && count_ok
            && numbered_ok
            && no_match_ok
            && sort_ok
            && reverse_ok
            && find_ok
            && du_ok
            && basename_ok
            && dirname_ok
            && seq_ok
            && false_ok
            && true_ok
    };

    let service_command = {
        allowed.execute("svc", &mut output);
        let denied_ok = allowed.last_status != 0;
        allowed.execute("ear svc add probe /bin/init", &mut output);
        let add_ok = allowed.last_status == 0;
        allowed.execute("ear svc add probe /bin/init", &mut output);
        let duplicate_ok = allowed.last_status != 0;
        allowed.execute("ear svc start probe", &mut output);
        let start_ok = allowed.last_status == 0 && output.as_str().starts_with("started probe");
        allowed.execute("ear svc stop probe", &mut output);
        let stop_ok = allowed.last_status == 0;
        allowed.execute("ear svc", &mut output);
        let listed_ok = output.as_str().contains("probe") && output.as_str().contains("stopped");
        allowed.execute("ear svc start missing", &mut output);
        let missing_ok = allowed.last_status != 0;
        allowed.execute("ear svc rm probe", &mut output);
        let removed_ok = allowed.last_status == 0;
        allowed.execute("ear svc", &mut output);
        let empty_ok = !output.as_str().contains("probe");
        denied_ok
            && add_ok
            && duplicate_ok
            && start_ok
            && stop_ok
            && listed_ok
            && missing_ok
            && removed_ok
            && empty_ok
    };

    ShellReport {
        commands: COMMANDS.len(),
        unique,
        parser,
        privilege,
        filesystem,
        reauth,
        redirection,
        startup,
        symlinks,
        background_jobs,
        firewall_command,
        dmesg_command,
        service_command,
        text_tools,
        priority_command,
        crashes_command,
        bench_command,
        sigcheck_command,
        pipelines,
        strace_command,
        fsck_command,
        verified: COMMANDS.len() == 87
            && unique
            && parser
            && privilege
            && filesystem
            && reauth
            && redirection
            && startup
            && symlinks
            && background_jobs
            && firewall_command
            && dmesg_command
            && service_command
            && text_tools
            && priority_command
            && crashes_command
            && bench_command
            && sigcheck_command
            && pipelines
            && strace_command
            && fsck_command,
    }
}

pub(crate) fn scancode_character(code: u8, shift: bool, caps: bool) -> Option<u8> {
    crate::keymap::ascii(code, shift, caps)
}

/// The original US-only mapping (kept for reference/tests of the shell).
#[allow(dead_code)]
fn scancode_character_us(code: u8, shift: bool, caps: bool) -> Option<u8> {
    let base = match code {
        0x02 => b'1',
        0x03 => b'2',
        0x04 => b'3',
        0x05 => b'4',
        0x06 => b'5',
        0x07 => b'6',
        0x08 => b'7',
        0x09 => b'8',
        0x0a => b'9',
        0x0b => b'0',
        0x0c => b'-',
        0x0d => b'=',
        0x10 => b'q',
        0x11 => b'w',
        0x12 => b'e',
        0x13 => b'r',
        0x14 => b't',
        0x15 => b'y',
        0x16 => b'u',
        0x17 => b'i',
        0x18 => b'o',
        0x19 => b'p',
        0x1a => b'[',
        0x1b => b']',
        0x1e => b'a',
        0x1f => b's',
        0x20 => b'd',
        0x21 => b'f',
        0x22 => b'g',
        0x23 => b'h',
        0x24 => b'j',
        0x25 => b'k',
        0x26 => b'l',
        0x27 => b';',
        0x28 => b'\'',
        0x29 => b'`',
        0x2b => b'\\',
        0x2c => b'z',
        0x2d => b'x',
        0x2e => b'c',
        0x2f => b'v',
        0x30 => b'b',
        0x31 => b'n',
        0x32 => b'm',
        0x33 => b',',
        0x34 => b'.',
        0x35 => b'/',
        0x39 => b' ',
        _ => return None,
    };
    if base.is_ascii_alphabetic() {
        return Some(if shift ^ caps {
            base.to_ascii_uppercase()
        } else {
            base
        });
    }
    if !shift {
        return Some(base);
    }
    Some(match base {
        b'1' => b'!',
        b'2' => b'@',
        b'3' => b'#',
        b'4' => b'$',
        b'5' => b'%',
        b'6' => b'^',
        b'7' => b'&',
        b'8' => b'*',
        b'9' => b'(',
        b'0' => b')',
        b'-' => b'_',
        b'=' => b'+',
        b'[' => b'{',
        b']' => b'}',
        b';' => b':',
        b'\'' => b'"',
        b'`' => b'~',
        b'\\' => b'|',
        b',' => b'<',
        b'.' => b'>',
        b'/' => b'?',
        other => other,
    })
}

fn find_command(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|command| command.name == name)
}
