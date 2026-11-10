#ifndef _LINUX_SEQLOCK_H
#define _LINUX_SEQLOCK_H

#include <linux/atomic.h>
#include <linux/spinlock.h>

typedef struct {
	unsigned int sequence;
} seqcount_t;
typedef seqcount_t seqcount_ww_mutex_t;
typedef seqcount_t seqcount_spinlock_t;
typedef seqcount_t seqcount_mutex_t;
typedef seqcount_t seqcount_raw_spinlock_t;

#define SEQCNT_ZERO(name) { 0 }
#define seqcount_init(s) ((s)->sequence = 0)
#define seqcount_ww_mutex_init(s, lock) seqcount_init(s)
#define seqcount_spinlock_init(s, lock) seqcount_init(s)
#define seqcount_mutex_init(s, lock) seqcount_init(s)
static inline unsigned int raw_read_seqcount(const seqcount_t *s)
{
	unsigned int sequence = READ_ONCE(s->sequence);

	smp_rmb();
	return sequence;
}
#define read_seqcount_begin raw_read_seqcount
#define raw_read_seqcount_begin raw_read_seqcount
#define __read_seqcount_begin raw_read_seqcount
static inline int read_seqcount_retry(const seqcount_t *s, unsigned int start)
{
	smp_rmb();
	return READ_ONCE(s->sequence) != start;
}
#define __read_seqcount_retry read_seqcount_retry
static inline void write_seqcount_begin(seqcount_t *s)
{
	s->sequence++;
	smp_wmb();
}
static inline void write_seqcount_end(seqcount_t *s)
{
	smp_wmb();
	s->sequence++;
}
#define raw_write_seqcount_begin write_seqcount_begin
#define raw_write_seqcount_end write_seqcount_end
#define write_seqcount_invalidate(s) do { (s)->sequence += 2; } while (0)

typedef struct {
	seqcount_t seqcount;
	spinlock_t lock;
} seqlock_t;
#define seqlock_init(sl) do { seqcount_init(&(sl)->seqcount); spin_lock_init(&(sl)->lock); } while (0)
#define DEFINE_SEQLOCK(name) seqlock_t name = { SEQCNT_ZERO(name), __SPIN_LOCK_UNLOCKED(name) }
#define read_seqbegin(sl) read_seqcount_begin(&(sl)->seqcount)
#define read_seqretry(sl, start) read_seqcount_retry(&(sl)->seqcount, (start))
#define write_seqlock(sl) do { spin_lock(&(sl)->lock); write_seqcount_begin(&(sl)->seqcount); } while (0)
#define write_sequnlock(sl) do { write_seqcount_end(&(sl)->seqcount); spin_unlock(&(sl)->lock); } while (0)

#endif
