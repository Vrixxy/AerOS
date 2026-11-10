#ifndef _LINUX_OVERFLOW_H
#define _LINUX_OVERFLOW_H

#include <linux/types.h>

#define check_add_overflow(a, b, d) __builtin_add_overflow(a, b, d)
#define check_sub_overflow(a, b, d) __builtin_sub_overflow(a, b, d)
#define check_mul_overflow(a, b, d) __builtin_mul_overflow(a, b, d)
static inline size_t size_mul(size_t a, size_t b)
{
	size_t bytes;

	return __builtin_mul_overflow(a, b, &bytes) ? (size_t)-1 : bytes;
}
static inline size_t size_add(size_t a, size_t b)
{
	size_t bytes;

	return __builtin_add_overflow(a, b, &bytes) ? (size_t)-1 : bytes;
}
#define array_size(a, b) size_mul(a, b)
#define array3_size(a, b, c) size_mul(size_mul(a, b), c)
#define flex_array_size(p, member, count) size_mul(count, sizeof(*(p)->member))
#define struct_size(p, member, count) size_add(sizeof(*(p)), flex_array_size(p, member, count))
#define struct_size_t(type, member, count) struct_size((type *)NULL, member, count)

#endif
