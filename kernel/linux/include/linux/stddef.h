#ifndef _AEROS_LINUX_STDDEF_H
#define _AEROS_LINUX_STDDEF_H

#include <linux/types.h>
#ifndef offsetofend
#define offsetofend(TYPE, MEMBER) (offsetof(TYPE, MEMBER) + sizeof(((TYPE *)0)->MEMBER))
#endif
#endif
