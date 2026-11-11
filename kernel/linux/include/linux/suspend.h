#ifndef _AEROS_LINUX_SUSPEND_H
#define _AEROS_LINUX_SUSPEND_H

#include <linux/notifier.h>
typedef int suspend_state_t;
#define PM_SUSPEND_ON 0
#define PM_SUSPEND_TO_IDLE 1
#define PM_SUSPEND_STANDBY 2
#define PM_SUSPEND_MEM 3
#define pm_suspend_target_state PM_SUSPEND_ON
#define pm_suspend_via_firmware() false
#define pm_resume_via_firmware() false
#define pm_suspend_default_s2idle() false
static inline void lock_system_sleep(void) { }
static inline void unlock_system_sleep(void) { }
static inline int register_pm_notifier(struct notifier_block *nb) { (void)nb; return 0; }
static inline int unregister_pm_notifier(struct notifier_block *nb) { (void)nb; return 0; }
#define PM_HIBERNATION_PREPARE 0x0001
#define PM_POST_HIBERNATION 0x0002
#define PM_SUSPEND_PREPARE 0x0003
#define PM_POST_SUSPEND 0x0004
#endif
