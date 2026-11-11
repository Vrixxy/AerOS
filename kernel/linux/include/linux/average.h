#ifndef _LINUX_AVERAGE_H
#define _LINUX_AVERAGE_H

#include <linux/kernel.h>

/* Exponentially weighted moving average with fixed-point precision. */
#define DECLARE_EWMA(name, _precision, _weight_rcp) \
	struct ewma_##name { unsigned long internal; }; \
	static inline void ewma_##name##_init(struct ewma_##name *e) { e->internal = 0; } \
	static inline unsigned long ewma_##name##_read(struct ewma_##name *e) { return e->internal >> (_precision); } \
	static inline void ewma_##name##_add(struct ewma_##name *e, unsigned long val) \
	{ \
		unsigned long internal = READ_ONCE(e->internal); \
		unsigned long weight_rcp = ilog2(_weight_rcp); \
		WRITE_ONCE(e->internal, internal ? \
			(((internal << weight_rcp) - internal) + (val << (_precision))) >> weight_rcp : \
			(val << (_precision))); \
	}

#endif
