#ifndef _LINUX_FPU_H
#define _LINUX_FPU_H

#include <linux/types.h>

/* The Linux side runs on AerOS's desktop task, whose FPU and SSE state the
 * scheduler saves and restores on every switch, and AerOS's own kernel code
 * never touches those registers: nothing to save here. */
static inline bool kernel_fpu_available(void) { return true; }
static inline void kernel_fpu_begin(void) { }
static inline void kernel_fpu_end(void) { }

#endif
