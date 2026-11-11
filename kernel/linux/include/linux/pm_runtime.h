#ifndef _AEROS_LINUX_PM_RUNTIME_H
#define _AEROS_LINUX_PM_RUNTIME_H

#include <linux/device.h>
/* No runtime power management: devices stay powered while bound. */
#define RPM_GET_PUT 0x04
static inline int pm_runtime_get_sync(struct device *dev) { (void)dev; return 1; }
static inline int pm_runtime_resume_and_get(struct device *dev) { (void)dev; return 0; }
static inline int pm_runtime_get_if_active(struct device *dev) { (void)dev; return 1; }
static inline int pm_runtime_get_if_in_use(struct device *dev) { (void)dev; return 1; }
static inline void pm_runtime_get_noresume(struct device *dev) { (void)dev; }
static inline int pm_runtime_put_autosuspend(struct device *dev) { (void)dev; return 0; }
static inline int pm_runtime_put_sync(struct device *dev) { (void)dev; return 0; }
static inline int pm_runtime_put(struct device *dev) { (void)dev; return 0; }
static inline void pm_runtime_put_noidle(struct device *dev) { (void)dev; }
static inline void pm_runtime_mark_last_busy(struct device *dev) { (void)dev; }
static inline void pm_runtime_use_autosuspend(struct device *dev) { (void)dev; }
static inline void pm_runtime_dont_use_autosuspend(struct device *dev) { (void)dev; }
static inline void pm_runtime_set_autosuspend_delay(struct device *dev, int delay) { (void)dev; (void)delay; }
static inline void pm_runtime_allow(struct device *dev) { (void)dev; }
static inline void pm_runtime_forbid(struct device *dev) { (void)dev; }
static inline void pm_runtime_enable(struct device *dev) { (void)dev; }
static inline void pm_runtime_disable(struct device *dev) { (void)dev; }
static inline int pm_runtime_set_active(struct device *dev) { (void)dev; return 0; }
static inline int pm_runtime_resume(struct device *dev) { (void)dev; return 0; }
static inline int pm_runtime_suspend(struct device *dev) { (void)dev; return -ENOSYS; }
static inline int pm_runtime_autosuspend(struct device *dev) { (void)dev; return -ENOSYS; }
static inline bool pm_runtime_suspended(struct device *dev) { (void)dev; return false; }
static inline bool pm_runtime_status_suspended(struct device *dev) { (void)dev; return false; }
static inline bool pm_runtime_active(struct device *dev) { (void)dev; return true; }
static inline u64 pm_runtime_autosuspend_expiration(struct device *dev) { (void)dev; return 0; }
static inline void pm_runtime_set_suspended(struct device *dev) { (void)dev; }
static inline void pm_runtime_no_callbacks(struct device *dev) { (void)dev; }
static inline void pm_runtime_irq_safe(struct device *dev) { (void)dev; }
#endif
