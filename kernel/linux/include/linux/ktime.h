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

#endif
