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
        "dyntest" => "/bin/dyntest",
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
        "syslogd", "logger",
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
    // Threads, sockets, signals, dlopen and more, in one dynamically linked program.
    check_exit("dyntest", &["dyntest"], 0);
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
        ("syslogd-logger", "syslogd -n -O /tmp/w/sys.log & sleep 1; logger aeros-syslog-probe; sleep 1; kill $!; grep -c aeros-syslog-probe /tmp/w/sys.log"),
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
        ("tar-roundtrip", r#"cd /tmp && /bin/tar cf t.tar /etc/aeros-release && /bin/tar tf t.tar && /bin/tar xOf t.tar etc/aeros-release && /bin/rm t.tar"#),
        ("awk", r#"echo 1 2 3 | /bin/awk '{s=0; for(i=1;i<=NF;i++) s+=$i; print s}'"#),
        ("make", r#"cd /tmp && /bin/printf 'all:\n\t@echo built-by-make\n' > Makefile && /bin/make && /bin/rm Makefile"#),
        ("meminfo-before", r#"/bin/head -n 3 /proc/meminfo"#),
        ("xz-only", r#"echo roundtrip | /bin/xz -0 -c | /bin/xz -dc"#),
        ("bzip2-only", r#"echo roundtrip | /bin/bzip2 -1 -c | /bin/bzip2 -dc"#),
        ("bzip2-default", r#"/bin/head -c 1000000 /dev/zero | /bin/bzip2 -c | /bin/bzip2 -dc | /bin/wc -c"#),
        ("meminfo-after", r#"/bin/head -n 3 /proc/meminfo"#),
        ("xz-bzip2", r#"echo roundtrip | /bin/xz -0 -c | /bin/xz -dc; echo roundtrip | /bin/bzip2 -1 -c | /bin/bzip2 -dc"#),
        ("diff-cmp-patch", r#"cd /tmp && echo one > a && echo two > b && /bin/cmp a b; /bin/diff -u a b > d.patch; /bin/patch a < d.patch && /bin/cmp a b && echo patched; /bin/rm a b d.patch"#),
        ("less-nano-vi-version", r#"/bin/less --version | /bin/head -n 1; /bin/nano --version | /bin/head -n 1; /bin/vi --version | /bin/head -n 1"#),
        ("git-version", r#"/bin/git --version"#),
        ("git-repository", r#"cd /tmp && /bin/rm -rf gr && /bin/mkdir gr && cd gr && /bin/git init -q . && echo hello > f && /bin/git add f && GIT_AUTHOR_NAME=a GIT_AUTHOR_EMAIL=a@b GIT_COMMITTER_NAME=a GIT_COMMITTER_EMAIL=a@b /bin/git commit -q -m first && /bin/git log --oneline | /bin/wc -l && /bin/git status --short | /bin/wc -l && /bin/git cat-file -p HEAD:f"#),
        ("hostname", r#"/bin/hostname"#),
    ];
    for (label, script) in bash_scripts {
        check_exit(label, &["/bin/bash", "-c", script], 0);
    }
    // Debian's Perl and its core modules, loaded through ld.so.
    let perl: &[(&str, &str)] = &[
        ("perl-print", "print 2 + 2, qq(\n)"),
        (
            "perl-modules",
            "use strict; use warnings; use List::Util qw(sum max); use POSIX qw(floor); \
             print sum(1..10), ' ', max(3, 9, 2), ' ', floor(3.7), qq(\n)",
        ),
        (
            "perl-hash-regex",
            "my %h = (a => 1, b => 2); print join(',', map { qq($_=$h{$_}) } sort keys %h), qq(\n); \
             'hello world' =~ /(\\w+)\\s+(\\w+)/ and print qq($2 $1\n)",
        ),
        (
            "perl-files",
            "open(my $f, '>', '/tmp/pl.txt') or die; print $f qq(data\n); close $f; \
             print -s '/tmp/pl.txt', qq(\n); open($f, '<', '/tmp/pl.txt'); print scalar(<$f>); \
             unlink '/tmp/pl.txt'; print -e '/tmp/pl.txt' ? qq(still\n) : qq(gone\n)",
        ),
        (
            "perl-subprocess",
            "my $out = `/bin/echo from-shell`; print $out; system('/bin/echo', 'from-system'); \
             print $? >> 8, qq(\n)",
        ),
        (
            "perl-core-modules",
            "use File::Basename; use File::Spec; use Cwd; use Getopt::Long; use Scalar::Util qw(blessed); \
             print basename('/a/b/c.txt'), ' ', File::Spec->catfile('x', 'y'), ' ', getcwd(), qq(\n)",
        ),
        (
            "perl-unix-sockets",
            "use Socket; socketpair(my $p, my $q, AF_UNIX, SOCK_STREAM, 0) or die qq(pair $!); \
             syswrite($p, 'peek'); defined recv($q, my $x, 4, MSG_PEEK) or die qq(peek $!); \
             sysread($q, my $y, 4); socket(my $s, AF_UNIX, SOCK_DGRAM, 0) or die qq(socket $!); \
             bind($s, pack_sockaddr_un('aeros-perl-dgram')) or die qq(bind $!); \
             socket(my $c, AF_UNIX, SOCK_DGRAM, 0) or die; \
             send($c, 'dgram', 0, pack_sockaddr_un('aeros-perl-dgram')) or die qq(send $!); \
             recv($s, my $z, 64, 0); print qq($x $y $z\n); exit($x.$y.$z eq 'peekpeekdgram' ? 0 : 1)",
        ),
        (
            "perl-big-string",
            "my $x = 'a' x 20_000_000; print length($x), qq(\\n); exit(length($x) == 20_000_000 ? 0 : 1)",
        ),
        (
            "perl-sprintf-sort",
            "printf(qq(%05.1f %s %x\n), 3.14159, join('', sort { $b <=> $a } (3, 1, 2)), 255)",
        ),
    ];
    for (label, code) in perl {
        check_exit(label, &["/usr/bin/perl", "-e", code], 0);
    }
    // Debian's Python 3.13: a fixed-address program with an interpreter.
    check_exit("python-version", &["/usr/bin/python3", "--version"], 0);
    let python: &[(&str, &str)] = &[
        (
            "python-stdlib",
            "import json, re, collections, os, sys\n\
             print(json.dumps(sorted(collections.Counter('hello').items())))\n\
             print(re.sub(r'l+', 'L', 'hello'), os.getcwd(), sys.version_info[:2])",
        ),
        (
            "python-files",
            "import pathlib, tempfile\n\
             p = pathlib.Path(tempfile.mkdtemp()) / 'x.txt'\n\
             p.write_text('from python')\n\
             print(p.read_text(), p.stat().st_size)\n\
             p.unlink()",
        ),
        (
            "python-ssl-sqlite",
            "import hashlib, ssl, sqlite3, uuid\n\
             print(hashlib.sha256(b'aeros').hexdigest()[:16], ssl.OPENSSL_VERSION.split()[0])\n\
             context = ssl.create_default_context()\n\
             assert context.cert_store_stats()['x509_ca'] > 100\n\
             db = sqlite3.connect(':memory:')\n\
             db.execute('create table t (n)'); db.executemany('insert into t values (?)', [(1,), (2,), (3,)])\n\
             print('sum', db.execute('select sum(n) from t').fetchone()[0], uuid.uuid4().version)",
        ),
        (
            "python-http-thread",
            "import http.server, threading, urllib.request, json\n\
             server = http.server.HTTPServer(('127.0.0.1', 0), http.server.SimpleHTTPRequestHandler)\n\
             threading.Thread(target=server.serve_forever, daemon=True).start()\n\
             reply = urllib.request.urlopen('http://127.0.0.1:%d/' % server.server_port, timeout=20)\n\
             print(reply.status, json.dumps({'threads': threading.active_count()}))\n\
             server.shutdown()",
        ),
        (
            "python-sqlite-file",
            "import sqlite3, os\n\
             path = '/tmp/aeros-probe.db'\n\
             db = sqlite3.connect(path)\n\
             db.execute('create table if not exists t (n integer, s text)')\n\
             with db:\n\
             \x20   db.executemany('insert into t values (?, ?)', [(i, str(i) * 20) for i in range(500)])\n\
             db.close()\n\
             again = sqlite3.connect(path)\n\
             print(again.execute('select count(*), sum(n) from t').fetchone(), os.path.getsize(path) > 0)\n\
             again.close(); os.remove(path)",
        ),
        (
            "python-big-tmp-file",
            "import os, hashlib\n\
             data = os.urandom(1 << 20) * 20\n\
             with open('/tmp/aeros-big', 'wb') as f:\n\
             \x20   f.write(data)\n\
             with open('/tmp/aeros-big', 'rb') as f:\n\
             \x20   back = f.read()\n\
             print(len(back), hashlib.sha256(back).digest() == hashlib.sha256(data).digest())\n\
             os.remove('/tmp/aeros-big')",
        ),
        (
            "python-asyncio",
            "import asyncio\n\
             async def handle(reader, writer):\n\
             \x20   writer.write((await reader.readline()).upper()); await writer.drain(); writer.close()\n\
             async def main():\n\
             \x20   server = await asyncio.start_server(handle, '127.0.0.1', 0)\n\
             \x20   port = server.sockets[0].getsockname()[1]\n\
             \x20   reader, writer = await asyncio.open_connection('127.0.0.1', port)\n\
             \x20   writer.write(b'hello asyncio\\n'); await writer.drain()\n\
             \x20   print((await reader.readline()).decode().strip())\n\
             \x20   writer.close(); server.close(); await server.wait_closed()\n\
             asyncio.run(main())",
        ),
        (
            "python-multiprocessing",
            "import multiprocessing\n\
             def square(value):\n\
             \x20   return value * value\n\
             with multiprocessing.Pool(2) as pool:\n\
             \x20   print(sum(pool.map(square, range(10))))",
        ),
        (
            "python-subprocess",
            "import subprocess\n\
             print(subprocess.run(['/bin/echo', 'child'], capture_output=True, text=True).stdout.strip())",
        ),
        (
            "python-pipe-capacity",
            "import os, subprocess\n\
             r, w = os.pipe()\n\
             assert os.write(w, b'x' * 65536) == 65536\n\
             os.set_blocking(w, False)\n\
             try:\n\
             \x20   os.write(w, b'y'); raise SystemExit('pipe should be full')\n\
             except BlockingIOError:\n\
             \x20   pass\n\
             os.close(w); os.close(r)\n\
             data = os.urandom(1 << 20)\n\
             out = subprocess.run(['/bin/cat'], input=data, capture_output=True).stdout\n\
             assert out == data, len(out)\n\
             print('pipe ok')",
        ),
        (
            "python-many-threads",
            "import threading, resource\n\
             barrier = threading.Barrier(41, timeout=60)\n\
             results = []\n\
             def work(n):\n\
             \x20   barrier.wait(); results.append(n * n)\n\
             threads = [threading.Thread(target=work, args=(n,)) for n in range(40)]\n\
             for started, t in enumerate(threads):\n\
             \x20   try:\n\
             \x20       t.start()\n\
             \x20   except RuntimeError:\n\
             \x20       barrier.abort(); raise SystemExit('started %d threads' % started)\n\
             barrier.wait()\n\
             [t.join() for t in threads]\n\
             assert sorted(results) == [n * n for n in range(40)]\n\
             print(len(results), resource.getrlimit(resource.RLIMIT_STACK)[0] >> 20)",
        ),
        (
            "python-rlimits",
            "import os, resource\n\
             resource.setrlimit(resource.RLIMIT_CORE, (0, resource.RLIM_INFINITY))\n\
             assert resource.getrlimit(resource.RLIMIT_CORE)[0] == 0\n\
             soft, hard = resource.getrlimit(resource.RLIMIT_STACK)\n\
             resource.setrlimit(resource.RLIMIT_STACK, (hard, hard))\n\
             resource.setrlimit(resource.RLIMIT_STACK, (soft, hard))\n\
             resource.setrlimit(resource.RLIMIT_NOFILE, (32, 48))\n\
             assert resource.prlimit(0, resource.RLIMIT_NOFILE, (8, 48)) == (32, 48)\n\
             fds = []\n\
             try:\n\
             \x20   while True:\n\
             \x20       fds.append(os.open('/dev/null', os.O_RDONLY))\n\
             except OSError as error:\n\
             \x20   assert error.errno == 24 and len(fds) < 8, (error, len(fds))\n\
             \x20   print('emfile', len(fds))",
        ),
    ];
    for (label, code) in python {
        check_exit(label, &["/usr/bin/python3", "-c", code], 0);
    }
    println!("AEROS_APPS done passed={passed} total={total}");
    std::process::exit(if passed == total { 90 } else { 91 });
}
