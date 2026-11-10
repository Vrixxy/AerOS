#ifndef _LINUX_BUG_H
#define _LINUX_BUG_H

#include <linux/kernel.h>
#define WARN(cond, fmt, ...) ({ int __w = !!(cond); if (__w) printk(KERN_WARNING fmt, ##__VA_ARGS__); __w; })
#define WARN_ONCE(cond, fmt, ...) ({ 	static bool __warned; 	int __w = !!(cond); 	if (__w && !__warned) { __warned = true; printk(KERN_WARNING fmt, ##__VA_ARGS__); } 	__w; })
#define BUILD_BUG_ON_MSG(c, msg) _Static_assert(!(c), msg)
#define BUILD_BUG() do { } while (0)

#endif
