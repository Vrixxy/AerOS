#ifndef _AEROS_LINUX_HWMON_H
#define _AEROS_LINUX_HWMON_H

#include <linux/device.h>
enum hwmon_temp_attributes { hwmon_temp_label = 4 };
/* No hardware-monitoring class: sensors stay reachable through the driver. */
static inline struct device *hwmon_device_register_with_groups(struct device *dev, const char *name, void *drvdata,
							      const struct attribute_group **groups)
{
	(void)dev; (void)name; (void)drvdata; (void)groups;
	return ERR_PTR(-ENODEV);
}
static inline void hwmon_device_unregister(struct device *dev) { (void)dev; }
#endif
