#ifndef _LINUX_DEVICE_H
#define _LINUX_DEVICE_H

#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/fs.h>
#include <linux/kobject.h>

struct dev_pm_ops;
struct device_driver {
	const char *name;
	const struct dev_pm_ops *pm;
};

struct device {
	const char *init_name;
	struct device *parent;
	struct device_driver *driver;
	void *driver_data;
	char name[32];
	dev_t devt;
	struct kobject kobj;
	struct device_node *of_node;
	struct fwnode_handle *fwnode;
	u64 *dma_mask;
	u64 coherent_dma_mask;
	bool is_pci;
	bool registered;
	void (*release)(struct device *dev);
};
#define dev_is_pci(dev) ((dev) && (dev)->is_pci)
static inline void device_initialize(struct device *dev) { (void)dev; }
static inline int device_add(struct device *dev) { dev->registered = true; return 0; }
static inline void device_del(struct device *dev) { dev->registered = false; }
static inline bool device_is_registered(struct device *dev) { return dev->registered; }
static inline struct device *get_device(struct device *dev) { return dev; }
static inline void put_device(struct device *dev) { (void)dev; }
static inline int dev_set_name(struct device *dev, const char *name, ...) { (void)dev; (void)name; return 0; }

static inline const char *dev_name(const struct device *dev) { return dev->name; }
static inline void *dev_get_drvdata(const struct device *dev) { return dev->driver_data; }
static inline void dev_set_drvdata(struct device *dev, void *data) { dev->driver_data = data; }

int dev_printk(const char *level, const struct device *dev, const char *format, ...)
	__attribute__((format(printf, 3, 4)));

#define dev_err(dev, fmt, ...) dev_printk(KERN_ERR, dev, fmt, ##__VA_ARGS__)
#define dev_warn(dev, fmt, ...) dev_printk(KERN_WARNING, dev, fmt, ##__VA_ARGS__)
#define dev_notice(dev, fmt, ...) dev_printk(KERN_NOTICE, dev, fmt, ##__VA_ARGS__)
#define dev_info(dev, fmt, ...) dev_printk(KERN_INFO, dev, fmt, ##__VA_ARGS__)
#define dev_dbg(dev, fmt, ...) do { } while (0)

static inline const char *dev_driver_string(const struct device *dev)
{
	return dev && dev->driver && dev->driver->name ? dev->driver->name : "aeros";
}

#define dev_to_node(dev) (-1)
#define __aeros_once(call) ({ static bool __done; if (!__done) { __done = true; call; } })
#define dev_info_once(dev, fmt, ...) __aeros_once(dev_info(dev, fmt, ##__VA_ARGS__))
#define dev_warn_once(dev, fmt, ...) __aeros_once(dev_warn(dev, fmt, ##__VA_ARGS__))
#define dev_err_once(dev, fmt, ...) __aeros_once(dev_err(dev, fmt, ##__VA_ARGS__))
#define dev_notice_once(dev, fmt, ...) __aeros_once(dev_notice(dev, fmt, ##__VA_ARGS__))
#define dev_dbg_once(dev, fmt, ...) do { } while (0)
#define dev_err_ratelimited dev_err
#define dev_warn_ratelimited dev_warn
#define dev_info_ratelimited dev_info
#define dev_dbg_ratelimited dev_dbg

static inline u32 old_encode_dev(dev_t dev) { return (MAJOR(dev) << 8) | MINOR(dev); }
static inline u32 new_encode_dev(dev_t dev) { return dev; }

#define devm_add_action(dev, action, data) ({ (void)(dev); (void)(action); (void)(data); 0; })
#define devm_add_action_or_reset devm_add_action
struct device_attribute {
	struct attribute attr;
	ssize_t (*show)(struct device *dev, struct device_attribute *attr, char *buf);
	ssize_t (*store)(struct device *dev, struct device_attribute *attr, const char *buf, size_t count);
};
#define DEVICE_ATTR(_name, _mode, _show, _store) struct device_attribute dev_attr_##_name = __ATTR(_name, _mode, _show, _store)
#define DEVICE_ATTR_RO(_name) struct device_attribute dev_attr_##_name = __ATTR_RO(_name)
#define DEVICE_ATTR_RW(_name) struct device_attribute dev_attr_##_name = __ATTR_RW(_name)
#define DEVICE_ATTR_WO(_name) struct device_attribute dev_attr_##_name = __ATTR_WO(_name)
static inline int device_create_file(struct device *dev, const struct device_attribute *attr) { (void)dev; (void)attr; return 0; }
static inline void device_remove_file(struct device *dev, const struct device_attribute *attr) { (void)dev; (void)attr; }
static inline int device_create_bin_file(struct device *dev, const struct bin_attribute *attr) { (void)dev; (void)attr; return 0; }
static inline void device_remove_bin_file(struct device *dev, const struct bin_attribute *attr) { (void)dev; (void)attr; }
static inline int devm_device_add_group(struct device *dev, const struct attribute_group *grp) { (void)dev; (void)grp; return 0; }

#define dev_err_probe(dev, error, fmt, ...) ({ int __e = (error); dev_err(dev, fmt, ##__VA_ARGS__); __e; })
#define dev_warn_probe(dev, error, fmt, ...) ({ int __e = (error); dev_warn(dev, fmt, ##__VA_ARGS__); __e; })

struct dev_pm_ops {
	int (*prepare)(struct device *dev);
	void (*complete)(struct device *dev);
	int (*suspend)(struct device *dev);
	int (*resume)(struct device *dev);
	int (*freeze)(struct device *dev);
	int (*thaw)(struct device *dev);
	int (*poweroff)(struct device *dev);
	int (*restore)(struct device *dev);
	int (*suspend_late)(struct device *dev);
	int (*resume_early)(struct device *dev);
	int (*freeze_late)(struct device *dev);
	int (*thaw_early)(struct device *dev);
	int (*poweroff_late)(struct device *dev);
	int (*restore_early)(struct device *dev);
	int (*suspend_noirq)(struct device *dev);
	int (*resume_noirq)(struct device *dev);
	int (*freeze_noirq)(struct device *dev);
	int (*thaw_noirq)(struct device *dev);
	int (*poweroff_noirq)(struct device *dev);
	int (*restore_noirq)(struct device *dev);
	int (*runtime_suspend)(struct device *dev);
	int (*runtime_resume)(struct device *dev);
	int (*runtime_idle)(struct device *dev);
};
struct dev_pm_domain {
	struct dev_pm_ops ops;
};
static inline void dev_pm_domain_set(struct device *dev, struct dev_pm_domain *pd) { (void)dev; (void)pd; }
#define kobj_to_dev(k) container_of(k, struct device, kobj)
#define dev_emerg(dev, fmt, ...) dev_printk(KERN_EMERG, dev, fmt, ##__VA_ARGS__)
#define dev_crit(dev, fmt, ...) dev_printk(KERN_CRIT, dev, fmt, ##__VA_ARGS__)
#define dev_alert(dev, fmt, ...) dev_printk(KERN_ALERT, dev, fmt, ##__VA_ARGS__)
#define dev_WARN(dev, format, ...) WARN(1, "%s: " format, dev_name(dev), ##__VA_ARGS__)
#define dev_WARN_ONCE(dev, condition, format, ...) WARN_ONCE(condition, "%s: " format, dev_name(dev), ##__VA_ARGS__)
static inline bool dev_is_removable(struct device *dev) { (void)dev; return false; }
#define num_possible_nodes() 1
#define num_online_nodes() 1

#include <linux/reboot.h>

#define DPM_FLAG_NO_DIRECT_COMPLETE (1 << 0)
#define DPM_FLAG_SMART_PREPARE (1 << 1)
#define DPM_FLAG_SMART_SUSPEND (1 << 2)
#define DPM_FLAG_MAY_SKIP_RESUME (1 << 3)
static inline void dev_pm_set_driver_flags(struct device *dev, u32 flags) { (void)dev; (void)flags; }

#endif
