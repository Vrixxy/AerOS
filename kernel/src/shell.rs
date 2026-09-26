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
    Route,
    Arp,
    Netstat,
    Reboot,
    Shutdown,
    Ear,
    Run,
    Av,
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

static COMMANDS: [CommandSpec; 53] = [
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
        "show scheduled kernel tasks",
        false,
        Handler::Jobs,
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
        "mount",
        "list mounted filesystems",
        false,
        Handler::Mount,
    ),
    command(
        "umount",
        "umount <path>",
        "unmount a filesystem",
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
        "av <scan [--clean] [path]|status|log|realtime [on|off]|hash <file>|quarantine|restore <id>|delete <id>|test>",
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

    pub(crate) fn backspace(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        self.len -= 1;
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
    elevated_commands: u64,
    denied_commands: u64,
    executed_commands: u64,
    last_status: u8,
    expansion_status: u8,
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
            elevated_commands: 0,
            denied_commands: 0,
            executed_commands: 0,
            last_status: 0,
            expansion_status: 0,
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
        let arguments = match Arguments::parse(line) {
            Ok(arguments) => arguments,
            Err(failure) => {
                let _ = writeln!(output, "aersh: {failure}");
                self.last_status = 2;
                return Control::None;
            }
        };
        let control = self.dispatch(&arguments, 0, output);
        if output.truncated {
            let _ = output.push_str_checked("\n[output truncated]\n");
        }
        control
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
        if !self.may_elevate {
            let _ = writeln!(output, "ear: this session has no elevation capability");
            self.denied_commands = self.denied_commands.saturating_add(1);
            self.last_status = 126;
            return Control::None;
        }
        let previous = self.effective_uid;
        self.effective_uid = 0;
        self.elevated_commands = self.elevated_commands.saturating_add(1);
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
                    if long {
                        let mut child: Text<MAX_PATH> = Text::new();
                        let _ = child.push_str_checked(path.as_str());
                        if child.as_str() != "/" {
                            child.push_byte(b'/');
                        }
                        child.push_str_checked(name);
                        if let Ok(metadata) = vfs::metadata(child.as_str()) {
                            let _ = writeln!(
                                output,
                                "{} {:03o} {:>6} {}",
                                if entry.kind == 4 { 'd' } else { '-' },
                                metadata.mode & 0o777,
                                metadata.size,
                                name
                            );
                        }
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
        let Some(input) = arguments.get(offset) else {
            self.usage_named(output, "cat");
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "cat", "invalid path");
            return;
        }
        let mut data = [0u8; 4096];
        match read_file(path.as_str(), &mut data) {
            Ok(length) if textual(&data[..length]) => {
                write_file_bytes(output, &data[..length]);
                if length != 0 && data[length - 1] != b'\n' {
                    let _ = writeln!(output);
                }
            }
            Ok(_) => self.fail(output, "cat", "refusing to print binary data"),
            Err(failure) => self.fail_vfs(output, "cat", failure),
        }
    }

    fn command_head(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let (count, file_index) = parse_line_count(arguments, offset).unwrap_or((10, offset));
        let Some(input) = arguments.get(file_index) else {
            self.usage_named(output, "head");
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "head", "invalid path");
            return;
        }
        let mut data = [0u8; 4096];
        match read_file(path.as_str(), &mut data) {
            Ok(length) if textual(&data[..length]) => {
                let end = prefix_lines(&data[..length], count);
                write_file_bytes(output, &data[..end]);
            }
            Ok(_) => self.fail(output, "head", "binary file"),
            Err(failure) => self.fail_vfs(output, "head", failure),
        }
    }

    fn command_tail(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
        let (count, file_index) = parse_line_count(arguments, offset).unwrap_or((10, offset));
        let Some(input) = arguments.get(file_index) else {
            self.usage_named(output, "tail");
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "tail", "invalid path");
            return;
        }
        let mut data = [0u8; 4096];
        match read_file(path.as_str(), &mut data) {
            Ok(length) if textual(&data[..length]) => {
                let start = suffix_lines(&data[..length], count);
                write_file_bytes(output, &data[start..length]);
            }
            Ok(_) => self.fail(output, "tail", "binary file"),
            Err(failure) => self.fail_vfs(output, "tail", failure),
        }
    }

    fn command_wc(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let Some(input) = arguments.get(offset) else {
            self.usage_named(output, "wc");
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "wc", "invalid path");
            return;
        }
        let mut data = [0u8; 4096];
        match read_file(path.as_str(), &mut data) {
            Ok(length) => {
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
            Err(failure) => self.fail_vfs(output, "wc", failure),
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
    /// syscalls) rather than the boot-time single-shot self-test runner -
    /// only files the VFS serves as a `'static` byte slice can be loaded
    /// this way (the embedded `/bin/*` binaries), since a real process
    /// image has to outlive the syscall that mapped it into the child.
    fn command_run(&mut self, arguments: &Arguments, offset: usize, output: &mut Text<MAX_OUTPUT>) {
        let Some(input) = arguments.get(offset) else {
            self.usage_named(output, "run");
            return;
        };
        let mut path = Text::new();
        if self.make_path(input, &mut path).is_err() {
            self.fail(output, "run", "invalid path");
            return;
        }
        let file = match vfs::file(path.as_str()) {
            Ok(file) => file,
            Err(failure) => {
                self.fail_vfs(output, "run", failure);
                return;
            }
        };
        if let Some(found) = crate::antivirus::scan_bytes(file.data)
            && found.class != crate::antivirus::Class::Info
        {
            let _ = writeln!(
                output,
                "run: blocked by AerOS Shield: {} ({})",
                found.name,
                found.class.label()
            );
            self.last_status = 126;
            return;
        }
        let Some(state) = crate::arch::paging::boot_state() else {
            self.fail(output, "run", "paging not ready");
            return;
        };
        let mut argv: [&[u8]; 16] = [&[]; 16];
        argv[0] = path.as_str().as_bytes();
        let mut argc = 1;
        while argc < argv.len() {
            let Some(value) = arguments.get(offset + argc) else {
                break;
            };
            argv[argc] = value.as_bytes();
            argc += 1;
        }
        let Some((id, _slot, _space)) =
            crate::scheduler::spawn_process_with(&state, file.data, path.as_str(), &argv[..argc])
        else {
            self.fail(output, "run", "not a valid executable");
            return;
        };
        match crate::scheduler::wait_for_child(id) {
            Some(exit_code) => {
                let _ = writeln!(output, "[{} exited with {}]", path.as_str(), exit_code);
            }
            None => self.fail(output, "run", "process did not exit"),
        }
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
                let _ = writeln!(output, "signatures:   {}", status.signatures);
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
        let stats = process::stats();
        let _ = writeln!(output, "PID  PPID STATE    COMMAND");
        let _ = writeln!(output, "0    0    running  kernel");
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

    fn command_jobs(&mut self, output: &mut Text<MAX_OUTPUT>) {
        let stats = scheduler::stats();
        let _ = writeln!(output, "ID  STATE    KIND");
        let _ = writeln!(output, "0   running  kernel");
        let _ = writeln!(
            output,
            "tasks={} ready={} running={} exited={} switches={} fpu_tasks={} fpu_bytes={} fpu_switches={} fpu_isolation={}",
            stats.tasks,
            stats.ready,
            stats.running,
            stats.exited,
            stats.context_switches,
            stats.fpu_tasks,
            stats.fpu_bytes,
            stats.fpu_switches,
            stats.fpu_isolation
        );
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
        if arguments.get(offset).is_some() {
            self.fail(output, "mount", "dynamic mounts are not available yet");
            return;
        }
        let _ = writeln!(output, "initramfs on / type initramfs (ro)");
        let _ = writeln!(output, "tmpfs on /tmp type tmpfs (rw,nosuid,nodev)");
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
    }

    fn command_ping(
        &mut self,
        arguments: &Arguments,
        offset: usize,
        output: &mut Text<MAX_OUTPUT>,
    ) {
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
        let _ = writeln!(output, "tcp       unavailable stack not implemented");
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
        });
    let mut denied = Shell::new(info, false);
    let mut output = Text::new();
    denied.execute("ear whoami", &mut output);
    let denied_valid = denied.last_status == 126
        && denied.denied_commands == 1
        && output.as_str().contains("no elevation capability");
    let mut allowed = Shell::new(info, true);
    allowed.execute("ear whoami", &mut output);
    let privilege = denied_valid
        && allowed.last_status == 0
        && allowed.effective_uid == allowed.uid
        && allowed.elevated_commands == 1
        && output.as_str() == "root\n";
    allowed.execute("ls /", &mut output);
    let listing = allowed.last_status == 0
        && output.as_str().contains("bin/")
        && output.as_str().contains("tmp/");
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
    let filesystem = listing
        && mutation
        && before.nodes == after.nodes
        && before.directories == after.directories
        && before.files == after.files
        && before.bytes == after.bytes
        && before.mutable_files == after.mutable_files
        && before.mutable_bytes == after.mutable_bytes
        && before.open_handles == after.open_handles;
    ShellReport {
        commands: COMMANDS.len(),
        unique,
        parser,
        privilege,
        filesystem,
        verified: COMMANDS.len() == 53 && unique && parser && privilege && filesystem,
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
