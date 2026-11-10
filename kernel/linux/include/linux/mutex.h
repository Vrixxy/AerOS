#ifndef _LINUX_MUTEX_H
#define _LINUX_MUTEX_H

#include <linux/spinlock.h>
#include <linux/list.h>
#include <linux/atomic.h>
#include <linux/rwsem.h>

/* See spinlock.h: one thread, so a mutex is held or not. */
struct mutex {
	int held;
};

#define __MUTEX_INITIALIZER(name) { 0 }
#define DEFINE_MUTEX(name) struct mutex name = __MUTEX_INITIALIZER(name)
static inline void mutex_init(struct mutex *lock) { lock->held = 0; }
static inline void mutex_destroy(struct mutex *lock) { (void)lock; }
static inline void mutex_lock(struct mutex *lock)
{
	if (lock->held)
		aeros_lock_twice(lock);
	lock->held = 1;
}
static inline int mutex_lock_interruptible(struct mutex *lock) { mutex_lock(lock); return 0; }
static inline int mutex_trylock(struct mutex *lock)
{
	if (lock->held)
		return 0;
	lock->held = 1;
	return 1;
}
static inline void mutex_unlock(struct mutex *lock) { lock->held = 0; }
static inline bool mutex_is_locked(struct mutex *lock) { return lock->held; }
#define mutex_lock_nested(lock, subclass) mutex_lock(lock)

#include <linux/cleanup.h>
DEFINE_GUARD(mutex, struct mutex *, mutex_lock(_T), mutex_unlock(_T))

static inline int atomic_dec_and_mutex_lock(atomic_t *count, struct mutex *lock)
{
	if (atomic_dec_return(count) != 0)
		return 0;
	mutex_lock(lock);
	return 1;
}

#endif
