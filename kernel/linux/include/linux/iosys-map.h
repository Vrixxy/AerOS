#ifndef _LINUX_IOSYS_MAP_H
#define _LINUX_IOSYS_MAP_H

#include <linux/kernel.h>
#include <linux/io.h>

/* A CPU mapping of a buffer that is either RAM or I/O memory. */
struct iosys_map {
	union {
		void __iomem *vaddr_iomem;
		void *vaddr;
	};
	bool is_iomem;
};

#define IOSYS_MAP_INIT_VADDR(vaddr_) { .vaddr = (vaddr_), .is_iomem = false }
#define IOSYS_MAP_INIT_VADDR_IOMEM(vaddr_iomem_) { .vaddr_iomem = (vaddr_iomem_), .is_iomem = true }
#define IOSYS_MAP_INIT_OFFSET(map_, offset_) ({ \
	struct iosys_map __copy = *(map_); \
	iosys_map_incr(&__copy, (offset_)); \
	__copy; })
static inline void iosys_map_set_vaddr(struct iosys_map *map, void *vaddr)
{
	map->vaddr = vaddr;
	map->is_iomem = false;
}
static inline void iosys_map_set_vaddr_iomem(struct iosys_map *map, void __iomem *vaddr_iomem)
{
	map->vaddr_iomem = vaddr_iomem;
	map->is_iomem = true;
}
static inline bool iosys_map_is_equal(const struct iosys_map *a, const struct iosys_map *b)
{
	return a->is_iomem == b->is_iomem && a->vaddr == b->vaddr;
}
static inline bool iosys_map_is_null(const struct iosys_map *map) { return !map->vaddr; }
static inline bool iosys_map_is_set(const struct iosys_map *map) { return !iosys_map_is_null(map); }
static inline void iosys_map_clear(struct iosys_map *map)
{
	map->vaddr = NULL;
	map->is_iomem = false;
}
static inline void iosys_map_incr(struct iosys_map *map, size_t increment)
{
	map->vaddr = (char *)map->vaddr + increment;
}
static inline void iosys_map_memcpy_to(struct iosys_map *dst, size_t offset, const void *src, size_t length)
{
	memcpy((char *)dst->vaddr + offset, src, length);
}
static inline void iosys_map_memcpy_from(void *dst, const struct iosys_map *src, size_t offset, size_t length)
{
	memcpy(dst, (const char *)src->vaddr + offset, length);
}
static inline void iosys_map_memset(struct iosys_map *dst, size_t offset, int value, size_t length)
{
	memset((char *)dst->vaddr + offset, value, length);
}
#define iosys_map_rd(map, offset, type) (*(volatile type *)((char *)(map)->vaddr + (offset)))
#define iosys_map_wr(map, offset, type, value) (*(volatile type *)((char *)(map)->vaddr + (offset)) = (value))
#define iosys_map_rd_field(map, struct_offset, struct_type, field) \
	iosys_map_rd(map, (struct_offset) + offsetof(struct_type, field), __typeof__(((struct_type *)0)->field))
#define iosys_map_wr_field(map, struct_offset, struct_type, field, value) \
	iosys_map_wr(map, (struct_offset) + offsetof(struct_type, field), __typeof__(((struct_type *)0)->field), value)

#endif
