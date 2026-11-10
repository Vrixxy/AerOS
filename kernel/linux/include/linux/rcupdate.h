#ifndef _LINUX_RCUPDATE_H
#define _LINUX_RCUPDATE_H

#include <linux/atomic.h>

/* One cooperative thread: read sections are free, and a grace period has
 * passed once control is back in the desktop loop, which is where queued
 * callbacks run (aeros_linux_poll). */
struct rcu_head {
	struct rcu_head *next;
	void (*func)(struct rcu_head *head);
};
#define callback_head rcu_head
typedef void (*rcu_callback_t)(struct rcu_head *head);

void call_rcu(struct rcu_head *head, rcu_callback_t func);
void rcu_barrier(void);
void aeros_kfree_rcu(struct rcu_head *head, unsigned long offset);
#define kfree_rcu(pointer, field) \
	aeros_kfree_rcu(&(pointer)->field, __builtin_offsetof(__typeof__(*(pointer)), field))
#define kvfree_rcu kfree_rcu
static inline void synchronize_rcu(void) { }
#define synchronize_rcu_expedited synchronize_rcu
#define rcu_read_lock() do { } while (0)
#define rcu_read_unlock() do { } while (0)
#define rcu_read_lock_held() 1
#define rcu_dereference(p) READ_ONCE(p)
#define rcu_dereference_raw(p) READ_ONCE(p)
#define rcu_dereference_check(p, c) READ_ONCE(p)
#define rcu_dereference_protected(p, c) (p)
#define rcu_access_pointer(p) READ_ONCE(p)
#define rcu_assign_pointer(p, v) WRITE_ONCE(p, v)
#define RCU_INIT_POINTER(p, v) ((p) = (v))
#define RCU_INITIALIZER(v) (v)
#define rcu_pointer_handoff(p) (p)
#define rcu_replace_pointer(rcu_ptr, ptr, c) \
	({ __typeof__(ptr) __rcu_old = (rcu_ptr); (rcu_ptr) = (ptr); __rcu_old; })
#define init_rcu_head(head) do { } while (0)
#define destroy_rcu_head(head) do { } while (0)

#endif
