#ifndef _AEROS_LINUX_GCD_H
#define _AEROS_LINUX_GCD_H

#include <linux/types.h>
static inline unsigned long gcd(unsigned long a, unsigned long b)
{
	while (b) {
		unsigned long t = a % b;

		a = b;
		b = t;
	}
	return a;
}
#endif
