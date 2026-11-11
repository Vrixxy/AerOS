#ifndef _AEROS_LINUX_SYSFS_H
#define _AEROS_LINUX_SYSFS_H

#include <linux/types.h>
#include <linux/printk.h>
struct kobject;
struct device;
struct file;
struct vm_area_struct;
struct bin_attribute;
struct attribute {
	const char *name;
	umode_t mode;
};
struct attribute_group {
	const char *name;
	umode_t (*is_visible)(struct kobject *, struct attribute *, int);
	umode_t (*is_bin_visible)(struct kobject *, struct bin_attribute *, int);
	struct attribute **attrs;
	const struct bin_attribute *const *bin_attrs_new;
	struct bin_attribute **bin_attrs;
};
struct bin_attribute {
	struct attribute attr;
	size_t size;
	void *private;
	ssize_t (*read)(struct file *, struct kobject *, struct bin_attribute *, char *, loff_t, size_t);
	ssize_t (*write)(struct file *, struct kobject *, struct bin_attribute *, char *, loff_t, size_t);
	int (*mmap)(struct file *, struct kobject *, struct bin_attribute *attr, struct vm_area_struct *vma);
};
struct sysfs_ops {
	ssize_t (*show)(struct kobject *, struct attribute *, char *);
	ssize_t (*store)(struct kobject *, struct attribute *, const char *, size_t);
};
#define __ATTR(_name, _mode, _show, _store) { .attr = { .name = __stringify(_name), .mode = (_mode) }, .show = _show, .store = _store }
#define __ATTR_RO(_name) { .attr = { .name = __stringify(_name), .mode = 0444 }, .show = _name##_show }
#define __ATTR_WO(_name) { .attr = { .name = __stringify(_name), .mode = 0200 }, .store = _name##_store }
#define __ATTR_RW(_name) __ATTR(_name, 0644, _name##_show, _name##_store)
#define __ATTR_NULL { .attr = { .name = NULL } }
#define __BIN_ATTR(_name, _mode, _read, _write, _size) { .attr = { .name = __stringify(_name), .mode = (_mode) }, .read = _read, .write = _write, .size = _size }
#define BIN_ATTR(_name, _mode, _read, _write, _size) struct bin_attribute bin_attr_##_name = __BIN_ATTR(_name, _mode, _read, _write, _size)
#define sysfs_attr_init(attr) do { } while (0)
#define sysfs_bin_attr_init(attr) do { } while (0)
#define ATTRIBUTE_GROUPS(_name) \
	static const struct attribute_group _name##_group = { .attrs = _name##_attrs }; \
	static const struct attribute_group *_name##_groups[] = { &_name##_group, NULL }
static inline int sysfs_create_file(struct kobject *kobj, const struct attribute *attr) { (void)kobj; (void)attr; return 0; }
static inline void sysfs_remove_file(struct kobject *kobj, const struct attribute *attr) { (void)kobj; (void)attr; }
static inline int sysfs_create_files(struct kobject *kobj, const struct attribute *const *attr) { (void)kobj; (void)attr; return 0; }
static inline void sysfs_remove_files(struct kobject *kobj, const struct attribute *const *attr) { (void)kobj; (void)attr; }
static inline int sysfs_create_group(struct kobject *kobj, const struct attribute_group *grp) { (void)kobj; (void)grp; return 0; }
static inline void sysfs_remove_group(struct kobject *kobj, const struct attribute_group *grp) { (void)kobj; (void)grp; }
static inline int sysfs_create_groups(struct kobject *kobj, const struct attribute_group **groups) { (void)kobj; (void)groups; return 0; }
static inline void sysfs_remove_groups(struct kobject *kobj, const struct attribute_group **groups) { (void)kobj; (void)groups; }
static inline int sysfs_update_group(struct kobject *kobj, const struct attribute_group *grp) { (void)kobj; (void)grp; return 0; }
static inline int sysfs_merge_group(struct kobject *kobj, const struct attribute_group *grp) { (void)kobj; (void)grp; return 0; }
static inline void sysfs_unmerge_group(struct kobject *kobj, const struct attribute_group *grp) { (void)kobj; (void)grp; }
static inline int sysfs_add_file_to_group(struct kobject *kobj, const struct attribute *attr, const char *group)
{
	(void)kobj; (void)attr; (void)group;
	return 0;
}
static inline void sysfs_remove_file_from_group(struct kobject *kobj, const struct attribute *attr, const char *group)
{
	(void)kobj; (void)attr; (void)group;
}
static inline int sysfs_create_bin_file(struct kobject *kobj, const struct bin_attribute *attr) { (void)kobj; (void)attr; return 0; }
static inline void sysfs_remove_bin_file(struct kobject *kobj, const struct bin_attribute *attr) { (void)kobj; (void)attr; }
static inline int sysfs_create_link(struct kobject *kobj, struct kobject *target, const char *name)
{
	(void)kobj; (void)target; (void)name;
	return 0;
}
static inline void sysfs_remove_link(struct kobject *kobj, const char *name) { (void)kobj; (void)name; }
static inline void sysfs_notify(struct kobject *kobj, const char *dir, const char *attr) { (void)kobj; (void)dir; (void)attr; }
int sysfs_emit(char *buf, const char *fmt, ...) __attribute__((format(printf, 2, 3)));
int sysfs_emit_at(char *buf, int at, const char *fmt, ...) __attribute__((format(printf, 3, 4)));
struct kobj_attribute {
	struct attribute attr;
	ssize_t (*show)(struct kobject *kobj, struct kobj_attribute *attr, char *buf);
	ssize_t (*store)(struct kobject *kobj, struct kobj_attribute *attr, const char *buf, size_t count);
};

#endif
