#ifndef _LINUX_MATH64_H
#define _LINUX_MATH64_H

#include <linux/types.h>

static inline u64 div_u64_rem(u64 dividend, u32 divisor, u32 *remainder)
{
	*remainder = (u32)(dividend % divisor);
	return dividend / divisor;
}
static inline u64 div64_u64_rem(u64 dividend, u64 divisor, u64 *remainder)
{
	*remainder = dividend % divisor;
	return dividend / divisor;
}
static inline u64 div_u64(u64 dividend, u32 divisor) { return dividend / divisor; }
static inline s64 div_s64(s64 dividend, s32 divisor) { return dividend / divisor; }
static inline u64 div64_u64(u64 dividend, u64 divisor) { return dividend / divisor; }
static inline s64 div64_s64(s64 dividend, s64 divisor) { return dividend / divisor; }
#define do_div(n, base) ({ u32 __rem = (u32)((n) % (base)); (n) /= (base); __rem; })
#define DIV_ROUND_UP_ULL(ll, d) div64_u64((ll) + (d) - 1, (d))
#define mul_u32_u32(a, b) ((u64)(a) * (b))

#endif
