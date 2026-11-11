#ifndef _AEROS_LINUX_KFIFO_H
#define _AEROS_LINUX_KFIFO_H

#include <linux/kernel.h>
/* Fixed-size FIFOs of one element type (the DECLARE_KFIFO form). */
#define DECLARE_KFIFO(fifo, type, size) \
	struct { \
		unsigned int in, out; \
		type *ptr; \
		type buf[((size) < 2 || ((size) & ((size) - 1))) ? -1 : (size)]; \
	} fifo
#define INIT_KFIFO(fifo) ((fifo).in = (fifo).out = 0, (fifo).ptr = (fifo).buf)
#define kfifo_size(fifo) ARRAY_SIZE((fifo)->buf)
#define kfifo_len(fifo) ((fifo)->in - (fifo)->out)
#define kfifo_is_empty(fifo) ((fifo)->in == (fifo)->out)
#define kfifo_is_full(fifo) (kfifo_len(fifo) >= kfifo_size(fifo))
#define kfifo_reset(fifo) ((fifo)->in = (fifo)->out = 0)
#define kfifo_put(fifo, val) ({ \
	__typeof__(fifo) __f = (fifo); \
	int __ok = !kfifo_is_full(__f); \
	if (__ok) \
		__f->buf[__f->in++ & (kfifo_size(__f) - 1)] = (val); \
	__ok; })
#define kfifo_get(fifo, val) ({ \
	__typeof__(fifo) __f = (fifo); \
	int __ok = !kfifo_is_empty(__f); \
	if (__ok) \
		*(val) = __f->buf[__f->out++ & (kfifo_size(__f) - 1)]; \
	__ok; })
#define kfifo_peek(fifo, val) ({ \
	__typeof__(fifo) __f = (fifo); \
	int __ok = !kfifo_is_empty(__f); \
	if (__ok) \
		*(val) = __f->buf[__f->out & (kfifo_size(__f) - 1)]; \
	__ok; })
#define kfifo_skip(fifo) ((void)((fifo)->out++))
#endif
