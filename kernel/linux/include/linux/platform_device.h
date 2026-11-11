#ifndef _AEROS_LINUX_PLATFORM_DEVICE_H
#define _AEROS_LINUX_PLATFORM_DEVICE_H

#include <linux/device.h>
struct platform_device {
	const char *name;
	int id;
	struct device dev;
};
#endif
