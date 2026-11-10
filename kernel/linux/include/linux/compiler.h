#ifndef _LINUX_COMPILER_H
#define _LINUX_COMPILER_H

#include <linux/types.h>
#define __read_mostly
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
