#ifndef _AEROS_LINUX_APPLE_GMUX_H
#define _AEROS_LINUX_APPLE_GMUX_H

#include <linux/types.h>
struct pnp_dev;
struct apple_gmux_data;
static inline bool apple_gmux_detect(struct pnp_dev *pnp_dev, struct apple_gmux_data *data)
{
	(void)pnp_dev; (void)data;
	return false;
}
#endif
