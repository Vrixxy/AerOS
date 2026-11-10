#ifndef _LINUX_FIRMWARE_H
#define _LINUX_FIRMWARE_H

#include <linux/types.h>
#include <linux/device.h>

/* Firmware comes from FIRMWARE.PAK on the boot partition (one file, so the
 * long Linux names need no long-name support); tools/firmware-pak.py makes
 * it from a linux-firmware checkout. */
struct firmware {
	size_t size;
	const u8 *data;
};

int request_firmware(const struct firmware **firmware, const char *name, struct device *dev);
void release_firmware(const struct firmware *firmware);
#define request_firmware_direct request_firmware
#define firmware_request_nowarn request_firmware

#endif
