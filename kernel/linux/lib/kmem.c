// SPDX-License-Identifier: MIT
#include <linux/kernel.h>
#include <linux/slab.h>
#include <linux/sched.h>

/* Caches are plain kmalloc with a fixed size. */
struct kmem_cache *kmem_cache_create(const char *name, unsigned int size, unsigned int align,
				     unsigned long flags, void (*ctor)(void *object))
{
	struct kmem_cache *cache = kmalloc(sizeof(*cache), GFP_KERNEL);

	(void)flags;
	if (!cache)
		return NULL;
	if (align > 16)
		pr_warn("kmem_cache %s: alignment %u not honoured\n", name, align);
	cache->name = name;
	cache->size = size;
	cache->ctor = ctor;
	return cache;
}

void *kmem_cache_alloc(struct kmem_cache *cache, gfp_t flags)
{
	void *object = (flags & __GFP_ZERO) ? kzalloc(cache->size, flags) : kmalloc(cache->size, flags);

	if (object && cache->ctor)
		cache->ctor(object);
	return object;
}

void kmem_cache_free(struct kmem_cache *cache, void *object)
{
	(void)cache;
	kfree(object);
}

void kmem_cache_destroy(struct kmem_cache *cache)
{
	kfree(cache);
}

struct task_struct aeros_task = { .pid = 1, .comm = "aeros-linux", .group_leader = &aeros_task };

/* Nothing can wake a sleeper early, so this returns after about a jiffy
 * (callers loop and recheck, as Linux allows) with the time left. */
long schedule_timeout(long timeout)
{
	unsigned long start = jiffies;
	long elapsed;

	if (timeout == MAX_SCHEDULE_TIMEOUT) {
		aeros_wait_step();
		return timeout;
	}
	do {
		aeros_wait_step();
		elapsed = (long)(jiffies - start);
	} while (elapsed < 1 && elapsed < timeout);
	return elapsed >= timeout ? 0 : timeout - elapsed;
}
