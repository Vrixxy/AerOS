#ifndef _AEROS_LINUX_HRTIMER_H
#define _AEROS_LINUX_HRTIMER_H

#include <linux/ktime.h>
#include <linux/timer.h>
/* High-resolution timers on the shared timer list (millisecond granularity). */
enum hrtimer_mode {
	HRTIMER_MODE_ABS = 0x00,
	HRTIMER_MODE_REL = 0x01,
	HRTIMER_MODE_PINNED = 0x02,
	HRTIMER_MODE_SOFT = 0x04,
	HRTIMER_MODE_HARD = 0x08,
	HRTIMER_MODE_ABS_SOFT = 0x04,
	HRTIMER_MODE_REL_SOFT = 0x05,
	HRTIMER_MODE_REL_PINNED = 0x03,
	HRTIMER_MODE_ABS_HARD = 0x08,
	HRTIMER_MODE_REL_HARD = 0x09,
};
enum hrtimer_restart { HRTIMER_NORESTART, HRTIMER_RESTART };
#define CLOCK_REALTIME 0
#define CLOCK_MONOTONIC 1
struct hrtimer {
	struct timer_list timer;
	ktime_t expires;
	enum hrtimer_restart (*function)(struct hrtimer *);
};
void hrtimer_init(struct hrtimer *timer, int clock_id, enum hrtimer_mode mode);
void hrtimer_setup(struct hrtimer *timer, enum hrtimer_restart (*function)(struct hrtimer *), int clock_id,
		   enum hrtimer_mode mode);
void hrtimer_start(struct hrtimer *timer, ktime_t time, enum hrtimer_mode mode);
#define hrtimer_start_range_ns(timer, time, delta, mode) hrtimer_start((timer), (time), (mode))
int hrtimer_cancel(struct hrtimer *timer);
#define hrtimer_try_to_cancel hrtimer_cancel
static inline bool hrtimer_active(const struct hrtimer *timer) { return timer_pending(&timer->timer); }
#define hrtimer_is_queued hrtimer_active
static inline ktime_t hrtimer_get_expires(const struct hrtimer *timer) { return timer->expires; }
static inline void hrtimer_set_expires(struct hrtimer *timer, ktime_t time) { timer->expires = time; }
static inline ktime_t hrtimer_cb_get_time(struct hrtimer *timer) { (void)timer; return ktime_get(); }
u64 hrtimer_forward(struct hrtimer *timer, ktime_t now, ktime_t interval);
static inline u64 hrtimer_forward_now(struct hrtimer *timer, ktime_t interval)
{
	return hrtimer_forward(timer, ktime_get(), interval);
}
#endif
