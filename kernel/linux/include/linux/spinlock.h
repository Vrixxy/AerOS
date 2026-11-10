#ifndef _LINUX_SPINLOCK_H
#define _LINUX_SPINLOCK_H

#include <linux/types.h>
#include <linux/preempt.h>

/* The Linux side runs on one thread (AerOS's desktop loop) and interrupt
 * handlers run from the same loop, so a lock only has to note that it is
 * held; taking it twice is a bug and is reported. */
struct raw_spinlock {
	int held;
};
typedef struct raw_spinlock spinlock_t;
typedef struct raw_spinlock raw_spinlock_t;

void aeros_lock_twice(const void *lock);

#define __SPIN_LOCK_UNLOCKED(name) { 0 }
#define DEFINE_SPINLOCK(name) spinlock_t name = __SPIN_LOCK_UNLOCKED(name)
static inline void spin_lock_init(spinlock_t *lock) { lock->held = 0; }
static inline void spin_lock(spinlock_t *lock)
{
	if (lock->held)
		aeros_lock_twice(lock);
	lock->held = 1;
}
static inline void spin_unlock(spinlock_t *lock) { lock->held = 0; }
static inline int spin_trylock(spinlock_t *lock)
{
	if (lock->held)
		return 0;
	lock->held = 1;
	return 1;
}
#define spin_lock_irq spin_lock
#define spin_unlock_irq spin_unlock
#define spin_lock_bh spin_lock
#define spin_unlock_bh spin_unlock
#define spin_lock_irqsave(lock, flags) do { (flags) = 0; spin_lock(lock); } while (0)
#define spin_unlock_irqrestore(lock, flags) do { (void)(flags); spin_unlock(lock); } while (0)
#define raw_spin_lock_init spin_lock_init
#define raw_spin_lock spin_lock
#define raw_spin_unlock spin_unlock
#define raw_spin_lock_irqsave spin_lock_irqsave
#define raw_spin_unlock_irqrestore spin_unlock_irqrestore
typedef struct raw_spinlock rwlock_t;
#define __RW_LOCK_UNLOCKED(name) { 0 }
#define DEFINE_RWLOCK(name) rwlock_t name = __RW_LOCK_UNLOCKED(name)
#define rwlock_init spin_lock_init
#define read_lock spin_lock
#define read_unlock spin_unlock
#define write_lock spin_lock
#define write_unlock spin_unlock
#define read_lock_irqsave spin_lock_irqsave
#define read_unlock_irqrestore spin_unlock_irqrestore
#define write_lock_irqsave spin_lock_irqsave
#define write_unlock_irqrestore spin_unlock_irqrestore
#define read_lock_bh spin_lock
#define read_unlock_bh spin_unlock
#define write_lock_bh spin_lock
#define write_unlock_bh spin_unlock
#define write_lock_irq spin_lock
#define write_unlock_irq spin_unlock
#define spin_trylock_irqsave(lock, flags) ({ (flags) = 0; spin_trylock(lock); })
#define spin_lock_nested(lock, subclass) spin_lock(lock)
#define spin_lock_irqsave_nested(lock, flags, subclass) spin_lock_irqsave(lock, flags)
#define spin_is_locked(lock) ((lock)->held != 0)
#define assert_spin_locked(lock) do { (void)(lock); } while (0)
#define lockdep_assert_held(lock) do { (void)(lock); } while (0)

#include <linux/cleanup.h>
DEFINE_GUARD(spinlock, spinlock_t *, spin_lock(_T), spin_unlock(_T))
DEFINE_GUARD(spinlock_irq, spinlock_t *, spin_lock(_T), spin_unlock(_T))
#include <linux/lockdep.h>

#endif
