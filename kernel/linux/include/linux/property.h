#ifndef _LINUX_PROPERTY_H
#define _LINUX_PROPERTY_H

#include <linux/device.h>

/* No firmware device tree on AerOS's PCs. */
struct fwnode_handle {
	const void *ops;
	struct fwnode_handle *secondary;
};
static inline void fwnode_handle_put(struct fwnode_handle *fwnode) { (void)fwnode; }
static inline struct fwnode_handle *fwnode_handle_get(struct fwnode_handle *fwnode) { return fwnode; }
#define dev_fwnode(dev) ((struct fwnode_handle *)NULL)
static inline bool device_property_present(const struct device *dev, const char *name) { (void)dev; (void)name; return false; }
static inline int device_property_read_u32(const struct device *dev, const char *name, u32 *value)
{
	(void)dev; (void)name; (void)value;
	return -EINVAL;
}

#endif
