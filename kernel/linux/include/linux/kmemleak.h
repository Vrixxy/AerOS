#ifndef _LINUX_KMEMLEAK_H
#define _LINUX_KMEMLEAK_H
#define kmemleak_update_trace(ptr) do { (void)(ptr); } while (0)
#define kmemleak_not_leak(ptr) do { (void)(ptr); } while (0)
#define kmemleak_ignore(ptr) do { (void)(ptr); } while (0)
#define kmemleak_alloc(ptr, size, min_count, gfp) do { } while (0)
#define kmemleak_free(ptr) do { } while (0)
#endif
