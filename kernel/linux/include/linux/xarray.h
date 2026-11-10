#ifndef _LINUX_XARRAY_H
#define _LINUX_XARRAY_H

#include <linux/kernel.h>
#include <linux/spinlock.h>
#include <linux/rbtree.h>
#include <linux/gfp.h>

/* An index-ordered map kept in a red-black tree; storing NULL erases. */
struct xarray {
	spinlock_t xa_lock;
	gfp_t xa_flags;
	struct rb_root root;
	unsigned long count;
};

struct xa_limit {
	u32 max;
	u32 min;
};
#define XA_LIMIT(lo, hi) (struct xa_limit){ .min = (lo), .max = (hi) }
#define xa_limit_32b XA_LIMIT(0, 0xffffffffu)
#define xa_limit_31b XA_LIMIT(0, 0x7fffffffu)
#define xa_limit_16b XA_LIMIT(0, 0xffffu)

#define XA_FLAGS_LOCK_IRQ 0x1u
#define XA_FLAGS_LOCK_BH 0x2u
#define XA_FLAGS_TRACK_FREE 0x4u
#define XA_FLAGS_ZERO_BUSY 0x8u
#define XA_FLAGS_ALLOC (XA_FLAGS_TRACK_FREE)
#define XA_FLAGS_ALLOC1 (XA_FLAGS_TRACK_FREE | XA_FLAGS_ZERO_BUSY)
#define XA_PRESENT ((unsigned long)-1)

#define XARRAY_INIT(name, flags) { __SPIN_LOCK_UNLOCKED(name.xa_lock), (flags), RB_ROOT, 0 }
#define DEFINE_XARRAY_FLAGS(name, flags) struct xarray name = XARRAY_INIT(name, flags)
#define DEFINE_XARRAY(name) DEFINE_XARRAY_FLAGS(name, 0)
#define DEFINE_XARRAY_ALLOC(name) DEFINE_XARRAY_FLAGS(name, XA_FLAGS_ALLOC)
#define DEFINE_XARRAY_ALLOC1(name) DEFINE_XARRAY_FLAGS(name, XA_FLAGS_ALLOC1)

static inline void *xa_mk_value(unsigned long value) { return (void *)((value << 1) | 1); }
static inline unsigned long xa_to_value(const void *entry) { return (unsigned long)entry >> 1; }
static inline bool xa_is_value(const void *entry) { return (unsigned long)entry & 1; }
static inline void *xa_mk_err(long error) { return (void *)((error << 2) | 2); }
static inline bool xa_is_err(const void *entry) { return ((unsigned long)entry & 3) == 2 && (long)entry >= -4094 * 4; }
static inline int xa_err(void *entry) { return xa_is_err(entry) ? (int)((long)entry >> 2) : 0; }

static inline void xa_init_flags(struct xarray *xa, gfp_t flags)
{
	spin_lock_init(&xa->xa_lock);
	xa->xa_flags = flags;
	xa->root = RB_ROOT;
	xa->count = 0;
}
static inline void xa_init(struct xarray *xa) { xa_init_flags(xa, 0); }
static inline bool xa_empty(const struct xarray *xa) { return xa->count == 0; }

void *xa_load(struct xarray *xa, unsigned long index);
void *__xa_store(struct xarray *xa, unsigned long index, void *entry, gfp_t gfp);
void *__xa_erase(struct xarray *xa, unsigned long index);
int __xa_insert(struct xarray *xa, unsigned long index, void *entry, gfp_t gfp);
void *__xa_cmpxchg(struct xarray *xa, unsigned long index, void *old, void *entry, gfp_t gfp);
int __xa_alloc(struct xarray *xa, u32 *id, void *entry, struct xa_limit limit, gfp_t gfp);
int __xa_alloc_cyclic(struct xarray *xa, u32 *id, void *entry, struct xa_limit limit, u32 *next, gfp_t gfp);
void *xa_find(struct xarray *xa, unsigned long *index, unsigned long max, unsigned long filter);
void *xa_find_after(struct xarray *xa, unsigned long *index, unsigned long max, unsigned long filter);
void xa_destroy(struct xarray *xa);

#define xa_lock(xa) spin_lock(&(xa)->xa_lock)
#define xa_unlock(xa) spin_unlock(&(xa)->xa_lock)
#define xa_lock_irq xa_lock
#define xa_unlock_irq xa_unlock
#define xa_lock_bh xa_lock
#define xa_unlock_bh xa_unlock
#define xa_lock_irqsave(xa, flags) spin_lock_irqsave(&(xa)->xa_lock, flags)
#define xa_unlock_irqrestore(xa, flags) spin_unlock_irqrestore(&(xa)->xa_lock, flags)

#define __AEROS_XA_LOCKED(call) ({ __typeof__(call) __xa_result; xa_lock(__aeros_xa); __xa_result = (call); xa_unlock(__aeros_xa); __xa_result; })
static inline void *xa_store(struct xarray *xa, unsigned long index, void *entry, gfp_t gfp)
{
	void *old;

	xa_lock(xa);
	old = __xa_store(xa, index, entry, gfp);
	xa_unlock(xa);
	return old;
}
static inline void *xa_erase(struct xarray *xa, unsigned long index)
{
	void *old;

	xa_lock(xa);
	old = __xa_erase(xa, index);
	xa_unlock(xa);
	return old;
}
static inline int xa_insert(struct xarray *xa, unsigned long index, void *entry, gfp_t gfp)
{
	int error;

	xa_lock(xa);
	error = __xa_insert(xa, index, entry, gfp);
	xa_unlock(xa);
	return error;
}
static inline void *xa_cmpxchg(struct xarray *xa, unsigned long index, void *old, void *entry, gfp_t gfp)
{
	void *current_entry;

	xa_lock(xa);
	current_entry = __xa_cmpxchg(xa, index, old, entry, gfp);
	xa_unlock(xa);
	return current_entry;
}
static inline int xa_alloc(struct xarray *xa, u32 *id, void *entry, struct xa_limit limit, gfp_t gfp)
{
	int error;

	xa_lock(xa);
	error = __xa_alloc(xa, id, entry, limit, gfp);
	xa_unlock(xa);
	return error;
}
static inline int xa_alloc_cyclic(struct xarray *xa, u32 *id, void *entry, struct xa_limit limit, u32 *next, gfp_t gfp)
{
	int error;

	xa_lock(xa);
	error = __xa_alloc_cyclic(xa, id, entry, limit, next, gfp);
	xa_unlock(xa);
	return error;
}
#define xa_store_irq xa_store
#define xa_store_bh xa_store
#define xa_erase_irq xa_erase
#define xa_erase_bh xa_erase
#define xa_insert_irq xa_insert
#define xa_alloc_irq xa_alloc
#define xa_alloc_bh xa_alloc
#define xa_alloc_cyclic_irq xa_alloc_cyclic
#define xa_release(xa, index) do { (void)(xa); (void)(index); } while (0)
static inline int xa_reserve(struct xarray *xa, unsigned long index, gfp_t gfp) { (void)xa; (void)index; (void)gfp; return 0; }

#define xa_for_each_range(xa, index, entry, start, last) \
	for ((index) = (start), (entry) = xa_find((xa), &(index), (last), XA_PRESENT); (entry); \
	     (entry) = xa_find_after((xa), &(index), (last), XA_PRESENT))
#define xa_for_each_start(xa, index, entry, start) xa_for_each_range(xa, index, entry, start, ULONG_MAX)
#define xa_for_each(xa, index, entry) xa_for_each_start(xa, index, entry, 0)
#define xa_for_each_marked(xa, index, entry, filter) xa_for_each(xa, index, entry)

#endif
