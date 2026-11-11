#ifndef _LINUX_MMU_NOTIFIER_H
#define _LINUX_MMU_NOTIFIER_H
#include <linux/mm_types.h>
struct mmu_notifier;
struct mmu_interval_notifier;
static inline void mmu_notifier_synchronize(void) { }

#endif
