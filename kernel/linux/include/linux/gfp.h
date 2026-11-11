#ifndef _LINUX_GFP_H
#define _LINUX_GFP_H

#include <linux/types.h>

/* Allocation never sleeps or reclaims here; only __GFP_ZERO changes anything. */
#define GFP_KERNEL 0x1u
#define GFP_ATOMIC 0x2u
#define GFP_NOWAIT 0x4u
#define GFP_USER 0x8u
#define GFP_HIGHUSER 0x10u
#define GFP_DMA32 0x20u
#define GFP_NOFS 0x40u
#define GFP_NOIO 0x80u
#define GFP_KERNEL_ACCOUNT GFP_KERNEL
#define __GFP_ZERO 0x100u
#define __GFP_NOWARN 0x200u
#define __GFP_NORETRY 0x400u
#define __GFP_RETRY_MAYFAIL 0x800u
#define __GFP_NOFAIL 0x1000u
#define __GFP_COMP 0x2000u
#define __GFP_DMA32 0x4000u
#define __GFP_HIGHMEM 0x8000u
#define __GFP_RECLAIM 0x10000u
#define __GFP_DIRECT_RECLAIM 0x20000u
#define __GFP_KSWAPD_RECLAIM 0x40000u
#define __GFP_ACCOUNT 0x80000u
#define __GFP_NOMEMALLOC 0x100000u
#define __GFP_THISNODE 0x200000u
#define GFP_HIGHUSER_MOVABLE GFP_HIGHUSER
#define GFP_TRANSHUGE_LIGHT GFP_HIGHUSER

static inline bool gfpflags_allow_blocking(gfp_t gfp) { return !(gfp & (GFP_ATOMIC | GFP_NOWAIT)); }

#endif
