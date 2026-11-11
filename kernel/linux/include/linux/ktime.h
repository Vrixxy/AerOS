#ifndef _LINUX_KTIME_H
#define _LINUX_KTIME_H

#include <linux/types.h>


#define NSEC_PER_USEC 1000L
#define NSEC_PER_MSEC 1000000L
#define NSEC_PER_SEC 1000000000L
#define USEC_PER_MSEC 1000L
#define USEC_PER_SEC 1000000L
#define MSEC_PER_SEC 1000L
#define KTIME_MAX ((s64)~((u64)1 << 63))
#define KTIME_SEC_MAX (KTIME_MAX / NSEC_PER_SEC)

u64 ktime_get_ns(void);
static inline ktime_t ktime_get(void) { return (ktime_t)ktime_get_ns(); }
#define ktime_get_raw ktime_get
#define ktime_get_boottime ktime_get
#define ktime_get_real ktime_get
#define ktime_get_raw_ns ktime_get_ns
#define ktime_get_boottime_ns ktime_get_ns
#define ktime_get_mono_fast_ns ktime_get_ns
#define ktime_get_real_ns ktime_get_ns
static inline time64_t ktime_get_seconds(void) { return (time64_t)(ktime_get_ns() / NSEC_PER_SEC); }
#define ktime_get_real_seconds ktime_get_seconds
static inline ktime_t ktime_set(s64 seconds, unsigned long nanoseconds) { return seconds * NSEC_PER_SEC + (s64)nanoseconds; }
static inline s64 ktime_to_ns(ktime_t time) { return time; }
static inline ktime_t ns_to_ktime(u64 nanoseconds) { return (ktime_t)nanoseconds; }
static inline s64 ktime_to_us(ktime_t time) { return time / NSEC_PER_USEC; }
static inline s64 ktime_to_ms(ktime_t time) { return time / NSEC_PER_MSEC; }
static inline ktime_t ms_to_ktime(u64 milliseconds) { return (ktime_t)(milliseconds * NSEC_PER_MSEC); }
static inline ktime_t us_to_ktime(u64 microseconds) { return (ktime_t)(microseconds * NSEC_PER_USEC); }
#define ktime_add(a, b) ((a) + (b))
#define ktime_sub(a, b) ((a) - (b))
#define ktime_add_ns(time, ns) ((time) + (s64)(ns))
#define ktime_sub_ns(time, ns) ((time) - (s64)(ns))
#define ktime_add_us(time, us) ((time) + (s64)(us) * NSEC_PER_USEC)
#define ktime_add_ms(time, ms) ((time) + (s64)(ms) * NSEC_PER_MSEC)
static inline int ktime_compare(ktime_t a, ktime_t b) { return a < b ? -1 : a > b ? 1 : 0; }
static inline bool ktime_after(ktime_t a, ktime_t b) { return a > b; }
static inline bool ktime_before(ktime_t a, ktime_t b) { return a < b; }
static inline s64 ktime_us_delta(ktime_t later, ktime_t earlier) { return ktime_to_us(later - earlier); }
static inline s64 ktime_ms_delta(ktime_t later, ktime_t earlier) { return ktime_to_ms(later - earlier); }
#define ktime_is_after ktime_after

struct timespec64 {
	time64_t tv_sec;
	long tv_nsec;
};
struct timeval {
	long tv_sec;
	long tv_usec;
};
static inline struct timespec64 ns_to_timespec64(s64 nanoseconds)
{
	struct timespec64 ts = { nanoseconds / NSEC_PER_SEC, (long)(nanoseconds % NSEC_PER_SEC) };

	return ts;
}
#define ktime_to_timespec64(kt) ns_to_timespec64(kt)
static inline ktime_t timespec64_to_ktime(struct timespec64 ts) { return ktime_set(ts.tv_sec, (unsigned long)ts.tv_nsec); }
static inline s64 timespec64_to_ns(const struct timespec64 *ts) { return ts->tv_sec * NSEC_PER_SEC + ts->tv_nsec; }
static inline void ktime_get_ts64(struct timespec64 *ts) { *ts = ns_to_timespec64(ktime_get_ns()); }
#define ktime_get_real_ts64 ktime_get_ts64
#define ktime_get_raw_ts64 ktime_get_ts64

/* As in Linux, where ktime.h reaches mutex.h and idr.h through jiffies and seqlock. */
#include <linux/mutex.h>
#include <linux/idr.h>
#include <linux/wait.h>
#include <linux/workqueue.h>

#endif
