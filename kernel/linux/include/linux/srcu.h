#ifndef _LINUX_SRCU_H
#define _LINUX_SRCU_H

#include <linux/rcupdate.h>
/* One cooperative thread: like RCU, a grace period is a return to the loop. */
struct srcu_struct {
	int unused;
};
#define DEFINE_STATIC_SRCU(name) static struct srcu_struct name
#define DEFINE_SRCU(name) struct srcu_struct name
static inline int init_srcu_struct(struct srcu_struct *ssp) { (void)ssp; return 0; }
static inline void cleanup_srcu_struct(struct srcu_struct *ssp) { (void)ssp; }
static inline int srcu_read_lock(struct srcu_struct *ssp) { (void)ssp; return 0; }
static inline void srcu_read_unlock(struct srcu_struct *ssp, int index) { (void)ssp; (void)index; }
static inline void synchronize_srcu(struct srcu_struct *ssp) { (void)ssp; }
#define srcu_dereference(p, ssp) READ_ONCE(p)

#endif
