#include <linux/firmware.h>
#include <linux/slab.h>
#include <linux/errno.h>
#include <linux/printk.h>
#include <aeros/ops.h>

int request_firmware(const struct firmware **firmware, const char *name, struct device *dev)
{
	unsigned long size = 0;
	const u8 *data = aeros->firmware(name, &size);
	struct firmware *found;

	*firmware = 0;
	if (!data) {
		printk("%s: firmware %s not in FIRMWARE.PAK\n", dev ? dev->name : "linux", name);
		return -ENOENT;
	}
	found = kmalloc(sizeof(*found), GFP_KERNEL);
	if (!found)
		return -ENOMEM;
	found->size = size;
	found->data = data;
	*firmware = found;
	return 0;
}

void release_firmware(const struct firmware *firmware)
{
	kfree(firmware);
}
