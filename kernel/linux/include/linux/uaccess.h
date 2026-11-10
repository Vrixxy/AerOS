#ifndef _LINUX_UACCESS_H
#define _LINUX_UACCESS_H

#include <linux/kernel.h>
/* Callers so far are kernel-side, so "user" pointers are plain memory.
 * TODO: route through AerOS's user-copy checks once ioctls come from
 * user programs. */
static inline unsigned long copy_to_user(void __user *to, const void *from, unsigned long n)
{
	memcpy((void *)to, from, n);
	return 0;
}
static inline unsigned long copy_from_user(void *to, const void __user *from, unsigned long n)
{
	memcpy(to, (const void *)from, n);
	return 0;
}
static inline unsigned long clear_user(void __user *to, unsigned long n)
{
	memset((void *)to, 0, n);
	return 0;
}
#define get_user(x, ptr) ({ (x) = *(ptr); 0; })
#define put_user(x, ptr) ({ *(ptr) = (x); 0; })
#define access_ok(addr, size) 1
#define u64_to_user_ptr(x) ((void __user *)(uintptr_t)(x))
#define user_access_begin(ptr, len) 1
#define user_access_end() do { } while (0)
#define unsafe_put_user(x, ptr, label) do { *(ptr) = (x); } while (0)
#define unsafe_get_user(x, ptr, label) do { (x) = *(ptr); } while (0)

#endif
