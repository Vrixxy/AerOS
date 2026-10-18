//! Runs real, unmodified Linux programs (BusyBox, a static non-PIE build from
//! Debian) as separate processes: fork, execve, wait. What they print goes to
//! the console; each run reports its exit status as `AEROS_APPS_RUN`.

use std::arch::asm;

fn sys(number: usize, args: [usize; 6]) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            in("rdi") args[0],
            in("rsi") args[1],
            in("rdx") args[2],
            in("r10") args[3],
            in("r8") args[4],
            in("r9") args[5],
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    result
}

const FORK: usize = 57;
const EXECVE: usize = 59;
const EXIT_GROUP: usize = 231;
const WAIT4: usize = 61;

fn cstr(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

/// Runs `/bin/busybox` with `arguments` (argv[0] included) and reports how it
/// ended: `exit=<code>` or `signal=<number>`.
fn run(label: &str, arguments: &[&str], expected: i32) -> bool {
    let program = cstr(match arguments[0] {
        "ldconfig" => "/bin/ldconfig",
        "dynhello" => "/bin/dynhello",
        path if path.starts_with('/') => path,
        _ => "/bin/busybox",
    });
    let strings: Vec<Vec<u8>> = arguments.iter().map(|argument| cstr(argument)).collect();
    let mut argv: Vec<usize> = strings.iter().map(|string| string.as_ptr() as usize).collect();
    argv.push(0);
    let path = cstr("PATH=/tmp/bin:/bin");
    let home = cstr("HOME=/tmp");
    let envp = [path.as_ptr() as usize, home.as_ptr() as usize, 0usize];
    println!("AEROS_APPS_BEGIN {label}");
    let child = sys(FORK, [0; 6]);
    if child == 0 {
        let result = sys(
            EXECVE,
            [program.as_ptr() as usize, argv.as_ptr() as usize, envp.as_ptr() as usize, 0, 0, 0],
        );
        // Only reached when the program could not be started.
        sys(EXIT_GROUP, [(200 + (-result).min(50)) as usize, 0, 0, 0, 0, 0]);
    }
    if child < 0 {
        println!("AEROS_APPS_RUN {label} fork_failed={child}");
        return false;
    }
    let mut status = 0i32;
    let waited = sys(
        WAIT4,
        [child as usize, &mut status as *mut i32 as usize, 0, 0, 0, 0],
    );
    let ok = waited == child && status == expected << 8;
    if status & 0x7f != 0 {
        println!("AEROS_APPS_RUN {label} signal={}", status & 0x7f);
    } else {
        println!("AEROS_APPS_RUN {label} exit={} ok={ok}", (status >> 8) & 0xff);
    }
    ok
}

/// Makes `/tmp/bin/<name>` links to BusyBox for the applets a shell command
/// line names, as a distribution does with its `/bin` links.
fn link_applets() {
    const MKDIR: usize = 83;
    const SYMLINK: usize = 88;
    let directory = cstr("/tmp/bin");
    sys(MKDIR, [directory.as_ptr() as usize, 0o755, 0, 0, 0, 0]);
    let target = cstr("/bin/busybox");
    for name in [
        "tr", "wc", "cat", "head", "sort", "seq", "gzip", "gunzip", "rm", "echo", "ls", "grep",
        "sed", "awk", "uniq", "cut", "sleep", "true", "false", "test", "date", "sh", "du", "tail",
        "base64", "bzip2", "bunzip2", "xz", "unxz", "nl", "tee", "md5sum", "sha1sum", "tar", "cpio", "od",
    ] {
        let link = cstr(&format!("/tmp/bin/{name}"));
        sys(SYMLINK, [target.as_ptr() as usize, link.as_ptr() as usize, 0, 0, 0, 0]);
    }
}

fn main() {
    link_applets();
    let mut passed = 0;
    let mut total = 0;
    let mut check_exit = |label: &str, arguments: &[&str], expected: i32| {
        total += 1;
        if run(label, arguments, expected) {
            passed += 1;
        }
    };
    check_exit("true", &["busybox", "true"], 0);
    check_exit("echo", &["busybox", "echo", "hello from busybox"], 0);
    check_exit("uname", &["busybox", "uname", "-a"], 0);
    check_exit("cat", &["busybox", "cat", "/etc/aeros-release"], 0);
    check_exit("ls", &["busybox", "ls", "-l", "/bin"], 0);
    check_exit("sh-arithmetic", &["busybox", "sh", "-c", "echo $((6*7))"], 0);
    check_exit(
        "sh-pipeline",
        &["busybox", "sh", "-c", "echo hello world | tr a-z A-Z | wc -c"],
        0,
    );
    check_exit(
        "sh-seq-sort-head",
        &["busybox", "sh", "-c", "seq 1 9 | sort -r | head -n 3"],
        0,
    );
    check_exit("find", &["busybox", "find", "/bin", "-name", "aeros-s*"], 0);
    check_exit("ps", &["busybox", "ps"], 0);
    check_exit("free", &["busybox", "free"], 0);
    check_exit("uptime", &["busybox", "uptime"], 0);
    check_exit("date", &["busybox", "date"], 0);
    check_exit("grep", &["busybox", "grep", "-c", "aeros", "/proc/version"], 0);
    check_exit("md5sum", &["busybox", "md5sum", "/etc/aeros-release"], 0);
    check_exit("sha256sum", &["busybox", "sha256sum", "/etc/aeros-release"], 0);
    check_exit("awk", &["busybox", "awk", "BEGIN { print 2 + 3 }"], 0);
    check_exit("sed", &["busybox", "sed", "s/AerOS/Aer/", "/etc/aeros-release"], 0);
    check_exit(
        "gzip-roundtrip",
        &["busybox", "sh", "-c", "echo squeezed text | gzip -c | gunzip -c"],
        0,
    );
    check_exit(
        "file-io",
        &[
            "busybox",
            "sh",
            "-c",
            "echo written > /tmp/apps.txt && cat /tmp/apps.txt && rm /tmp/apps.txt && echo removed",
        ],
        0,
    );
    check_exit(
        "loop-and-vars",
        &["busybox", "sh", "-c", "n=0; for i in 1 2 3 4; do n=$((n+i)); done; echo sum=$n"],
        0,
    );
    check_exit("sleep", &["busybox", "sleep", "0"], 0);
    check_exit("wc", &["busybox", "wc", "-l", "/etc/aeros-release"], 0);
    // Exit statuses seen from inside the shell (the lines name what is wrong).
    check_exit("status-exit-5", &["busybox", "sh", "-c", "exit 5"], 5);
    check_exit("status-subshell", &["busybox", "sh", "-c", "(exit 3); echo subshell=$?"], 0);
    check_exit("status-false", &["busybox", "sh", "-c", "false; echo false=$?"], 0);
    check_exit("status-pipe-last", &["busybox", "sh", "-c", "true | false; echo pipe_false=$?"], 0);
    check_exit("status-pipe-first", &["busybox", "sh", "-c", "false | true; echo pipe_true=$?"], 0);
    check_exit("status-pipe-echo", &["busybox", "sh", "-c", "echo hi | cat; echo cat_status=$?"], 0);
    check_exit("status-background", &["busybox", "sh", "-c", "sleep 0 & wait; echo wait=$?"], 0);
    // A dynamically linked glibc program: the kernel starts ld.so, which maps libc.
    check_exit("dynhello", &["dynhello"], 42);
    // A static-PIE glibc program of Debian's, started the same way.
    check_exit("ldconfig-version", &["ldconfig", "--version"], 0);
    check_exit("ldconfig-help", &["ldconfig", "--help"], 0);
    check_exit("ldconfig-print-cache", &["ldconfig", "-p"], 1);
    // A wider sweep of BusyBox applets, each a different part of the system
    // call surface (files, directories, archives, text, processes).
    let sweep = |script: &str| ["busybox", "sh", "-c", script].map(|part| part.to_string());
    let sweeps: &[(&str, &str)] = &[
        ("mkdir-touch-ls", "mkdir -p /tmp/w/a/b && touch /tmp/w/a/b/f && ls /tmp/w/a/b"),
        ("cp-mv-rm", "echo data > /tmp/w/x && cp /tmp/w/x /tmp/w/y && mv /tmp/w/y /tmp/w/z && cat /tmp/w/z && rm /tmp/w/x /tmp/w/z"),
        ("ln-readlink", "echo t > /tmp/w/t && ln -s /tmp/w/t /tmp/w/l && readlink /tmp/w/l && cat /tmp/w/l"),
        ("chmod-stat", "echo c > /tmp/w/c && chmod 600 /tmp/w/c && stat -c '%a %s' /tmp/w/c"),
        ("find-name", "find /tmp/w -name 'f' -type f"),
        ("du-df", "du -s /tmp/w; df /tmp | tail -n 1"),
        ("tar-roundtrip", "cd /tmp/w && echo tarred > tf && tar cf t.tar tf && rm tf && tar xf t.tar && cat tf"),
        ("cpio-roundtrip", "cd /tmp/w && echo cp > cf && echo cf | cpio -o -H newc > c.cpio 2>/dev/null && rm cf && cpio -i < c.cpio 2>/dev/null && cat cf"),
        ("gzip-file", "cd /tmp/w && echo zipped > zf && gzip -k zf && gunzip -c zf.gz"),
        ("xz", "echo xz | xz -c | unxz -c"),
        ("od-hexdump", "echo AB | od -An -tx1; echo AB | hexdump -C | head -n 1"),
        ("base64", "echo hello | base64 | base64 -d"),
        ("sort-uniq", "printf 'b
a
b
' | sort | uniq -c"),
        ("cut-tr-paste", "echo a:b:c | cut -d: -f2; echo abc | tr a-c A-C"),
        ("xargs", "echo 1 2 3 | xargs -n 1 echo n="),
        ("expr-test", "expr 6 '*' 7; [ -f /etc/aeros-release ] && echo exists"),
        ("printf-seq", "printf '%05d|%s
' 42 text; seq 3 | tr '
' ,"),
        ("diff", "printf 'a
b
' > /tmp/w/d1; printf 'a
c
' > /tmp/w/d2; diff /tmp/w/d1 /tmp/w/d2; true"),
        ("head-tail-rev", "seq 5 | tail -n 2; echo abc | rev; seq 3 | tac"),
        ("id-whoami-hostname", "id; whoami; hostname"),
        ("env-sh", "env FOO=bar sh -c 'echo $FOO'"),
        ("date-seconds", "date +%s | grep -c '^[0-9]'"),
        ("dd", "dd if=/etc/aeros-release of=/tmp/w/dd bs=4 count=2 2>/dev/null; cat /tmp/w/dd; echo"),
        ("stat-file", "stat /etc/aeros-release | head -n 2"),
        ("wc-nl", "printf 'a b
c
' | wc; printf 'x
y
' | nl"),
        ("bc-dc", "echo '2+3' | dc -e '2 3 + p' 2>/dev/null || echo dc-missing"),
        ("sed-awk-script", "printf '1 2
3 4
' | awk '{s+=$1+$2} END{print s}'"),
        ("sh-function", "f() { echo in-f $1; }; f arg; for i in a b; do echo $i; done"),
        ("sh-case-while", "i=0; while [ $i -lt 3 ]; do i=$((i+1)); done; case $i in 3) echo three;; esac"),
        ("sh-here-doc", "cat <<EOF
line1
line2
EOF"),
        ("sh-subst", "echo $(echo nested) `echo back`"),
        ("sh-trap-kill", "trap 'echo got-term' TERM; kill -TERM $$; echo after"),
        ("sh-background-wait", "sleep 0 & sleep 0 & wait; echo done"),
        ("sleep-timeout", "sleep 1 & kill $!; wait; echo killed"),
        ("pidof-ps", "ps | head -n 3"),
        ("top-batch", "top -b -n 1 | head -n 4"),
        ("uptime-free", "uptime; free | head -n 2"),
        ("mount-list", "mount | head -n 3"),
        ("lsmod-dmesg", "dmesg | head -n 1; true"),
        ("uname-all", "uname -s -r -m"),
        ("nproc", "nproc"),
        ("true-false", "true && echo t; false || echo f"),
        ("yes-head", "yes | head -n 2"),
        ("tee", "echo teed | tee /tmp/w/tee1 | cat; cat /tmp/w/tee1"),
        ("sync", "sync"),
        ("mktemp", "f=$(mktemp); echo ok > $f; cat $f; rm $f"),
        ("realpath-dirname", "realpath /tmp/../tmp; dirname /a/b/c; basename /a/b/c"),
        ("watch-timeout", "timeout 1 sleep 0; echo t=$?"),
        ("hexedit-strings", "strings /bin/busybox | head -n 1"),
        ("md5", "echo x | md5sum"),
        ("tail-follow-no", "echo last | tail -c 5"),
        ("vi-batch", "printf 'a
' > /tmp/w/v && vi -c 'q' /tmp/w/v 2>/dev/null; true"),
        ("dev-null", "echo gone > /dev/null; cat /dev/null; echo rc=$?"),
        ("dev-zero", "head -c 4 /dev/zero | od -An -tx1"),
        ("dev-urandom", "head -c 8 /dev/urandom | wc -c"),
        ("dev-ls", "ls /dev | head -n 3"),
        ("dev-stderr", "echo to-stderr > /dev/stderr"),
        ("ping-localhost", "ping -c 1 127.0.0.1 2>&1 | head -n 2; true"),
    ];
    for (label, script) in sweeps {
        let arguments = sweep(script);
        let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();
        // Output and status are what is being looked at; a script ending in
        // `; true` is allowed to fail silently, others must exit 0.
        check_exit(label, &borrowed, 0);
    }
    // Debian's own GNU programs, dynamically linked against glibc.
    let gnu: &[(&str, &[&str])] = &[
        ("gnu-echo", &["/bin/echo", "hello gnu"]),
        ("gnu-cat", &["/bin/cat", "/etc/aeros-release"]),
        ("gnu-ls", &["/bin/ls", "/bin"]),
        ("gnu-ls-long", &["/bin/ls", "-l", "/etc"]),
        ("gnu-uname", &["/bin/uname", "-sm"]),
        ("gnu-wc", &["/bin/wc", "-c", "/etc/aeros-release"]),
        ("gnu-stat", &["/bin/stat", "-c", "%s", "/etc/aeros-release"]),
        ("gnu-date", &["/bin/date", "+%Y"]),
    ];
    for (label, arguments) in gnu {
        check_exit(label, arguments, 0);
    }
    let bash_scripts: &[(&str, &str)] = &[
        ("bash-builtins", r#"echo builtin; x=$(echo nested); echo $x; for i in 1 2 3; do echo n$i; done"#),
        ("bash-pipeline", r#"/bin/ls /bin | /bin/head -n 3"#),
        ("bash-files", r#"/bin/mkdir -p /tmp/g/a && /bin/touch /tmp/g/a/f && /bin/ls /tmp/g/a && /bin/cp /tmp/g/a/f /tmp/g/b && /bin/mv /tmp/g/b /tmp/g/c && /bin/rm /tmp/g/c /tmp/g/a/f && echo files-ok"#),
        ("bash-printf-tr", r#"/bin/printf '%s-%d
' a 7 | /bin/tr a-z A-Z"#),
        ("bash-sort", r#"/bin/seq 5 | /bin/sort -rn | /bin/head -n 2"#),
        ("bash-uniq", r#"/bin/printf 'b
a
b
' | /bin/sort | /bin/uniq -c"#),
        ("bash-grep-sed", r#"/bin/grep -c AerOS /etc/aeros-release; /bin/sed s/AerOS/X/ /etc/aeros-release"#),
        ("bash-gzip", r#"/bin/gzip -c /etc/aeros-release | /bin/gzip -dc"#),
        ("bash-du-df", r#"/bin/du -s /etc; /bin/df /tmp | /bin/tail -n 1"#),
        ("bash-find", r#"/bin/find /bin -maxdepth 1 -name 'ba*'"#),
        ("bash-expr", r#"/bin/expr 6 '*' 7"#),
        ("bash-od-base64", r#"/bin/od -An -tx1 /etc/aeros-release | /bin/head -n 1; /bin/base64 /etc/aeros-release"#),
        ("bash-dd", r#"/bin/dd if=/etc/aeros-release bs=4 count=2 2>/dev/null; echo"#),
        ("bash-status", r#"/bin/true && echo t; /bin/false || echo f; /bin/sleep 0"#),
        ("bash-names", r#"/bin/basename /a/b/c; /bin/dirname /a/b/c; /bin/id -u"#),
        ("bash-nested", r#"/bin/env FOO=bar /bin/bash -c 'echo $FOO'"#),
        ("bash-mktemp", r#"d=$(/bin/mktemp -d); /bin/test -d $d && echo temp-ok; /bin/rmdir $d"#),
        ("bash-xargs", r#"echo 1 2 3 | /bin/xargs -n1 /bin/echo n="#),
    ];
    for (label, script) in bash_scripts {
        check_exit(label, &["/bin/bash", "-c", script], 0);
    }
    println!("AEROS_APPS done passed={passed} total={total}");
    std::process::exit(if passed == total { 90 } else { 91 });
}
