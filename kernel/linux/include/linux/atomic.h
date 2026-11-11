#ifndef _LINUX_ATOMIC_H
#define _LINUX_ATOMIC_H

#include <linux/types.h>

#define ATOMIC_INIT(i) { (i) }
#define ATOMIC64_INIT(i) { (i) }
#define xchg(ptr, value) __atomic_exchange_n((ptr), (value), __ATOMIC_SEQ_CST)
#define cmpxchg(ptr, old, new_value) 	({ __typeof__(*(ptr)) __cmpxchg_old = (old); 	   __atomic_compare_exchange_n((ptr), &__cmpxchg_old, (new_value), false, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST); 	   __cmpxchg_old; })
#define try_cmpxchg(ptr, oldp, new_value) 	__atomic_compare_exchange_n((ptr), (oldp), (new_value), false, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST)
#define cmpxchg64 cmpxchg
#define cmpxchg_relaxed cmpxchg

static inline int atomic_read(const atomic_t *v) { return __atomic_load_n(&v->counter, __ATOMIC_RELAXED); }
static inline void atomic_set(atomic_t *v, int i) { __atomic_store_n(&v->counter, i, __ATOMIC_RELAXED); }
static inline int atomic_add_return(int i, atomic_t *v) { return __atomic_add_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int atomic_sub_return(int i, atomic_t *v) { return __atomic_sub_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline void atomic_add(int i, atomic_t *v) { atomic_add_return(i, v); }
static inline void atomic_sub(int i, atomic_t *v) { atomic_sub_return(i, v); }
static inline void atomic_inc(atomic_t *v) { atomic_add(1, v); }
static inline void atomic_dec(atomic_t *v) { atomic_sub(1, v); }
static inline int atomic_inc_return(atomic_t *v) { return atomic_add_return(1, v); }
static inline int atomic_dec_return(atomic_t *v) { return atomic_sub_return(1, v); }
static inline bool atomic_dec_and_test(atomic_t *v) { return atomic_dec_return(v) == 0; }
static inline int atomic_xchg(atomic_t *v, int i) { return __atomic_exchange_n(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int atomic_cmpxchg(atomic_t *v, int old, int new_value)
{
	__atomic_compare_exchange_n(&v->counter, &old, new_value, false, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);
	return old;
}

static inline s64 atomic64_read(const atomic64_t *v) { return __atomic_load_n(&v->counter, __ATOMIC_RELAXED); }
static inline void atomic64_set(atomic64_t *v, s64 i) { __atomic_store_n(&v->counter, i, __ATOMIC_RELAXED); }
static inline s64 atomic64_add_return(s64 i, atomic64_t *v) { return __atomic_add_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline s64 atomic64_inc_return(atomic64_t *v) { return atomic64_add_return(1, v); }
static inline s64 atomic64_fetch_add(s64 i, atomic64_t *v) { return __atomic_fetch_add(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline void atomic64_add(s64 i, atomic64_t *v) { atomic64_add_return(i, v); }
static inline void atomic64_sub(s64 i, atomic64_t *v) { atomic64_add_return(-i, v); }
static inline void atomic64_inc(atomic64_t *v) { atomic64_add(1, v); }
static inline void atomic64_dec(atomic64_t *v) { atomic64_sub(1, v); }
static inline s64 atomic64_xchg(atomic64_t *v, s64 i) { return __atomic_exchange_n(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int atomic_fetch_add(int i, atomic_t *v) { return __atomic_fetch_add(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int atomic_fetch_or(int i, atomic_t *v) { return __atomic_fetch_or(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int atomic_fetch_and(int i, atomic_t *v) { return __atomic_fetch_and(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int atomic_fetch_inc(atomic_t *v) { return atomic_fetch_add(1, v); }
static inline int atomic_fetch_dec(atomic_t *v) { return atomic_fetch_add(-1, v); }
static inline bool atomic_inc_not_zero(atomic_t *v)
{
	int old = atomic_read(v);

	while (old && !__atomic_compare_exchange_n(&v->counter, &old, old + 1, false, __ATOMIC_SEQ_CST, __ATOMIC_RELAXED))
		;
	return old != 0;
}
#define atomic_read_acquire(v) __atomic_load_n(&(v)->counter, __ATOMIC_ACQUIRE)
#define atomic_set_release(v, i) __atomic_store_n(&(v)->counter, (i), __ATOMIC_RELEASE)
typedef struct {
	long counter;
} atomic_long_t;
#define ATOMIC_LONG_INIT(i) { (i) }
static inline long atomic_long_read(const atomic_long_t *v) { return __atomic_load_n(&v->counter, __ATOMIC_RELAXED); }
static inline void atomic_long_set(atomic_long_t *v, long i) { __atomic_store_n(&v->counter, i, __ATOMIC_RELAXED); }
static inline long atomic_long_add_return(long i, atomic_long_t *v) { return __atomic_add_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline void atomic_long_add(long i, atomic_long_t *v) { atomic_long_add_return(i, v); }
static inline void atomic_long_sub(long i, atomic_long_t *v) { atomic_long_add_return(-i, v); }
static inline void atomic_long_inc(atomic_long_t *v) { atomic_long_add(1, v); }
static inline void atomic_long_dec(atomic_long_t *v) { atomic_long_sub(1, v); }
static inline long atomic_long_inc_return(atomic_long_t *v) { return atomic_long_add_return(1, v); }
static inline long atomic_long_dec_return(atomic_long_t *v) { return atomic_long_add_return(-1, v); }
static inline long atomic_long_xchg(atomic_long_t *v, long i) { return __atomic_exchange_n(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline long atomic_long_cmpxchg(atomic_long_t *v, long old, long new_value)
{
	__atomic_compare_exchange_n(&v->counter, &old, new_value, false, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);
	return old;
}
#define smp_store_release(p, v) __atomic_store_n((p), (v), __ATOMIC_RELEASE)
#define smp_load_acquire(p) __atomic_load_n((p), __ATOMIC_ACQUIRE)
#define smp_mb__before_atomic() barrier()
#define smp_mb__after_atomic() barrier()
#define smp_wmb__after_atomic smp_mb__after_atomic
#define dma_wmb() barrier()
#define dma_rmb() barrier()
#define cpu_relax() __asm__ volatile("pause" ::: "memory")

#define READ_ONCE(x) (*(const volatile __typeof__(x) *)&(x))
#define WRITE_ONCE(x, v) (*(volatile __typeof__(x) *)&(x) = (v))
#define smp_mb() __atomic_thread_fence(__ATOMIC_SEQ_CST)
#define smp_rmb() __atomic_thread_fence(__ATOMIC_ACQUIRE)
#define smp_wmb() __atomic_thread_fence(__ATOMIC_RELEASE)
#define mb() __asm__ volatile("mfence" ::: "memory")
#define rmb() __asm__ volatile("lfence" ::: "memory")
#define wmb() __asm__ volatile("sfence" ::: "memory")
#define barrier() __asm__ volatile("" ::: "memory")

#include <linux/refcount.h>

#endif
