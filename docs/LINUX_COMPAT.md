# Linux compatibility

AerOS runs static Linux x86-64 programs through the native `SYSCALL` entry. This page lists what is there and where it stops. The
boot test drives the new corners from real programs (`userspace/sysx`, `userspace/threads`,
`userspace/ptrace`, ...); `tools/test.ps1` is the gate.

## Implemented syscalls (258 numbers)

| | | | |
|---|---|---|---|
| 0 `read` | 1 `write` | 2 `open` | 3 `close` |
| 4 `stat` | 5 `fstat` | 6 `lstat` | 7 `poll` |
| 8 `lseek` | 9 `mmap` | 10 `mprotect` | 11 `munmap` |
| 12 `brk` | 13 `rt_sigaction` | 14 `rt_sigprocmask` | 15 `rt_sigreturn` |
| 16 `ioctl` | 17 `pread64` | 18 `pwrite64` | 19 `readv` |
| 20 `writev` | 21 `access` | 22 `pipe` | 23 `select` |
| 24 `sched_yield` | 25 `mremap` | 26 `msync` | 27 `mincore` |
| 28 `madvise` | 29 `shmget` | 30 `shmat` | 31 `shmctl` |
| 32 `dup` | 33 `dup2` | 34 `pause` | 35 `nanosleep` |
| 36 `getitimer` | 37 `alarm` | 38 `setitimer` | 39 `getpid` |
| 40 `sendfile` | 41 `socket` | 42 `connect` | 43 `accept` |
| 44 `sendto` | 45 `recvfrom` | 46 `sendmsg` | 47 `recvmsg` |
| 48 `shutdown` | 49 `bind` | 50 `listen` | 51 `getsockname` |
| 52 `getpeername` | 53 `socketpair` | 54 `setsockopt` | 55 `getsockopt` |
| 56 `clone` | 57 `fork` | 58 `vfork` | 59 `execve` |
| 60 `exit` | 61 `wait4` | 62 `kill` | 63 `uname` |
| 64 `semget` | 65 `semop` | 66 `semctl` | 67 `shmdt` |
| 68 `msgget` | 69 `msgsnd` | 70 `msgrcv` | 71 `msgctl` |
| 72 `fcntl` | 73 `flock` | 74 `fsync` | 75 `fdatasync` |
| 76 `truncate` | 77 `ftruncate` | 78 `getdents` | 79 `getcwd` |
| 80 `chdir` | 81 `fchdir` | 82 `rename` | 83 `mkdir` |
| 84 `rmdir` | 85 `creat` | 86 `link` | 87 `unlink` |
| 88 `symlink` | 89 `readlink` | 90 `chmod` | 91 `fchmod` |
| 92 `chown` | 93 `fchown` | 94 `lchown` | 95 `umask` |
| 96 `gettimeofday` | 97 `getrlimit` | 98 `getrusage` | 99 `sysinfo` |
| 100 `times` | 101 `ptrace` | 102 `getuid` | 103 `syslog` |
| 104 `getgid` | 105 `setuid` | 106 `setgid` | 107 `geteuid` |
| 108 `getegid` | 109 `setpgid` | 110 `getppid` | 111 `getpgrp` |
| 112 `setsid` | 113 `setreuid` | 114 `setregid` | 115 `getgroups` |
| 116 `setgroups` | 117 `setresuid` | 118 `getresuid` | 119 `setresgid` |
| 120 `getresgid` | 121 `getpgid` | 122 `setfsuid` | 123 `setfsgid` |
| 124 `getsid` | 125 `capget` | 126 `capset` | 127 `rt_sigpending` |
| 128 `rt_sigtimedwait` | 129 `rt_sigqueueinfo` | 130 `rt_sigsuspend` | 131 `sigaltstack` |
| 132 `utime` | 133 `mknod` | 135 `personality` | 137 `statfs` |
| 138 `fstatfs` | 140 `getpriority` | 141 `setpriority` | 142 `sched_setparam` |
| 143 `sched_getparam` | 144 `sched_setscheduler` | 145 `sched_getscheduler` | 146 `sched_get_priority_max` |
| 147 `sched_get_priority_min` | 148 `sched_rr_get_interval` | 149 `mlock` | 150 `munlock` |
| 151 `mlockall` | 152 `munlockall` | 157 `prctl` | 158 `arch_prctl` |
| 160 `setrlimit` | 161 `chroot` | 162 `sync` | 164 `settimeofday` |
| 165 `mount` | 166 `umount2` | 169 `reboot` | 186 `gettid` |
| 187 `readahead` | 200 `tkill` | 201 `time` | 202 `futex` |
| 203 `sched_setaffinity` | 204 `sched_getaffinity` | 213 `epoll_create` | 217 `getdents64` |
| 218 `set_tid_address` | 219 `restart_syscall` | 220 `semtimedop` | 221 `fadvise64` |
| 222 `timer_create` | 223 `timer_settime` | 224 `timer_gettime` | 225 `timer_getoverrun` |
| 226 `timer_delete` | 227 `clock_settime` | 228 `clock_gettime` | 229 `clock_getres` |
| 230 `clock_nanosleep` | 231 `exit_group` | 232 `epoll_wait` | 233 `epoll_ctl` |
| 234 `tgkill` | 235 `utimes` | 253 `inotify_init` | 254 `inotify_add_watch` |
| 255 `inotify_rm_watch` | 257 `openat` | 258 `mkdirat` | 259 `mknodat` |
| 260 `fchownat` | 261 `futimesat` | 262 `newfstatat` | 263 `unlinkat` |
| 264 `renameat` | 265 `linkat` | 266 `symlinkat` | 267 `readlinkat` |
| 268 `fchmodat` | 269 `faccessat` | 270 `pselect6` | 271 `ppoll` |
| 272 `unshare` | 273 `set_robust_list` | 274 `get_robust_list` | 275 `splice` |
| 276 `tee` | 277 `sync_file_range` | 278 `vmsplice` | 280 `utimensat` |
| 281 `epoll_pwait` | 282 `signalfd` | 283 `timerfd_create` | 284 `eventfd` |
| 285 `fallocate` | 286 `timerfd_settime` | 287 `timerfd_gettime` | 288 `accept4` |
| 289 `signalfd4` | 290 `eventfd2` | 291 `epoll_create1` | 292 `dup3` |
| 293 `pipe2` | 294 `inotify_init1` | 295 `preadv` | 296 `pwritev` |
| 297 `rt_tgsigqueueinfo` | 299 `recvmmsg` | 302 `prlimit64` | 306 `syncfs` |
| 307 `sendmmsg` | 308 `setns` | 309 `getcpu` | 310 `process_vm_readv` |
| 311 `process_vm_writev` | 314 `sched_setattr` | 315 `sched_getattr` | 316 `renameat2` |
| 317 `seccomp` | 318 `getrandom` | 319 `memfd_create` | 322 `execveat` |
| 324 `membarrier` | 325 `mlock2` | 326 `copy_file_range` | 327 `preadv2` |
| 328 `pwritev2` | 332 `statx` | 334 `rseq` | 424 `pidfd_send_signal` |
| 434 `pidfd_open` | 435 `clone3` | 436 `close_range` | 437 `openat2` |
| 439 `faccessat2` | 441 `epoll_pwait2` |

The extended attribute family (188-199) answers EOPNOTSUPP, as a filesystem without xattrs does.

## What is deliberately partial

- **Processes**: `fork`, `clone`/`clone3` threads, `execve`/`execveat` (a path that is absolute or
  relative to the working directory, or `AT_EMPTY_PATH`; no directory descriptors), `wait4`, process
  groups and sessions, `ptrace`, `pidfd_open`/`pidfd_send_signal`, `process_vm_readv`/`writev`.
  `execve` takes up to 64 argument and 64 environment strings, 12 KiB in all (Linux allows far more).
  `unshare`/`setns` and namespaces are refused with EPERM. Only uid/gid 0 exists; what separates
  privilege is the capability sets: a program the Terminal starts without `ear` has none (only
  `CAP_NET_BIND_SERVICE`), may signal, trace or `process_vm_*` only what it started, and cannot
  change the system's protected files (see the CHANGELOG's security review). No process may
  signal a kernel task.
- **Programs**: static x86-64 ELF, position-independent or linked at a fixed address (0x400000; the
  image area is 6.25 MiB and needs the first 8 MiB of the machine's memory to be plain RAM, checked
  at boot). Debian's BusyBox runs as-is, and so do dynamically linked programs: `execve` loads the
  interpreter named by `PT_INTERP` (from the initramfs) next to the program and `ld.so` maps the
  libraries (`/lib/x86_64-linux-gnu`, from the initramfs too). Debian's bash and 60 GNU programs are
  in the boot gate, with Perl, git, make, tar, awk and more (141 command lines). Only position-independent
  programs can be dynamic; `execve` and the shell's own program launcher (the desktop Terminal runs any
  `/bin` or `/usr/bin` program for a command it does not know) both start them through the interpreter.
  A fixed-address program (Debian's Python is one, 11 MiB) does not fit the 8 MiB window.
  A process window is 16 MiB: image area 6.25 MiB, heap 512 KiB, stack up to 784 KiB, then the
  mapping area (8.5 MiB; a fixed-address program has 8 MiB in all and 488 KiB of mappings). Process
  memory comes from a pool of a third of the usable memory (16 to 128 MiB).
  `/dev` has `null`, `zero`, `urandom`, `random`, `tty`, `console`, `stdin`, `stdout` and `stderr`
  (no other device nodes). The boot gate runs 92 BusyBox command lines (files, archives, text,
  processes, shell scripting) and `ldconfig`; `bzip2` at its default block size is the known failure
  (it needs more mapping space than the window has).
- **Memory**: anonymous and read-only file mappings, `MAP_SHARED|MAP_ANONYMOUS` (stays shared across
  `fork`), System V shared memory, `mremap` by copy. A writable `MAP_SHARED` mapping of a file is
  refused: the writes would have to reach the file. 2170 mapping pages (122 for a fixed-address program) and 128 heap pages per process.
- **Signals**: handlers with `SA_RESTORER`, masks, alternate stacks, `signalfd`, `rt_sigtimedwait`,
  `rt_sigsuspend`, queued blocked signals. No realtime queueing: a signal is pending or not, and
  `siginfo` has the code, sender, value and, for faults, the address; faults run the process's own SIGSEGV/SIGFPE/SIGILL/SIGBUS handler.
- **Timers**: `alarm`, all three `setitimer` timers, `timerfd`, POSIX `timer_*` (8 per process; the
  `siginfo` carries `sigev_value` and the overrun), `nanosleep`, `clock_gettime` with real CPU-time clocks.
- **Files**: the VFS (initramfs, tmpfs, FAT, AerFS, union overlay), `splice`/`tee`/`vmsplice`,
  `sendfile`, `copy_file_range`, `inotify` (events raised by what processes do; consecutive identical
  events are not merged), `flock`, no `fanotify`. Hard links are refused (EPERM): no filesystem has them.
- **IPC**: pipes, `eventfd`, `socketpair` (stream, and datagram and sequenced-packet pairs that keep message boundaries, up to 4000 bytes a message), named AF_UNIX stream and sequenced-packet sockets (`bind`/`listen`/`accept`/`connect`) and datagram sockets (`bind`, `sendto` a name, `connect` then `send`; BusyBox `syslogd` and `logger` talk over `/dev/log` in the boot gate). Names are matched in a kernel table like Linux's abstract namespace, nothing is created in the filesystem, and a received datagram has no sender address. System V message queues, semaphores and
  shared memory (small fixed tables, kernel-wide so unrelated processes meet by key), `epoll`.
- **Network sockets**: TCP and UDP over IPv4 and IPv6, and IPv4 ping sockets
  (`SOCK_DGRAM` + `IPPROTO_ICMP`: echo requests out, replies in, the identifier is the socket's).
  No raw sockets or `AF_PACKET`.
- **Accounting**: CPU time is sampled from the timer tick per process (`getrusage`, `times`,
  `/proc/<pid>/stat`, `ps`, `top`).
- **Not there**: `fanotify`, `userfaultfd`, `bpf`, `perf_event_open`, `io_uring`, `keyctl`, kernel
  modules, real xattrs. `seccomp` takes classic-BPF filters of up to 64 instructions.

Programs that need more run in the isolated Debian guest (`docs/linux-guest-boot.txt`).
