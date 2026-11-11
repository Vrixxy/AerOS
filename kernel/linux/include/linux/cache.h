#ifndef _AEROS_LINUX_CACHE_H
#define _AEROS_LINUX_CACHE_H

#include <linux/kernel.h>
#ifndef L1_CACHE_BYTES
#define L1_CACHE_SHIFT 6
#define L1_CACHE_BYTES (1 << L1_CACHE_SHIFT)
#endif
#define SMP_CACHE_BYTES L1_CACHE_BYTES
#define ____cacheline_aligned __attribute__((__aligned__(SMP_CACHE_BYTES)))
#define ____cacheline_aligned_in_smp ____cacheline_aligned
#define __cacheline_aligned ____cacheline_aligned
#endif
