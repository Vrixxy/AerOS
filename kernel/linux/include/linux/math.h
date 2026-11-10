#ifndef _LINUX_MATH_H
#define _LINUX_MATH_H

#include <linux/kernel.h>
#define mult_frac(x, numer, denom) ({ \
	__typeof__(x) __q = (x) / (denom); \
	__typeof__(x) __r = (x) % (denom); \
	(__q * (numer)) + ((__r * (numer)) / (denom)); })
#define abs(x) ({ __typeof__(x) __a = (x); __a < 0 ? -__a : __a; })

#endif
