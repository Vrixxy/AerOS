#ifndef _LINUX_STACKTRACE_H
#define _LINUX_STACKTRACE_H

#include <linux/types.h>
static inline unsigned int stack_trace_save(unsigned long *store, unsigned int size, unsigned int skip)
{
	(void)store; (void)size; (void)skip;
	return 0;
}

#endif
