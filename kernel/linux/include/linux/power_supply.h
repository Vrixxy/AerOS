#ifndef _AEROS_LINUX_POWER_SUPPLY_H
#define _AEROS_LINUX_POWER_SUPPLY_H

#include <linux/types.h>
/* Treated as always on mains power (no battery driver on the Linux side). */
static inline int power_supply_is_system_supplied(void) { return 1; }
#endif
