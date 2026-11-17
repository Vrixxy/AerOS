#ifndef _LINUX_COMPONENT_H
#define _LINUX_COMPONENT_H

#include <linux/types.h>

/* No other driver (HDA audio) binds to the GPU's components in AerOS. */
struct device;
struct component_ops {
	int (*bind)(struct device *comp, struct device *master, void *data);
	void (*unbind)(struct device *comp, struct device *master, void *data);
};
static inline int component_add(struct device *dev, const struct component_ops *ops) { (void)dev; (void)ops; return 0; }
static inline void component_del(struct device *dev, const struct component_ops *ops) { (void)dev; (void)ops; }

#endif
