#ifndef _AEROS_LINUX_DEVCOREDUMP_H
#define _AEROS_LINUX_DEVCOREDUMP_H

#include <linux/device.h>
static inline void dev_coredumpm(struct device *dev, struct module *owner, void *data, size_t datalen, gfp_t gfp,
				 ssize_t (*read)(char *buffer, loff_t offset, size_t count, void *data, size_t datalen),
				 void (*free)(void *data))
{
	(void)dev; (void)owner; (void)gfp; (void)read; (void)datalen;
	free(data);
}
#endif
