#ifndef _LINUX_COMPILER_H
#define _LINUX_COMPILER_H

#include <linux/types.h>
#define __read_mostly
#define __deprecated
#define ___PASTE(a, b) a##b
#define __PASTE(a, b) ___PASTE(a, b)
#define DECLARE_FLEX_ARRAY(TYPE, NAME) struct { struct { } __empty_##NAME; TYPE NAME[]; }
#define __static_assert(expr, msg, ...) _Static_assert(expr, msg)
#define static_assert(expr, ...) __static_assert(expr, ##__VA_ARGS__, #expr)
#define __counted_by_le(member)
#define __sized_by(member)
#define __pure __attribute__((pure))
#define __weak __attribute__((weak))
#define __noreturn __attribute__((noreturn))
#define __used __attribute__((used))
#define __cold __attribute__((cold))
#define __printf(a, b) __attribute__((format(printf, a, b)))
#define __must_be_array(a) 0
#define __same_type(a, b) __builtin_types_compatible_p(__typeof__(a), __typeof__(b))
#define fallthrough __attribute__((fallthrough))
#define __counted_by(m)
#define __nonstring
#define __randomize_layout
#define __rcu
#define unreachable() __builtin_unreachable()

#endif
