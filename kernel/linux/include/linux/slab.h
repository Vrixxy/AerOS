#ifndef _LINUX_SLAB_H
#define _LINUX_SLAB_H

#include <linux/types.h>

#include <linux/gfp.h>
#include <linux/overflow.h>

void *kmalloc(size_t size, gfp_t flags);
void *kzalloc(size_t size, gfp_t flags);
void *kcalloc(size_t count, size_t size, gfp_t flags);
void *krealloc(const void *pointer, size_t size, gfp_t flags);
void kfree(const void *pointer);
size_t ksize(const void *pointer);
static inline size_t kmalloc_size_roundup(size_t size) { return size; }
static inline void *krealloc_array(void *pointer, size_t count, size_t size, gfp_t flags)
{
	size_t bytes;

	if (__builtin_mul_overflow(count, size, &bytes))
		return NULL;
	return krealloc(pointer, bytes, flags);
}

struct kmem_cache {
	const char *name;
	unsigned int size;
	void (*ctor)(void *object);
};
#define SLAB_HWCACHE_ALIGN 0x1ul
#define SLAB_RECLAIM_ACCOUNT 0x2ul
#define SLAB_PANIC 0x4ul
#define SLAB_ACCOUNT 0x8ul
#define SLAB_TYPESAFE_BY_RCU 0x10ul
struct kmem_cache *kmem_cache_create(const char *name, unsigned int size, unsigned int align,
				     unsigned long flags, void (*ctor)(void *object));
void *kmem_cache_alloc(struct kmem_cache *cache, gfp_t flags);
void kmem_cache_free(struct kmem_cache *cache, void *object);
void kmem_cache_destroy(struct kmem_cache *cache);
#define kmem_cache_zalloc(cache, flags) kmem_cache_alloc((cache), (flags) | __GFP_ZERO)
#define KMEM_CACHE(type, flags) 	kmem_cache_create(#type, sizeof(struct type), __alignof__(struct type), (flags), NULL)

#define kmalloc_array(n, size, flags) kmalloc((n) * (size), flags)
#define vmalloc(size) kmalloc(size, GFP_KERNEL)
#define vzalloc(size) kzalloc(size, GFP_KERNEL)
#define vfree(pointer) kfree(pointer)
#define kvmalloc(size, flags) kmalloc(size, flags)
#define kvzalloc(size, flags) kzalloc(size, flags)
#define kvfree(pointer) kfree(pointer)
#define kvcalloc(count, size, flags) kcalloc((count), (size), (flags))
#define kvmalloc_array(count, size, flags) kmalloc_array((count), (size), (flags))
#define kvrealloc(pointer, size, flags) krealloc((pointer), (size), (flags))
#define kvfree_sensitive(pointer, size) kfree(pointer)
#define kfree_sensitive(pointer) kfree(pointer)

#endif
