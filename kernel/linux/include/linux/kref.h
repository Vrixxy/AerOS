#ifndef _LINUX_KREF_H
#define _LINUX_KREF_H

#include <linux/atomic.h>

struct kref {
	atomic_t refcount;
};

#define KREF_INIT(n) { .refcount = ATOMIC_INIT(n) }
static inline void kref_init(struct kref *kref) { atomic_set(&kref->refcount, 1); }
static inline unsigned int kref_read(const struct kref *kref) { return (unsigned int)atomic_read(&kref->refcount); }
static inline void kref_get(struct kref *kref) { atomic_inc(&kref->refcount); }
static inline int kref_put(struct kref *kref, void (*release)(struct kref *kref))
{
	if (atomic_dec_and_test(&kref->refcount)) {
		release(kref);
		return 1;
	}
	return 0;
}
static inline int kref_get_unless_zero(struct kref *kref)
{
	int old = atomic_read(&kref->refcount);

	while (old) {
		int seen = atomic_cmpxchg(&kref->refcount, old, old + 1);

		if (seen == old)
			return 1;
		old = seen;
	}
	return 0;
}
#define kref_put_mutex(kref, release, lock) kref_put(kref, release)

#endif
