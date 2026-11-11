#ifndef _AEROS_LINUX_RATELIMIT_H
#define _AEROS_LINUX_RATELIMIT_H

#include <linux/types.h>
#include <linux/jiffies.h>
#include <linux/spinlock_types.h>
#define DEFAULT_RATELIMIT_INTERVAL (5 * HZ)
#define DEFAULT_RATELIMIT_BURST 10
#define RATELIMIT_MSG_ON_RELEASE 1
struct ratelimit_state {
	int interval;
	int burst;
	int printed;
	int missed;
	unsigned long begin;
	unsigned long flags;
	spinlock_t lock;
};
#define RATELIMIT_STATE_INIT(name, interval_init, burst_init) { .interval = (interval_init), .burst = (burst_init) }
#define DEFINE_RATELIMIT_STATE(name, interval_init, burst_init) \
	struct ratelimit_state name = RATELIMIT_STATE_INIT(name, interval_init, burst_init)
static inline void ratelimit_state_init(struct ratelimit_state *rs, int interval, int burst)
{
	rs->interval = interval;
	rs->burst = burst;
	rs->printed = 0;
	rs->missed = 0;
	rs->begin = 0;
	rs->flags = 0;
}
static inline void ratelimit_set_flags(struct ratelimit_state *rs, unsigned long flags) { rs->flags = flags; }
static inline void ratelimit_state_exit(struct ratelimit_state *rs) { (void)rs; }
static inline int ___ratelimit(struct ratelimit_state *rs, const char *func)
{
	(void)func;
	if (!rs->interval)
		return 1;
	if (!rs->begin || time_after_eq(jiffies, rs->begin + (unsigned long)rs->interval)) {
		rs->begin = jiffies;
		rs->printed = 0;
	}
	if (rs->burst && rs->burst > rs->printed) {
		rs->printed++;
		return 1;
	}
	rs->missed++;
	return 0;
}
#define __ratelimit(state) ___ratelimit(state, __func__)
#define printk_ratelimit() ({ static DEFINE_RATELIMIT_STATE(__rs, DEFAULT_RATELIMIT_INTERVAL, DEFAULT_RATELIMIT_BURST); __ratelimit(&__rs); })
#define printk_ratelimited(fmt, ...) ({ if (printk_ratelimit()) printk(fmt, ##__VA_ARGS__); })
#endif
