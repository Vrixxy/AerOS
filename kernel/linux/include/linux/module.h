#ifndef _LINUX_MODULE_H
#define _LINUX_MODULE_H

#include <linux/kernel.h>
#include <linux/errno.h>
#include <linux/moduleparam.h>

/* Every module_init lands in one section; AerOS runs them in link order. */
typedef int (*initcall_t)(void);

#define __aeros_initcall(fn, id) \
	static initcall_t __initcall_##fn##id \
	__attribute__((used, section("aeros_initcalls"))) = fn

#define module_init(fn) __aeros_initcall(fn, 6)
#define module_exit(fn) \
	static void (*__exitcall_##fn)(void) __attribute__((used)) = fn
#define subsys_initcall(fn) __aeros_initcall(fn, 4)
#define device_initcall(fn) __aeros_initcall(fn, 6)

#define MODULE_LICENSE(x)
#define MODULE_AUTHOR(x)
#define MODULE_DESCRIPTION(x)
#define MODULE_VERSION(x)
#define MODULE_FIRMWARE(x)
#define MODULE_DEVICE_TABLE(type, name)
#define THIS_MODULE ((struct module *)0)
#define MODULE_IMPORT_NS(ns)
#define MODULE_SOFTDEP(text)
#define MODULE_ALIAS(name)
#define MODULE_INFO(tag, info)

struct module;

#endif
