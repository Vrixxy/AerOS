#ifndef _LINUX_REFCOUNT_H
#define _LINUX_REFCOUNT_H

#include <linux/atomic.h>

typedef struct refcount_struct {
	atomic_t refs;
} refcount_t;
#define REFCOUNT_INIT(n) { .refs = ATOMIC_INIT(n) }
static inline void refcount_set(refcount_t *r, int n) { atomic_set(&r->refs, n); }
static inline unsigned int refcount_read(const refcount_t *r) { return (unsigned int)atomic_read(&r->refs); }
static inline void refcount_inc(refcount_t *r) { atomic_inc(&r->refs); }
static inline void refcount_add(int n, refcount_t *r) { atomic_add(n, &r->refs); }
static inline bool refcount_inc_not_zero(refcount_t *r) { return atomic_inc_not_zero(&r->refs); }
static inline bool refcount_dec_and_test(refcount_t *r) { return atomic_dec_and_test(&r->refs); }
static inline void refcount_dec(refcount_t *r) { atomic_dec(&r->refs); }
static inline bool refcount_sub_and_test(int n, refcount_t *r) { return atomic_sub_return(n, &r->refs) == 0; }

#endif
