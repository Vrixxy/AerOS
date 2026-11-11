#ifndef _AEROS_LINUX_IOMMU_H
#define _AEROS_LINUX_IOMMU_H

#include <linux/device.h>
/* AerOS programs its IOMMU itself; drivers see no translation domain. */
#define IOMMU_DOMAIN_BLOCKED 0
#define IOMMU_DOMAIN_IDENTITY 1
#define IOMMU_DOMAIN_UNMANAGED 2
#define IOMMU_DOMAIN_DMA 3
struct iommu_domain {
	unsigned int type;
};
static inline struct iommu_domain *iommu_get_domain_for_dev(struct device *dev) { (void)dev; return NULL; }
static inline bool device_iommu_mapped(struct device *dev) { (void)dev; return false; }
#endif
