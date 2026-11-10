#ifndef _LINUX_KMSG_DUMP_H
#define _LINUX_KMSG_DUMP_H

#include <linux/list.h>
enum kmsg_dump_reason {
	KMSG_DUMP_UNDEF,
	KMSG_DUMP_PANIC,
	KMSG_DUMP_OOPS,
	KMSG_DUMP_EMERG,
	KMSG_DUMP_SHUTDOWN,
};
struct kmsg_dump_detail {
	enum kmsg_dump_reason reason;
	const char *description;
};
struct kmsg_dumper {
	void (*dump)(struct kmsg_dumper *dumper, struct kmsg_dump_detail *detail);
	struct list_head list;
};
static inline int kmsg_dump_register(struct kmsg_dumper *dumper) { (void)dumper; return 0; }
static inline int kmsg_dump_unregister(struct kmsg_dumper *dumper) { (void)dumper; return 0; }

#endif
