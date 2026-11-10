#ifndef _LINUX_MODULEPARAM_H
#define _LINUX_MODULEPARAM_H

#include <linux/types.h>

/* Parameters keep their compiled-in defaults; there is no command line. */
#define module_param_named(name, value, type, perm) \
	static __attribute__((unused)) void *__aeros_param_##name = &(value)
#define module_param(name, type, perm) module_param_named(name, name, type, perm)
#define module_param_unsafe(name, type, perm) module_param(name, type, perm)
#define module_param_named_unsafe(name, value, type, perm) module_param_named(name, value, type, perm)
#define module_param_string(name, string, len, perm) module_param_named(name, string, charp, perm)
#define module_param_array(name, type, nump, perm) \
	static __attribute__((unused)) void *__aeros_param_##name = (name)
#define module_param_cb(name, ops, arg, perm) \
	static __attribute__((unused)) const void *__aeros_param_##name = (arg)
#define MODULE_PARM_DESC(name, text)
#define __MODULE_PARM_TYPE(name, type)

struct kernel_param;
struct kernel_param_ops {
	unsigned int flags;
	int (*set)(const char *value, const struct kernel_param *param);
	int (*get)(char *buffer, const struct kernel_param *param);
	void (*free)(void *arg);
};
struct kernel_param {
	const char *name;
	const struct kernel_param_ops *ops;
	void *arg;
};

#endif
