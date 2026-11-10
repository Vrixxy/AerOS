#ifndef _LINUX_DEVICE_H
#define _LINUX_DEVICE_H

#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/fs.h>
struct kobject {
	const char *name;
	struct kobject *parent;
};

struct device_driver {
	const char *name;
};

struct device {
	const char *init_name;
	struct device *parent;
	struct device_driver *driver;
	void *driver_data;
	char name[32];
	dev_t devt;
	struct kobject kobj;
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
static inline int sysfs_create_link(struct kobject *kobj, struct kobject *target, const char *name)
{
	(void)kobj; (void)target; (void)name;
	return 0;
}
static inline void sysfs_remove_link(struct kobject *kobj, const char *name) { (void)kobj; (void)name; }

#endif
