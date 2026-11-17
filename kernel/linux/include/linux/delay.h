#ifndef _LINUX_DELAY_H
#define _LINUX_DELAY_H

#include <linux/types.h>

void ndelay(unsigned long nanoseconds);
void udelay(unsigned long microseconds);
void mdelay(unsigned long milliseconds);
void msleep(unsigned int milliseconds);
void usleep_range(unsigned long minimum, unsigned long maximum);

u64 ktime_get_ns(void);

static inline void fsleep(unsigned long microseconds)
{
	if (microseconds <= 10)
		udelay(microseconds);
	else if (microseconds <= 20000)
		usleep_range(microseconds, 2 * microseconds);
	else
		msleep((unsigned int)((microseconds + 999) / 1000));
}

#endif
