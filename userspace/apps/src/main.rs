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
            "python-killpg",
            "import os, signal, subprocess\n\
             first = subprocess.Popen(['/bin/sleep', '30'], process_group=0)\n\
             second = subprocess.Popen(['/bin/sleep', '30'], process_group=first.pid)\n\
             assert os.getpgid(second.pid) == first.pid\n\
             os.kill(0, 0)\n\
             assert os.waitpid(-first.pid, os.WNOHANG) == (0, 0)\n\
             try:\n\
             \x20   os.waitpid(0, os.WNOHANG); raise SystemExit('no child shares our group')\n\
             except ChildProcessError:\n\
             \x20   pass\n\
             os.killpg(first.pid, signal.SIGTERM)\n\
             ended = sorted(os.waitpid(-first.pid, 0) for _ in range(2))\n\
             assert [pid for pid, _ in ended] == sorted([first.pid, second.pid]), ended\n\
             codes = tuple(-os.WTERMSIG(status) for _, status in ended)\n\
             assert codes == (-15, -15), codes\n\
             first.returncode = second.returncode = -15\n\
             try:\n\
             \x20   os.killpg(first.pid, 0); raise SystemExit('group still there')\n\
             except ProcessLookupError:\n\
             \x20   pass\n\
             print('killpg', codes)",
        ),        (
            "python-job-control",
            "import os, signal, time\n\
             pid = os.fork()\n\
             if pid == 0:\n\
             \x20   time.sleep(30); os._exit(0)\n\
             os.kill(pid, signal.SIGSTOP)\n\
             _, status = os.waitpid(pid, os.WUNTRACED)\n\
             assert os.WIFSTOPPED(status) and os.WSTOPSIG(status) == signal.SIGSTOP, status\n\
             os.kill(pid, signal.SIGCONT)\n\
             _, status = os.waitpid(pid, os.WCONTINUED)\n\
             assert os.WIFCONTINUED(status), status\n\
             os.kill(pid, signal.SIGTERM)\n\
             _, status = os.waitpid(pid, 0)\n\
             assert os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGTERM, status\n\
             print('job control ok')",
        ),        (
            "python-pty",
            "import os, pty\n\
             pid, fd = pty.fork()\n\
             if pid == 0:\n\
             \x20   print('child tty', os.isatty(0), os.ttyname(0), flush=True)\n\
             \x20   print('got', input(), flush=True)\n\
             \x20   os._exit(3)\n\
             os.write(fd, b'hello pty\\n')\n\
             out = b''\n\
             while True:\n\
             \x20   try:\n\
             \x20       chunk = os.read(fd, 1024)\n\
             \x20   except OSError:\n\
             \x20       break\n\
             \x20   if not chunk:\n\
             \x20       break\n\
             \x20   out += chunk\n\
             _, status = os.waitpid(pid, 0)\n\
             assert b'child tty True /dev/pts/' in out and b'got hello pty' in out, out\n\
             assert os.WEXITSTATUS(status) == 3, status\n\
             print('pty ok')",
        ),        (
            "python-fd-passing",
            "import os, socket\n\
             a, b = socket.socketpair()\n\
             r, w = os.pipe()\n\
             socket.send_fds(a, [b'pipe'], [w])\n\
             os.close(w)\n\
             msg, fds, _, _ = socket.recv_fds(b, 16, 4)\n\
             assert msg == b'pipe' and len(fds) == 1, (msg, fds)\n\
             os.write(fds[0], b'through a passed fd')\n\
             os.close(fds[0])\n\
             assert os.read(r, 64) == b'through a passed fd'\n\
             assert os.read(r, 64) == b''\n\
             pid = os.fork()\n\
             if pid == 0:\n\
             \x20   _, got, _, _ = socket.recv_fds(b, 16, 1)\n\
             \x20   os.write(got[0], b'child'); os._exit(0)\n\
             f = os.open('/tmp/fdpass', os.O_RDWR | os.O_CREAT | os.O_TRUNC)\n\
             socket.send_fds(a, [b'file'], [f])\n\
             os.waitpid(pid, 0)\n\
             os.lseek(f, 0, 0)\n\
             assert os.read(f, 16) == b'child'\n\
             os.unlink('/tmp/fdpass')\n\
             print('fd passing ok')",
        ),        (
            "python-madvise",
            "import mmap\n\
             m = mmap.mmap(-1, 1 << 20, flags=mmap.MAP_PRIVATE | mmap.MAP_ANONYMOUS)\n\
             m[:5] = b'hello'; m[500000:500003] = b'abc'\n\
             m.madvise(mmap.MADV_DONTNEED)\n\
             assert m[:5] == bytes(5) and m[500000:500003] == bytes(3), (m[:5], m[500000:500003])\n\
             m[:2] = b'ok'\n\
             assert m[:2] == b'ok'\n\
             s = mmap.mmap(-1, 4096)\n\
             s[:3] = b'xyz'; s.madvise(mmap.MADV_DONTNEED)\n\
             assert s[:3] == b'xyz'\n\
             m.madvise(mmap.MADV_HUGEPAGE)\n\
             print('madvise ok')",
        ),        (
            "python-epoll-oneshot",
            "import os, select\n\
             r, w = os.pipe()\n\
             ep = select.epoll()\n\
             ep.register(r, select.EPOLLIN | select.EPOLLONESHOT)\n\
             os.write(w, b'x')\n\
             assert ep.poll(0) == [(r, select.EPOLLIN)]\n\
             assert ep.poll(0) == []\n\
             ep.modify(r, select.EPOLLIN | select.EPOLLONESHOT)\n\
             assert ep.poll(0) == [(r, select.EPOLLIN)]\n\
             ep.register(w, select.EPOLLOUT | select.EPOLLET)\n\
             assert (w, select.EPOLLOUT) in ep.poll(0)\n\
             print('epoll ok')",
        ),        (
            "python-waitid",
            "import os, signal, time\n\
             pid = os.fork()\n\
             if pid == 0:\n\
             \x20   os._exit(5)\n\
             info = os.waitid(os.P_PID, pid, os.WEXITED)\n\
             assert info.si_pid == pid and info.si_code == os.CLD_EXITED and info.si_status == 5, info\n\
             pid = os.fork()\n\
             if pid == 0:\n\
             \x20   time.sleep(30); os._exit(0)\n\
             assert os.waitid(os.P_ALL, 0, os.WEXITED | os.WNOHANG) is None\n\
             os.kill(pid, signal.SIGSTOP)\n\
             info = os.waitid(os.P_PID, pid, os.WSTOPPED)\n\
             assert info.si_code == os.CLD_STOPPED and info.si_status == signal.SIGSTOP, info\n\
             os.kill(pid, signal.SIGKILL)\n\
             info = os.waitid(os.P_PID, pid, os.WEXITED)\n\
             assert info.si_code == os.CLD_KILLED and info.si_status == signal.SIGKILL, info\n\
             print('waitid ok')",
        ),        (
            "python-peercred",
            "import os, socket, struct\n\
             a, b = socket.socketpair()\n\
             pid, uid, gid = struct.unpack('3i', a.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))\n\
             assert pid == os.getpid() and uid == 0, (pid, uid, gid)\n\
             server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)\n\
             server.bind('/tmp/peercred.sock'); server.listen()\n\
             child = os.fork()\n\
             if child == 0:\n\
             \x20   c = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)\n\
             \x20   c.connect('/tmp/peercred.sock'); c.recv(1); os._exit(0)\n\
             conn, _ = server.accept()\n\
             pid, _, _ = struct.unpack('3i', conn.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))\n\
             assert pid == child, (pid, child)\n\
             conn.send(b'x'); os.waitpid(child, 0)\n\
             print('peercred ok')",
        ),        (
            "python-pdeathsig",
            "import ctypes, os, signal, time\n\
             libc = ctypes.CDLL(None, use_errno=True)\n\
             r, w = os.pipe()\n\
             held_r, held_w = os.pipe()\n\
             middle = os.fork()\n\
             if middle == 0:\n\
             \x20   if os.fork() == 0:\n\
             \x20       libc.prctl(1, signal.SIGTERM)\n\
             \x20       got = ctypes.c_int(0); libc.prctl(2, ctypes.byref(got))\n\
             \x20       os.write(w, b'%d' % got.value); time.sleep(30); os._exit(0)\n\
             \x20   os.read(r, 0); time.sleep(0.5); os._exit(0)\n\
             os.close(held_w)\n\
             start = time.monotonic()\n\
             assert os.read(r, 8) == b'15'\n\
             os.waitpid(middle, 0)\n\
             assert os.read(held_r, 1) == b''\n\
             assert time.monotonic() - start < 20, time.monotonic() - start\n\
             print('pdeathsig ok')",
        ),        (
            "python-task-names",
            "import ctypes, subprocess, time\n\
             libc = ctypes.CDLL(None)\n\
             assert open('/proc/self/comm').read().strip() == 'python3', open('/proc/self/comm').read()\n\
             libc.prctl(15, b'renamed')\n\
             assert open('/proc/self/comm').read().strip() == 'renamed'\n\
             name = ctypes.create_string_buffer(16); libc.prctl(16, name)\n\
             assert name.value == b'renamed', name.value\n\
             child = subprocess.Popen(['/bin/sleep', '5'])\n\
             time.sleep(0.5)\n\
             assert open('/proc/%d/comm' % child.pid).read().strip() == 'sleep'\n\
             child.kill(); child.wait()\n\
             print('names ok')",
        ),        (
            "python-cmdline",
            "import subprocess, time\n\
             own = open('/proc/self/cmdline', 'rb').read()\n\
             assert own.startswith(b'/usr/bin/python3\\x00-c\\x00'), own[:40]\n\
             child = subprocess.Popen(['/bin/sleep', '5'])\n\
             time.sleep(0.5)\n\
             theirs = open('/proc/%d/cmdline' % child.pid, 'rb').read()\n\
             assert theirs == b'/bin/sleep\\x005\\x00', theirs\n\
             child.kill(); child.wait()\n\
             print('cmdline ok')",
        ),        (
            "python-environ",
            "import subprocess, time\n\
             own = open('/proc/self/environ', 'rb').read()\n\
             assert own and own.endswith(b'\\x00'), own[:60]\n\
             child = subprocess.Popen(['/bin/sleep', '5'], env={'AEROS_PROBE': 'yes'})\n\
             time.sleep(0.5)\n\
             theirs = open('/proc/%d/environ' % child.pid, 'rb').read()\n\
             assert theirs == b'AEROS_PROBE=yes\\x00', theirs\n\
             child.kill(); child.wait()\n\
             print('environ ok')",
        ),        (
            "python-wait4-rusage",
            "import os, time\n\
             pid = os.fork()\n\
             if pid == 0:\n\
             \x20   end = time.monotonic() + 0.5\n\
             \x20   while time.monotonic() < end:\n\
             \x20       pass\n\
             \x20   os._exit(0)\n\
             _, status, usage = os.wait4(pid, 0)\n\
             assert status == 0 and usage.ru_utime + usage.ru_stime > 0.1, usage\n\
             print('wait4 rusage ok')",
        ),        (
            "python-thread-count",
            "import threading, time\n\
             def threads():\n\
             \x20   for line in open('/proc/self/status'):\n\
             \x20       if line.startswith('Threads:'):\n\
             \x20           return int(line.split()[1])\n\
             assert threads() == 1, threads()\n\
             release = threading.Event()\n\
             workers = [threading.Thread(target=release.wait) for _ in range(3)]\n\
             for worker in workers: worker.start()\n\
             assert threads() == 4, threads()\n\
             release.set()\n\
             for worker in workers: worker.join()\n\
             end = time.monotonic() + 2\n\
             while threads() > 1 and time.monotonic() < end: time.sleep(0.05)\n\
             assert threads() == 1, threads()\n\
             print('thread count ok')",
        ),        (
            "python-proc-task",
            "import os, threading, time\n\
             main = threading.get_native_id()\n\
             assert os.listdir('/proc/self/task') == [str(main)], os.listdir('/proc/self/task')\n\
             ids = []\n\
             release = threading.Event()\n\
             def work():\n\
             \x20   ids.append(threading.get_native_id())\n\
             \x20   release.wait()\n\
             workers = [threading.Thread(target=work) for _ in range(2)]\n\
             for worker in workers: worker.start()\n\
             while len(ids) < 2: time.sleep(0.01)\n\
             listed = sorted(int(name) for name in os.listdir('/proc/%d/task' % os.getpid()))\n\
             assert listed == sorted([main] + ids), (listed, main, ids)\n\
             stat = open('/proc/self/task/%d/stat' % ids[0]).read()\n\
             assert stat.startswith('%d (python3) S ' % ids[0]) or stat.startswith('%d (python3) R ' % ids[0]), stat[:40]\n\
             status = open('/proc/self/task/%d/status' % ids[1]).read()\n\
             assert 'Tgid:\\t%d\\n' % os.getpid() in status and 'Pid:\\t%d\\n' % ids[1] in status, status\n\
             assert open('/proc/self/task/%d/comm' % main).read() == 'python3\\n'\n\
             release.set()\n\
             for worker in workers: worker.join()\n\
             end = time.monotonic() + 2\n\
             while len(os.listdir('/proc/self/task')) > 1 and time.monotonic() < end: time.sleep(0.05)\n\
             assert os.listdir('/proc/self/task') == [str(main)], os.listdir('/proc/self/task')\n\
             print('proc task ok')",
        ),        (
            "python-epoll-et",
            "import os, select\n\
             r, w = os.pipe()\n\
             os.set_blocking(r, False)\n\
             edge = select.epoll(); edge.register(r, select.EPOLLIN | select.EPOLLET)\n\
             level = select.epoll(); level.register(r, select.EPOLLIN)\n\
             os.write(w, b'a')\n\
             assert len(edge.poll(0)) == 1\n\
             assert edge.poll(0) == [], 'edge reported twice'\n\
             assert len(level.poll(0)) == 1 and len(level.poll(0)) == 1\n\
             assert os.read(r, 16) == b'a'\n\
             assert edge.poll(0) == [] and level.poll(0) == []\n\
             os.write(w, b'b')\n\
             assert len(edge.poll(0.5)) == 1\n\
             print('epoll et ok')",
        ),        (
            "python-waitid-nowait",
            "import os\n\
             pid = os.fork()\n\
             if pid == 0:\n\
             \x20   os._exit(7)\n\
             info = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOWAIT)\n\
             assert info.si_pid == pid and info.si_status == 7, info\n\
             again = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOWAIT)\n\
             assert again.si_status == 7, again\n\
             assert os.waitpid(pid, 0) == (pid, 7 << 8)\n\
             print('waitid nowait ok')",
        ),        (
            "python-multiprocessing",
            "import multiprocessing\n\
             if __name__ == '__main__':\n\
             \x20   multiprocessing.set_start_method('fork')\n\
             \x20   with multiprocessing.Pool(2) as pool:\n\
             \x20       assert pool.map(abs, [-1, -2, -3]) == [1, 2, 3]\n\
             \x20   queue = multiprocessing.Queue()\n\
             \x20   worker = multiprocessing.Process(target=queue.put, args=(42,))\n\
             \x20   worker.start(); assert queue.get(timeout=20) == 42; worker.join()\n\
             \x20   print('multiprocessing ok')",
        ),        (
            "python-asyncio-tcp",
            "import asyncio\n\
             async def handle(reader, writer):\n\
             \x20   writer.write(await reader.readline()); await writer.drain(); writer.close()\n\
             async def main():\n\
             \x20   server = await asyncio.start_server(handle, '127.0.0.1', 0)\n\
             \x20   port = server.sockets[0].getsockname()[1]\n\
             \x20   reader, writer = await asyncio.open_connection('127.0.0.1', port)\n\
             \x20   writer.write(b'ping\\n'); await writer.drain()\n\
             \x20   assert await asyncio.wait_for(reader.readline(), 10) == b'ping\\n'\n\
             \x20   writer.close(); server.close(); await server.wait_closed()\n\
             asyncio.run(main())\n\
             print('asyncio tcp ok')",
        ),        (
            "python-process-pool",
            "import concurrent.futures, multiprocessing\n\
             if __name__ == '__main__':\n\
             \x20   context = multiprocessing.get_context('fork')\n\
             \x20   with concurrent.futures.ProcessPoolExecutor(2, mp_context=context) as pool:\n\
             \x20       assert list(pool.map(pow, [2, 3], [5, 2])) == [32, 9]\n\
             \x20   print('process pool ok')",
        ),        (
            "python-pty",
            "import os, pty\n\
             pid, fd = pty.fork()\n\
             if pid == 0:\n\
             \x20   os.write(1, b'from the child\\n'); os._exit(3)\n\
             seen = b''\n\
             while b'child' not in seen:\n\
             \x20   seen += os.read(fd, 64)\n\
             assert os.waitpid(pid, 0)[1] == 3 << 8\n\
             print('pty ok')",
        ),        (
            "python-posix-spawn",
            "import os\n\
             pid = os.posix_spawn('/bin/true', ['true'], os.environ)\n\
             assert os.waitpid(pid, 0) == (pid, 0)\n\
             print('posix spawn ok')",
        ),        (
            "python-itimer",
            "import signal, time\n\
             fired = []\n\
             signal.signal(signal.SIGALRM, lambda *_: fired.append(1))\n\
             signal.setitimer(signal.ITIMER_REAL, 0.2)\n\
             end = time.monotonic() + 5\n\
             while not fired and time.monotonic() < end:\n\
             \x20   time.sleep(0.05)\n\
             assert fired, 'SIGALRM never came'\n\
             print('itimer ok')",
        ),        (
            "python-shared-mmap",
            "import mmap, os\n\
             shared = mmap.mmap(-1, 4096)\n\
             pid = os.fork()\n\
             if pid == 0:\n\
             \x20   shared[:5] = b'hello'; os._exit(0)\n\
             os.waitpid(pid, 0)\n\
             assert shared[:5] == b'hello', shared[:5]\n\
             print('shared mmap ok')",
        ),        (
            "python-flock",
            "import fcntl, os\n\
             path = '/tmp/flock-probe'\n\
             holder = open(path, 'w'); fcntl.flock(holder, fcntl.LOCK_EX)\n\
             pid = os.fork()\n\
             if pid == 0:\n\
             \x20   other = open(path, 'w')\n\
             \x20   try:\n\
             \x20       fcntl.flock(other, fcntl.LOCK_EX | fcntl.LOCK_NB); os._exit(1)\n\
             \x20   except BlockingIOError:\n\
             \x20       os._exit(0)\n\
             assert os.waitpid(pid, 0)[1] == 0, 'second lock was granted'\n\
             print('flock ok')",
        ),        (
            "python-lockf",
            "import fcntl, os\n\
             path = '/tmp/lockf-probe'\n\
             holder = open(path, 'w'); holder.write('x' * 64); holder.flush()\n\
             fcntl.lockf(holder, fcntl.LOCK_EX, 10, 0)\n\
             def child(start):\n\
             \x20   other = open(path, 'r+')\n\
             \x20   try:\n\
             \x20       fcntl.lockf(other, fcntl.LOCK_EX | fcntl.LOCK_NB, 2, start); return 0\n\
             \x20   except OSError:\n\
             \x20       return 1\n\
             def run(start):\n\
             \x20   pid = os.fork()\n\
             \x20   if pid == 0:\n\
             \x20       os._exit(child(start))\n\
             \x20   return os.waitpid(pid, 0)[1] >> 8\n\
             assert run(5) == 1, 'overlapping lock granted'\n\
             assert run(20) == 0, 'separate range refused'\n\
             fcntl.lockf(holder, fcntl.LOCK_UN, 10, 0)\n\
             assert run(5) == 0, 'lock refused after unlock'\n\
             print('lockf ok')",
        ),        (
            "python-fifo",
            "import os\n\
             path = '/tmp/fifo-probe'\n\
             os.mkfifo(path)\n\
             pid = os.fork()\n\
             if pid == 0:\n\
             \x20   with open(path, 'wb') as pipe:\n\
             \x20       pipe.write(b'through the fifo')\n\
             \x20   os._exit(0)\n\
             with open(path, 'rb') as pipe:\n\
             \x20   assert pipe.read() == b'through the fifo'\n\
             os.waitpid(pid, 0); os.unlink(path)\n\
             print('fifo ok')",
        ),        (
            "python-copyfile",
            "import os, shutil\n\
             data = os.urandom(300000)\n\
             open('/tmp/copy-a', 'wb').write(data)\n\
             shutil.copyfile('/tmp/copy-a', '/tmp/copy-b')\n\
             assert open('/tmp/copy-b', 'rb').read() == data\n\
             source = os.open('/tmp/copy-a', os.O_RDONLY)\n\
             target = os.open('/tmp/copy-c', os.O_WRONLY | os.O_CREAT | os.O_TRUNC)\n\
             assert os.sendfile(target, source, 0, 1000) == 1000\n\
             os.close(source); os.close(target)\n\
             assert open('/tmp/copy-c', 'rb').read() == data[:1000]\n\
             for name in ('/tmp/copy-a', '/tmp/copy-b', '/tmp/copy-c'): os.unlink(name)\n\
             print('copyfile ok')",
        ),        (
            "python-eventfd-timerfd",
            "import os, select, time\n\
             event = os.eventfd(0)\n\
             os.eventfd_write(event, 3); os.eventfd_write(event, 4)\n\
             assert os.eventfd_read(event) == 7\n\
             timer = os.timerfd_create(time.CLOCK_MONOTONIC)\n\
             os.timerfd_settime(timer, initial=0.1)\n\
             assert select.select([timer], [], [], 5)[0] == [timer]\n\
             assert int.from_bytes(os.read(timer, 8), 'little') == 1\n\
             print('eventfd timerfd ok')",
        ),        (
            "python-usage-clocks",
            "import resource, time\n\
             start = time.process_time()\n\
             end = time.monotonic() + 0.3\n\
             while time.monotonic() < end:\n\
             \x20   pass\n\
             spent = time.process_time() - start\n\
             assert spent > 0.1, spent\n\
             usage = resource.getrusage(resource.RUSAGE_SELF)\n\
             assert usage.ru_utime + usage.ru_stime > 0.1, usage\n\
             assert time.clock_gettime(time.CLOCK_THREAD_CPUTIME_ID) > 0\n\
             print('usage clocks ok')",
        ),        (
            "python-fs-misc",
            "import os\n\
             stats = os.statvfs('/tmp')\n\
             assert stats.f_bsize > 0 and stats.f_blocks >= stats.f_bfree, stats\n\
             open('/tmp/link-a', 'w').write('shared')\n\
             os.link('/tmp/link-a', '/tmp/link-b')\n\
             assert os.stat('/tmp/link-a').st_nlink == 2\n\
             assert os.stat('/tmp/link-a').st_ino == os.stat('/tmp/link-b').st_ino\n\
             open('/tmp/link-b', 'a').write(' file')\n\
             assert open('/tmp/link-a').read() == 'shared file'\n\
             os.unlink('/tmp/link-a')\n\
             assert open('/tmp/link-b').read() == 'shared file'\n\
             assert os.stat('/tmp/link-b').st_nlink == 1\n\
             assert len(os.getrandom(32)) == 32\n\
             print('fs misc ok')",
        ),        (
            "python-udp-resolve",
            "import socket\n\
             server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); server.bind(('127.0.0.1', 0))\n\
             client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)\n\
             client.sendto(b'datagram', server.getsockname())\n\
             server.settimeout(5); data, peer = server.recvfrom(64)\n\
             assert data == b'datagram', data\n\
             server.sendto(b'reply', peer); client.settimeout(5)\n\
             assert client.recvfrom(64)[0] == b'reply'\n\
             assert any(info[4][0] == '127.0.0.1' for info in socket.getaddrinfo('localhost', 80))\n\
             print('udp resolve ok')",
        ),        (
            "python-subprocess-timeout",
            "import subprocess, time\n\
             start = time.monotonic()\n\
             try:\n\
             \x20   subprocess.run(['/bin/sleep', '30'], timeout=0.5)\n\
             \x20   raise SystemExit('no timeout')\n\
             except subprocess.TimeoutExpired:\n\
             \x20   pass\n\
             assert time.monotonic() - start < 10\n\
             out = subprocess.run(['/bin/echo', 'hi'], capture_output=True, check=True).stdout\n\
             assert out == b'hi\\n', out\n\
             print('subprocess timeout ok')",
        ),        (
            "python-sched-load",
            "import os\n\
             cpus = os.sched_getaffinity(0)\n\
             assert cpus and os.cpu_count() >= len(cpus), cpus\n\
             os.sched_setaffinity(0, {min(cpus)})\n\
             assert os.sched_getaffinity(0) == {min(cpus)}\n\
             os.sched_setaffinity(0, cpus)\n\
             assert len(os.getloadavg()) == 3\n\
             print('sched load ok')",
        ),        (
            "python-pipe-ioctl-splice",
            "import fcntl, os, struct, termios\n\
             r, w = os.pipe(); os.write(w, b'12345')\n\
             waiting = struct.unpack('i', fcntl.ioctl(r, termios.FIONREAD, b'\\0' * 4))[0]\n\
             assert waiting == 5, waiting\n\
             r2, w2 = os.pipe()\n\
             assert os.splice(r, w2, 5) == 5 and os.read(r2, 5) == b'12345'\n\
             fd = os.open('/tmp/falloc-probe', os.O_RDWR | os.O_CREAT)\n\
             os.posix_fallocate(fd, 0, 8192); assert os.fstat(fd).st_size == 8192\n\
             os.close(fd)\n\
             print('pipe ioctl splice ok')",
        ),        (
            "python-zip-walk",
            "import os, zipfile\n\
             os.makedirs('/tmp/walk/a/b', exist_ok=True)\n\
             for name in ['/tmp/walk/x', '/tmp/walk/a/y', '/tmp/walk/a/b/z']:\n\
             \x20   open(name, 'w').write(name)\n\
             found = sorted(os.path.join(root, f) for root, _, files in os.walk('/tmp/walk') for f in files)\n\
             assert len(found) == 3, found\n\
             with zipfile.ZipFile('/tmp/walk.zip', 'w', zipfile.ZIP_DEFLATED) as archive:\n\
             \x20   for name in found: archive.write(name)\n\
             with zipfile.ZipFile('/tmp/walk.zip') as archive:\n\
             \x20   assert archive.testzip() is None and len(archive.namelist()) == 3\n\
             import shutil; shutil.rmtree('/tmp/walk'); os.unlink('/tmp/walk.zip')\n\
             print('zip walk ok')",
        ),        (
            "python-http-loopback",
            "import http.server, threading, urllib.request\n\
             class Handler(http.server.BaseHTTPRequestHandler):\n\
             \x20   def do_GET(self):\n\
             \x20       body = b'hello over http'\n\
             \x20       self.send_response(200); self.send_header('Content-Length', str(len(body))); self.end_headers()\n\
             \x20       self.wfile.write(body)\n\
             \x20   def log_message(self, *args):\n\
             \x20       pass\n\
             server = http.server.HTTPServer(('127.0.0.1', 0), Handler)\n\
             threading.Thread(target=server.serve_forever, daemon=True).start()\n\
             url = 'http://127.0.0.1:%d/' % server.server_address[1]\n\
             assert urllib.request.urlopen(url, timeout=10).read() == b'hello over http'\n\
             server.shutdown()\n\
             print('http loopback ok')",
        ),        (
            "python-sqlite-processes",
            "import os, sqlite3\n\
             path = '/tmp/shared.db'\n\
             db = sqlite3.connect(path); db.execute('create table t(n)'); db.commit(); db.close()\n\
             def add(start):\n\
             \x20   db = sqlite3.connect(path, timeout=20)\n\
             \x20   for n in range(start, start + 50):\n\
             \x20       db.execute('insert into t values (?)', (n,)); db.commit()\n\
             \x20   db.close()\n\
             pid = os.fork()\n\
             if pid == 0:\n\
             \x20   add(1000); os._exit(0)\n\
             add(0)\n\
             assert os.waitpid(pid, 0)[1] == 0\n\
             count = sqlite3.connect(path).execute('select count(*) from t').fetchone()[0]\n\
             assert count == 100, count\n\
             print('sqlite processes ok')",
        ),        (
            "python-thread-sync",
            "import threading\n\
             barrier = threading.Barrier(4); hits = []; lock = threading.Semaphore(2)\n\
             def work(n):\n\
             \x20   barrier.wait(timeout=10)\n\
             \x20   with lock:\n\
             \x20       hits.append(n)\n\
             threads = [threading.Thread(target=work, args=(n,)) for n in range(4)]\n\
             for thread in threads: thread.start()\n\
             for thread in threads: thread.join(10)\n\
             assert sorted(hits) == [0, 1, 2, 3], hits\n\
             print('thread sync ok')",
        ),        (
            "python-sigwait",
            "import os, signal\n\
             signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGUSR1})\n\
             os.kill(os.getpid(), signal.SIGUSR1)\n\
             assert signal.SIGUSR1 in signal.sigpending()\n\
             assert signal.sigwait({signal.SIGUSR1}) == signal.SIGUSR1\n\
             print('sigwait ok')",
        ),        (
            "python-rmtree",
            "import os, shutil, tempfile\n\
             root = tempfile.mkdtemp()\n\
             for depth in range(4):\n\
             \x20   path = os.path.join(root, *['d%d' % n for n in range(depth + 1)])\n\
             \x20   os.makedirs(path, exist_ok=True)\n\
             \x20   for n in range(5): open(os.path.join(path, 'f%d' % n), 'w').write('x')\n\
             shutil.rmtree(root)\n\
             assert not os.path.exists(root)\n\
             print('rmtree ok')",
        ),        (
            "python-dgram-fds",
            "import os, socket\n\
             left, right = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)\n\
             r, w = os.pipe()\n\
             socket.send_fds(left, [b'fd'], [w]); os.close(w)\n\
             message, fds, _, _ = socket.recv_fds(right, 16, 4)\n\
             assert message == b'fd' and len(fds) == 1, (message, fds)\n\
             os.write(fds[0], b'via dgram'); os.close(fds[0])\n\
             assert os.read(r, 16) == b'via dgram'\n\
             print('dgram fds ok')",
        ),        (
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
