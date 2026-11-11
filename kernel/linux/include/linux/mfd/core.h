#ifndef _AEROS_LINUX_MFD_CORE_H
#define _AEROS_LINUX_MFD_CORE_H

#include <linux/device.h>
struct mfd_cell {
	const char *name;
	int id;
	void *platform_data;
	size_t pdata_size;
	int num_resources;
	const struct resource *resources;
};
static inline int mfd_add_hotplug_devices(struct device *parent, const struct mfd_cell *cells, int n_devs)
{
	(void)parent; (void)cells; (void)n_devs;
	return -ENODEV;
}
static inline void mfd_remove_devices(struct device *parent) { (void)parent; }
#endif
