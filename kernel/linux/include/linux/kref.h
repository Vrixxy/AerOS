#ifndef _LINUX_KREF_H
#define _LINUX_KREF_H

#include <linux/refcount.h>

struct kref {
	refcount_t refcount;
};

#define KREF_INIT(n) { .refcount = REFCOUNT_INIT(n) }
static inline void kref_init(struct kref *kref) { atomic_set(&kref->refcount.refs, 1); }
static inline unsigned int kref_read(const struct kref *kref) { return (unsigned int)atomic_read(&kref->refcount.refs); }
static inline void kref_get(struct kref *kref) { atomic_inc(&kref->refcount.refs); }
static inline int kref_put(struct kref *kref, void (*release)(struct kref *kref))
{
	if (atomic_dec_and_test(&kref->refcount.refs)) {
		release(kref);
		return 1;
	}
	return 0;
}
static inline int kref_get_unless_zero(struct kref *kref)
{
	int old = atomic_read(&kref->refcount.refs);

	while (old) {
		int seen = atomic_cmpxchg(&kref->refcount.refs, old, old + 1);

		if (seen == old)
			return 1;
		old = seen;
	}
	return 0;
}
#define kref_put_mutex(kref, release, lock) kref_put(kref, release)

#endif
