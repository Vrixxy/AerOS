#ifndef _LINUX_RWSEM_H
#define _LINUX_RWSEM_H

#include <linux/spinlock.h>

/* One thread: readers count, a writer is held or not. */
struct rw_semaphore {
	int readers;
	int writer;
};
#define __RWSEM_INITIALIZER(name) { 0, 0 }
#define DECLARE_RWSEM(name) struct rw_semaphore name = __RWSEM_INITIALIZER(name)
static inline void init_rwsem(struct rw_semaphore *sem) { sem->readers = 0; sem->writer = 0; }
static inline void down_read(struct rw_semaphore *sem)
{
	if (sem->writer)
		aeros_lock_twice(sem);
	sem->readers++;
}
static inline int down_read_trylock(struct rw_semaphore *sem)
{
	if (sem->writer)
		return 0;
	sem->readers++;
	return 1;
}
static inline void up_read(struct rw_semaphore *sem) { sem->readers--; }
static inline void down_write(struct rw_semaphore *sem)
{
	if (sem->writer || sem->readers)
		aeros_lock_twice(sem);
	sem->writer = 1;
}
static inline int down_write_trylock(struct rw_semaphore *sem)
{
	if (sem->writer || sem->readers)
		return 0;
	sem->writer = 1;
	return 1;
}
static inline void up_write(struct rw_semaphore *sem) { sem->writer = 0; }
static inline void downgrade_write(struct rw_semaphore *sem) { sem->writer = 0; sem->readers++; }
#define down_read_killable(sem) ({ down_read(sem); 0; })
#define down_write_killable(sem) ({ down_write(sem); 0; })
#define down_read_interruptible down_read_killable
#define rwsem_is_locked(sem) ((sem)->writer || (sem)->readers)
#define lockdep_assert_held_write(lock) do { (void)(lock); } while (0)
#define lockdep_assert_held_read(lock) do { (void)(lock); } while (0)

#endif
