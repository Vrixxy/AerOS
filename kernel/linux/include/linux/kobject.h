#ifndef _AEROS_LINUX_KOBJECT_H
#define _AEROS_LINUX_KOBJECT_H

#include <linux/types.h>
#include <linux/kref.h>
#include <linux/sysfs.h>
#include <linux/list.h>
#include <linux/spinlock.h>
struct kset;
/* Reference-counted named objects; there is no sysfs to show them in. */
struct kobject;
struct kobj_type {
	void (*release)(struct kobject *kobj);
	const struct sysfs_ops *sysfs_ops;
	const struct attribute_group **default_groups;
};
struct kobject {
	const char *name;
	struct kobject *parent;
	const struct kobj_type *ktype;
	struct kref kref;
	bool initialized;
	void *sd;
	struct kset *kset;
	struct list_head entry;
};
static inline void kobject_init(struct kobject *kobj, const struct kobj_type *ktype)
{
	kobj->ktype = ktype;
	kref_init(&kobj->kref);
	kobj->initialized = true;
}
static inline int kobject_add(struct kobject *kobj, struct kobject *parent, const char *fmt, ...)
{
	kobj->parent = parent;
	kobj->name = fmt;
	return 0;
}
static inline int kobject_init_and_add(struct kobject *kobj, const struct kobj_type *ktype, struct kobject *parent,
				       const char *fmt, ...)
{
	kobject_init(kobj, ktype);
	kobj->parent = parent;
	kobj->name = fmt;
	return 0;
}
static inline int kobject_set_name(struct kobject *kobj, const char *fmt, ...)
{
	kobj->name = fmt;
	return 0;
}
static inline const char *kobject_name(const struct kobject *kobj) { return kobj->name; }
static inline struct kobject *kobject_get(struct kobject *kobj)
{
	if (kobj)
		kref_get(&kobj->kref);
	return kobj;
}
static inline void __kobject_release(struct kref *kref)
{
	struct kobject *kobj = container_of(kref, struct kobject, kref);

	if (kobj->ktype && kobj->ktype->release)
		kobj->ktype->release(kobj);
}
static inline void kobject_put(struct kobject *kobj)
{
	if (kobj && kobj->initialized)
		kref_put(&kobj->kref, __kobject_release);
}
static inline void kobject_del(struct kobject *kobj) { (void)kobj; }
static inline int kobject_uevent(struct kobject *kobj, int action) { (void)kobj; (void)action; return 0; }
#define KOBJ_CHANGE 2
struct kset {
	struct list_head list;
	spinlock_t list_lock;
	struct kobject kobj;
};
#define to_kset(k) container_of(k, struct kset, kobj)
static inline int kset_register(struct kset *kset) { (void)kset; return 0; }
static inline void kset_unregister(struct kset *kset) { if (kset) kobject_put(&kset->kobj); }
extern const struct sysfs_ops kobj_sysfs_ops;

#endif
