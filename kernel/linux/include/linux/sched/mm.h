#ifndef _LINUX_SCHED_MM_H
#define _LINUX_SCHED_MM_H

#include <linux/gfp.h>

/* No reclaim exists, so these scopes have nothing to change. */
static inline unsigned int memalloc_noreclaim_save(void) { return 0; }
static inline void memalloc_noreclaim_restore(unsigned int flags) { (void)flags; }
#define memalloc_nofs_save memalloc_noreclaim_save
#define memalloc_nofs_restore memalloc_noreclaim_restore
#define memalloc_noio_save memalloc_noreclaim_save
#define memalloc_noio_restore memalloc_noreclaim_restore
static inline void fs_reclaim_acquire(gfp_t gfp) { (void)gfp; }
static inline void fs_reclaim_release(gfp_t gfp) { (void)gfp; }
static inline void might_alloc(gfp_t gfp) { (void)gfp; }

#endif
