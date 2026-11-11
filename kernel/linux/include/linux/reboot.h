#ifndef _AEROS_LINUX_REBOOT_H
#define _AEROS_LINUX_REBOOT_H

#include <linux/notifier.h>
#define SYS_DOWN 0x0001
#define SYS_RESTART SYS_DOWN
#define SYS_HALT 0x0002
#define SYS_POWER_OFF 0x0003
static inline int register_reboot_notifier(struct notifier_block *nb) { (void)nb; return 0; }
static inline int unregister_reboot_notifier(struct notifier_block *nb) { (void)nb; return 0; }
static inline void orderly_poweroff(bool force) { (void)force; }
static inline void emergency_restart(void) { }
#endif
