#ifndef _LINUX_IOPOLL_H
#define _LINUX_IOPOLL_H

#include <linux/ktime.h>
#include <linux/delay.h>

/* Poll op(args) into val until cond or the timeout passes; the wait keeps the
 * Linux side running between reads. */
#define read_poll_timeout(op, val, cond, sleep_us, timeout_us, sleep_before_read, args...) ({ \
	u64 __timeout_us = (timeout_us); \
	unsigned long __sleep_us = (sleep_us); \
	ktime_t __timeout = ktime_add_us(ktime_get(), __timeout_us); \
	if ((sleep_before_read) && __sleep_us) \
		udelay(__sleep_us); \
	for (;;) { \
		(val) = op(args); \
		if (cond) \
			break; \
		if (__timeout_us && ktime_compare(ktime_get(), __timeout) > 0) { \
			(val) = op(args); \
			break; \
		} \
		if (__sleep_us) \
			udelay(__sleep_us); \
		cpu_relax(); \
	} \
	(cond) ? 0 : -ETIMEDOUT; })
#define read_poll_timeout_atomic read_poll_timeout
#define readx_poll_timeout(op, addr, val, cond, sleep_us, timeout_us) \
	read_poll_timeout(op, val, cond, sleep_us, timeout_us, false, addr)
#define readl_poll_timeout(addr, val, cond, delay_us, timeout_us) \
	readx_poll_timeout(readl, addr, val, cond, delay_us, timeout_us)
#define readl_poll_timeout_atomic readl_poll_timeout

#endif
