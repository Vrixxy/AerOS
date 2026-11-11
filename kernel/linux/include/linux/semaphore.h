#ifndef _AEROS_LINUX_SEMAPHORE_H
#define _AEROS_LINUX_SEMAPHORE_H

#include <linux/wait.h>
/* Counting semaphore; a waiter keeps the Linux side running until it can take one. */
struct semaphore {
	int count;
};
#define __SEMAPHORE_INITIALIZER(name, n) { .count = (n) }
#define DEFINE_SEMAPHORE(name, n) struct semaphore name = __SEMAPHORE_INITIALIZER(name, n)
static inline void sema_init(struct semaphore *sem, int val) { sem->count = val; }
static inline void down(struct semaphore *sem)
{
	while (sem->count <= 0)
		aeros_wait_step();
	sem->count--;
}
static inline int down_interruptible(struct semaphore *sem) { down(sem); return 0; }
static inline int down_killable(struct semaphore *sem) { down(sem); return 0; }
static inline int down_trylock(struct semaphore *sem)
{
	if (sem->count <= 0)
		return 1;
	sem->count--;
	return 0;
}
static inline int down_timeout(struct semaphore *sem, long jiffies_left)
{
	unsigned long end = jiffies + (unsigned long)jiffies_left;

	while (sem->count <= 0) {
		if (time_after_eq(jiffies, end))
			return -ETIME;
		aeros_wait_step();
	}
	sem->count--;
	return 0;
}
static inline void up(struct semaphore *sem) { sem->count++; }
#endif
