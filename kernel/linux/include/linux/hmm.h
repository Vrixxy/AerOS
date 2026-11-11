#ifndef _AEROS_LINUX_HMM_H
#define _AEROS_LINUX_HMM_H

#include <linux/mmu_notifier.h>
struct hmm_range {
	struct mmu_interval_notifier *notifier;
	unsigned long notifier_seq;
	unsigned long start;
	unsigned long end;
	unsigned long *hmm_pfns;
	unsigned long default_flags;
	unsigned long pfn_flags_mask;
	void *dev_private_owner;
};
static inline int hmm_range_fault(struct hmm_range *range) { (void)range; return -EOPNOTSUPP; }
#endif
