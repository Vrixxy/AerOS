#ifndef _LINUX_CC_PLATFORM_H
#define _LINUX_CC_PLATFORM_H
#include <linux/types.h>
enum cc_attr {
	CC_ATTR_MEM_ENCRYPT,
	CC_ATTR_HOST_MEM_ENCRYPT,
	CC_ATTR_GUEST_MEM_ENCRYPT,
	CC_ATTR_GUEST_STATE_ENCRYPT,
};
static inline bool cc_platform_has(enum cc_attr attr) { (void)attr; return false; }
#endif
