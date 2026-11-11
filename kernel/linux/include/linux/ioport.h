#ifndef _LINUX_IOPORT_H
#define _LINUX_IOPORT_H

#include <linux/types.h>

struct resource {
	resource_size_t start;
	resource_size_t end;
	const char *name;
	unsigned long flags;
	struct resource *parent, *sibling, *child;
};
extern struct resource iomem_resource;
#define IORESOURCE_IO 0x00000100ul
#define IORESOURCE_MEM 0x00000200ul
#define IORESOURCE_IRQ 0x00000400ul
#define IORESOURCE_PREFETCH 0x00002000ul
#define IORESOURCE_MEM_64 0x00100000ul
#define IORESOURCE_UNSET 0x20000000ul
#define DEFINE_RES_MEM(_start, _size) 	((struct resource){ .start = (_start), .end = (_start) + (_size) - 1, .flags = IORESOURCE_MEM })
static inline resource_size_t resource_size(const struct resource *res) { return res->end - res->start + 1; }
static inline unsigned long resource_type(const struct resource *res) { return res->flags & 0x1f00ul; }
static inline bool resource_contains(const struct resource *a, const struct resource *b)
{
	return a->start <= b->start && a->end >= b->end;
}
#define request_mem_region(start, n, name) ((struct resource *)1)
#define release_mem_region(start, n) do { } while (0)

#define IORESOURCE_ROM_SHADOW (1 << 1)

#endif
