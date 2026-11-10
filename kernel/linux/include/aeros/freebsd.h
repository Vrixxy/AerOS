#ifndef _AEROS_FREEBSD_H
#define _AEROS_FREEBSD_H

/* The few FreeBSD kernel calls drm-kmod's BSD-licensed rewrites use. */
#include <linux/slab.h>

#define M_NOWAIT 0x0001
#define M_WAITOK 0x0002
#define M_ZERO 0x0100
static inline void *aeros_bsd_malloc(size_t size, int flags)
{
	return (flags & M_ZERO) ? kzalloc(size, GFP_KERNEL) : kmalloc(size, GFP_KERNEL);
}
#define malloc(size, type, flags) aeros_bsd_malloc((size), (flags))
#define mallocarray(count, size, type, flags) aeros_bsd_malloc((count) * (size), (flags))
#define free(pointer, type) kfree(pointer)
#define MPASS(condition) WARN_ON(!(condition))
#define KASSERT(condition, message) WARN_ON(!(condition))

#endif
