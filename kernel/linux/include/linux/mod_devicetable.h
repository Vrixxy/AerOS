#ifndef _LINUX_MOD_DEVICETABLE_H
#define _LINUX_MOD_DEVICETABLE_H

#include <linux/types.h>

typedef unsigned long kernel_ulong_t;

struct pci_device_id {
	u32 vendor, device;
	u32 subvendor, subdevice;
	u32 class, class_mask;
	kernel_ulong_t driver_data;
	u32 override_only;
};

#endif
