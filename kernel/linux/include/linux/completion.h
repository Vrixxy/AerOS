#ifndef _LINUX_COMPLETION_H
#define _LINUX_COMPLETION_H

#include <linux/wait.h>

struct completion {
	unsigned int done;
};

#define DECLARE_COMPLETION(name) struct completion name = { 0 }
static inline void init_completion(struct completion *x) { x->done = 0; }
static inline void reinit_completion(struct completion *x) { x->done = 0; }
static inline void complete(struct completion *x) { x->done++; }
static inline void complete_all(struct completion *x) { x->done = ~0u >> 1; }
static inline bool completion_done(struct completion *x) { return x->done != 0; }

static inline void wait_for_completion(struct completion *x)
{
	wait_event(0, x->done);
	x->done--;
}

static inline unsigned long wait_for_completion_timeout(struct completion *x, unsigned long timeout)
{
	long left = wait_event_timeout(0, x->done, timeout);
	if (left)
		x->done--;
	return (unsigned long)left;
}

#endif
