#ifndef _LINUX_KERNEL_H
#define _LINUX_KERNEL_H

#include <linux/types.h>
#include <linux/printk.h>
#include <linux/errno.h>
#include <linux/bitops.h>
#include <linux/compiler.h>
#include <linux/limits.h>
#include <linux/math64.h>
#include <linux/string.h>
#include <linux/export.h>
#include <linux/slab.h>
#include <linux/bug.h>
#include <linux/preempt.h>
#include <linux/sizes.h>
#include <linux/stringify.h>
#include <linux/rcupdate.h>

#define _THIS_IP_ ({ __label__ __here; __here: (unsigned long)&&__here; })
#define _RET_IP_ ((unsigned long)__builtin_return_address(0))
#define BUILD_BUG_ON_INVALID(e) ((void)(sizeof((__force long)(e))))
#define BUILD_BUG_ON_ZERO(e) ((int)(sizeof(struct { int:(-!!(e)); })))

#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define min(a, b) ((a) < (b) ? (a) : (b))
#define max(a, b) ((a) > (b) ? (a) : (b))
#define min_t(t, a, b) ((t)(a) < (t)(b) ? (t)(a) : (t)(b))
#define max_t(t, a, b) ((t)(a) > (t)(b) ? (t)(a) : (t)(b))
#define clamp(v, lo, hi) min(max(v, lo), hi)
#define DIV_ROUND_UP(n, d) (((n) + (d) - 1) / (d))
#define ALIGN(x, a) (((x) + ((a) - 1)) & ~((__typeof__(x))(a) - 1))
#include <linux/bits.h>
#define __round_mask(x, y) ((__typeof__(x))((y) - 1))
#define round_up(x, y) ((((x) - 1) | __round_mask(x, y)) + 1)
#define round_down(x, y) ((x) & ~__round_mask(x, y))
#define roundup(x, y) ((((x) + ((y) - 1)) / (y)) * (y))
#define rounddown(x, y) ((x) - ((x) % (y)))
#define IS_ALIGNED(x, a) (((x) & ((__typeof__(x))(a) - 1)) == 0)
#define ALIGN_DOWN(x, a) ((x) & ~((__typeof__(x))(a) - 1))
#define PTR_ALIGN(p, a) ((__typeof__(p))ALIGN((unsigned long)(p), (a)))
#define DIV_ROUND_CLOSEST(x, d) (((x) + ((d) / 2)) / (d))
#define DIV_ROUND_CLOSEST_ULL(x, d) DIV_ROUND_CLOSEST((unsigned long long)(x), (unsigned long long)(d))
#define clamp_t(t, v, lo, hi) min_t(t, max_t(t, v, lo), hi)
#define clamp_val(v, lo, hi) clamp_t(__typeof__(v), v, lo, hi)
#define min3(a, b, c) min(min(a, b), c)
#define max3(a, b, c) max(max(a, b), c)
#define swap(a, b) do { __typeof__(a) __swap_tmp = (a); (a) = (b); (b) = __swap_tmp; } while (0)
#define lower_32_bits(n) ((u32)((n) & 0xffffffff))
#define upper_32_bits(n) ((u32)(((n) >> 16) >> 16))
#ifndef offsetof
#define offsetof(t, m) __builtin_offsetof(t, m)
#endif
#define container_of(ptr, type, member) \
	((type *)((char *)(ptr) - offsetof(type, member)))
#define BUILD_BUG_ON(c) _Static_assert(!(c), #c)
#define WARN_ON(c) ({ int __w = !!(c); if (__w) pr_warn("WARN_ON(%s) at %s:%d\n", #c, __FILE__, __LINE__); __w; })
#define WARN_ON_ONCE(c) WARN_ON(c)
#define BUG_ON(c) do { if (c) aeros_bug(__FILE__, __LINE__); } while (0)
#define BUG() aeros_bug(__FILE__, __LINE__)

void aeros_bug(const char *file, int line) __attribute__((noreturn));

#endif
