/*
 * mmap-test: exercises file-backed mmap in the Linux guest (page-cache faults
 * through the emulated virtio-blk root, copy-on-write private mappings, and a
 * shared writable mapping on tmpfs). Prints AEROS_LINUX_MMAP_OK on success.
 */
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

static int fail(const char *what) {
    printf("AEROS_LINUX_MMAP_FAIL %s\n", what);
    return 1;
}

int main(void) {
    /* 1. Read-only private mapping of a real file on the root disk. */
    int fd = open("/proc/self/exe", O_RDONLY);
    if (fd < 0) return fail("open exe");
    struct stat st;
    if (fstat(fd, &st) || st.st_size < 8192) return fail("stat exe");
    size_t len = (size_t)st.st_size;
    unsigned char *map = mmap(NULL, len, PROT_READ, MAP_PRIVATE, fd, 0);
    if (map == MAP_FAILED) return fail("mmap ro");
    unsigned char *buf = malloc(len);
    if (!buf || pread(fd, buf, len, 0) != (ssize_t)len) return fail("pread");
    if (memcmp(map, buf, len)) return fail("ro contents differ");
    munmap(map, len);

    /* 2. Private writable mapping: writes are copy-on-write, file unchanged. */
    map = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    if (map == MAP_FAILED) return fail("mmap cow");
    unsigned char first = map[0];
    map[0] = (unsigned char)~first;
    if (map[0] == first) return fail("cow write");
    unsigned char again;
    if (pread(fd, &again, 1, 0) != 1 || again != first) return fail("cow leaked to file");
    munmap(map, len);
    close(fd);

    /* 3. Shared writable mapping of a tmpfs file, visible through read(). */
    const char *path = "/tmp/aeros-mmap-test";
    fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0 || ftruncate(fd, 3 * 4096)) return fail("tmp create");
    map = mmap(NULL, 3 * 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (map == MAP_FAILED) return fail("mmap shared");
    for (size_t i = 0; i < 3 * 4096; i++) map[i] = (unsigned char)(i * 7 + 1);
    if (msync(map, 3 * 4096, MS_SYNC)) return fail("msync");
    unsigned char page[4096];
    if (pread(fd, page, sizeof page, 4096) != (ssize_t)sizeof page) return fail("pread shared");
    for (size_t i = 0; i < sizeof page; i++)
        if (page[i] != (unsigned char)((i + 4096) * 7 + 1)) return fail("shared contents");
    munmap(map, 3 * 4096);
    close(fd);
    unlink(path);
    free(buf);
    printf("AEROS_LINUX_MMAP_OK exe=%zu bytes\n", len);
    return 0;
}
