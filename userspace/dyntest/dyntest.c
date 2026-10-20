// A dynamically linked glibc program that exercises what real programs use:
// threads, fork and pipes, signals, sockets, files, mmap, dlopen, time.
// Built with Debian's gcc (tools/build-dyntest.sh) and run by aeros-apps.
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <math.h>
#include <netinet/in.h>
#include <poll.h>
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/timerfd.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

extern char **environ;
static int failures;

#define CHECK(name, condition)                                                  \
    do {                                                                        \
        int ok_ = (condition);                                                  \
        printf("AEROS_DT %s %s\n", name, ok_ ? "ok" : "FAIL");                  \
        fflush(stdout);                                                         \
        if (!ok_) failures++;                                                   \
    } while (0)

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t ready = PTHREAD_COND_INITIALIZER;
static int counter, flag;

static void *worker(void *arg) {
    for (int i = 0; i < 1000; i++) {
        pthread_mutex_lock(&lock);
        counter++;
        pthread_mutex_unlock(&lock);
    }
    return arg;
}

static void *signaller(void *arg) {
    pthread_mutex_lock(&lock);
    flag = 1;
    pthread_cond_signal(&ready);
    pthread_mutex_unlock(&lock);
    return arg;
}

static volatile int got_signal;
static void on_signal(int number, siginfo_t *info, void *context) {
    (void)info;
    (void)context;
    got_signal = number;
}

static jmp_buf jump;

static int compare(const void *a, const void *b) {
    return *(const int *)a - *(const int *)b;
}

static void *serve(void *arg) {
    int listener = *(int *)arg;
    int client = accept(listener, 0, 0);
    char buffer[16];
    int count = read(client, buffer, sizeof buffer);
    if (count > 0) write(client, buffer, count);
    close(client);
    return 0;
}

int main(void) {
    // fork, pipe, wait
    int pipes[2];
    pipe(pipes);
    pid_t child = fork();
    if (child == 0) {
        close(pipes[0]);
        write(pipes[1], "from-child", 10);
        _exit(7);
    }
    close(pipes[1]);
    char text[32] = {0};
    int count = read(pipes[0], text, sizeof text - 1);
    int status = 0;
    waitpid(child, &status, 0);
    CHECK("fork-pipe-wait", count == 10 && strcmp(text, "from-child") == 0 &&
                               WIFEXITED(status) && WEXITSTATUS(status) == 7);
    close(pipes[0]);

    // posix_spawn and system
    pid_t spawned;
    char *argv[] = {"/bin/echo", "spawned", 0};
    int spawn_result = posix_spawn(&spawned, "/bin/echo", 0, 0, argv, environ);
    int spawn_status = -1;
    if (spawn_result == 0) waitpid(spawned, &spawn_status, 0);
    CHECK("posix-spawn", spawn_result == 0 && WIFEXITED(spawn_status) &&
                             WEXITSTATUS(spawn_status) == 0);
    int system_status = system("/bin/echo from-system");
    CHECK("system", system_status == 0);

    // signals
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_sigaction = on_signal;
    action.sa_flags = SA_SIGINFO;
    sigaction(SIGUSR1, &action, 0);
    raise(SIGUSR1);
    CHECK("signal-raise", got_signal == SIGUSR1);
    got_signal = 0;
    sigaction(SIGALRM, &action, 0);
    alarm(1);
    for (int i = 0; i < 300 && !got_signal; i++) usleep(10000);
    CHECK("signal-alarm", got_signal == SIGALRM);
    CHECK("setjmp", setjmp(jump) == 0 ? (longjmp(jump, 1), 0) : 1);
    signal(SIGALRM, SIG_DFL);
    alarm(40);

    // files and directories
    mkdir("/tmp/dt", 0755);
    int fd = open("/tmp/dt/a", O_CREAT | O_RDWR | O_TRUNC, 0644);
    write(fd, "hello mmap", 10);
    char *map = mmap(0, 4096, PROT_READ, MAP_PRIVATE, fd, 0);
    CHECK("mmap-file", map != MAP_FAILED && memcmp(map, "hello mmap", 10) == 0);
    if (map != MAP_FAILED) munmap(map, 4096);
    int high = dup2(fd, 40);
    CHECK("fd-above-24", high == 40);
    close(high);
    close(fd);
    CHECK("rename", rename("/tmp/dt/a", "/tmp/dt/b") == 0);
    struct stat info;
    CHECK("stat", stat("/tmp/dt/b", &info) == 0 && info.st_size == 10);
    DIR *directory = opendir("/bin");
    int entries = 0;
    struct dirent *entry;
    while (directory && (entry = readdir(directory))) entries++;
    if (directory) closedir(directory);
    CHECK("readdir", entries > 20);
    char cwd[128];
    CHECK("getcwd-chdir", chdir("/tmp/dt") == 0 && getcwd(cwd, sizeof cwd) &&
                              strcmp(cwd, "/tmp/dt") == 0);
    unlink("/tmp/dt/b");
    chdir("/");
    rmdir("/tmp/dt");
    CHECK("unlink-rmdir", stat("/tmp/dt", &info) != 0);

    // shared anonymous memory across fork
    int *shared = mmap(0, 4096, PROT_READ | PROT_WRITE,
                       MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    *shared = 1;
    if (fork() == 0) {
        *shared = 2;
        _exit(0);
    }
    wait(0);
    CHECK("mmap-shared-fork", *shared == 2);

    // libraries
    void *handle = dlopen("libm.so.6", RTLD_NOW);
    double (*cosine)(double) = handle ? (double (*)(double))dlsym(handle, "cos") : 0;
    CHECK("dlopen", cosine && fabs(cosine(0.0) - 1.0) < 1e-9);
    char formatted[64];
    snprintf(formatted, sizeof formatted, "%.3f %5d %s", 3.14159, 42, "x");
    CHECK("printf-float", strcmp(formatted, "3.142    42 x") == 0);
    int numbers[] = {5, 3, 9, 1, 7};
    qsort(numbers, 5, sizeof(int), compare);
    CHECK("qsort", numbers[0] == 1 && numbers[4] == 9);
    CHECK("strtod", fabs(strtod("2.5e2", 0) - 250.0) < 1e-9);

    // time
    struct timespec before, after;
    clock_gettime(CLOCK_MONOTONIC, &before);
    struct timespec pause = {0, 20 * 1000 * 1000};
    nanosleep(&pause, 0);
    clock_gettime(CLOCK_MONOTONIC, &after);
    long elapsed = (after.tv_sec - before.tv_sec) * 1000 +
                   (after.tv_nsec - before.tv_nsec) / 1000000;
    CHECK("nanosleep", elapsed >= 19);
    time_t now = time(0);
    struct tm parts;
    gmtime_r(&now, &parts);
    char date[32];
    strftime(date, sizeof date, "%Y", &parts);
    CHECK("strftime", atoi(date) >= 2026);

    // sockets
    int pair[2];
    CHECK("socketpair", socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
    write(pair[0], "ping", 4);
    char reply[8] = {0};
    read(pair[1], reply, 4);
    CHECK("socketpair-io", strcmp(reply, "ping") == 0);
    int listener = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in address;
    memset(&address, 0, sizeof address);
    address.sin_family = AF_INET;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    address.sin_port = htons(19001);
    int bound = bind(listener, (struct sockaddr *)&address, sizeof address);
    listen(listener, 4);
    pthread_t server;
    pthread_create(&server, 0, serve, &listener);
    int client = socket(AF_INET, SOCK_STREAM, 0);
    int connected = connect(client, (struct sockaddr *)&address, sizeof address);
    write(client, "tcp-echo", 8);
    char echoed[16] = {0};
    read(client, echoed, 8);
    pthread_join(server, 0);
    CHECK("tcp-loopback", bound == 0 && connected == 0 && strcmp(echoed, "tcp-echo") == 0);
    close(client);
    close(listener);

    // poll, epoll, eventfd, timerfd
    int event = eventfd(0, 0);
    uint64_t one = 1;
    write(event, &one, 8);
    struct pollfd polled = {event, POLLIN, 0};
    CHECK("poll-eventfd", poll(&polled, 1, 1000) == 1 && (polled.revents & POLLIN));
    int epoll = epoll_create1(0);
    struct epoll_event wanted = {EPOLLIN, {.fd = event}}, got;
    epoll_ctl(epoll, EPOLL_CTL_ADD, event, &wanted);
    CHECK("epoll", epoll_wait(epoll, &got, 1, 1000) == 1);
    close(epoll);
    close(event);
    int timer = timerfd_create(CLOCK_MONOTONIC, 0);
    struct itimerspec spec = {{0, 0}, {0, 30 * 1000 * 1000}};
    timerfd_settime(timer, 0, &spec, 0);
    uint64_t expirations = 0;
    CHECK("timerfd", read(timer, &expirations, 8) == 8 && expirations >= 1);
    close(timer);

    // threads
    pthread_t threads[4];
    for (int i = 0; i < 4; i++) pthread_create(&threads[i], 0, worker, 0);
    for (int i = 0; i < 4; i++) pthread_join(threads[i], 0);
    printf("AEROS_DT counter=%d\n", counter);
    CHECK("threads-mutex", counter == 4000);
    pthread_t other;
    pthread_create(&other, 0, signaller, 0);
    pthread_mutex_lock(&lock);
    while (!flag) pthread_cond_wait(&ready, &lock);
    pthread_mutex_unlock(&lock);
    pthread_join(other, 0);
    CHECK("threads-condvar", flag == 1);

    // misc
    char random[8];
    CHECK("getrandom", getentropy(random, sizeof random) == 0);
    CHECK("getenv", getenv("HOME") != 0);
    printf("AEROS_DT done failures=%d\n", failures);
    return failures;
}
