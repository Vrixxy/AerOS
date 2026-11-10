#ifndef _LINUX_JIFFIES_H
#define _LINUX_JIFFIES_H

#include <linux/types.h>

#define HZ 1000

unsigned long aeros_jiffies(void);
#define jiffies aeros_jiffies()

static inline unsigned long msecs_to_jiffies(unsigned int milliseconds) { return milliseconds; }
static inline unsigned long usecs_to_jiffies(unsigned int microseconds) { return (microseconds + 999) / 1000; }
static inline unsigned int jiffies_to_msecs(unsigned long ticks) { return (unsigned int)ticks; }

#define time_after(a, b) ((long)((b) - (a)) < 0)
#define time_before(a, b) time_after(b, a)
#define time_after_eq(a, b) ((long)((a) - (b)) >= 0)
#define time_before_eq(a, b) time_after_eq(b, a)

#define MAX_JIFFY_OFFSET ((long)(~0UL >> 1) - 1)
static inline u64 get_jiffies_64(void) { return jiffies; }
#define jiffies_64 get_jiffies_64()
static inline unsigned long nsecs_to_jiffies(u64 ns) { return (unsigned long)(ns / 1000000); }
static inline u64 nsecs_to_jiffies64(u64 ns) { return ns / 1000000; }
static inline u64 jiffies_to_nsecs(unsigned long ticks) { return (u64)ticks * 1000000; }
static inline unsigned int jiffies_to_usecs(unsigned long ticks) { return (unsigned int)(ticks * 1000); }
#define round_jiffies_up(j) (j)
#define round_jiffies_up_relative(j) (j)
#define time_in_range(a, b, c) (time_after_eq(a, b) && time_before_eq(a, c))

#endif
