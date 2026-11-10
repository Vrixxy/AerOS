#ifndef _LINUX_EVENTFD_H
#define _LINUX_EVENTFD_H

#include <linux/kernel.h>
struct eventfd_ctx;
static inline struct eventfd_ctx *eventfd_ctx_fdget(int fd) { (void)fd; return (struct eventfd_ctx *)ERR_PTR(-EBADF); }
static inline void eventfd_ctx_put(struct eventfd_ctx *ctx) { (void)ctx; }
static inline void eventfd_signal(struct eventfd_ctx *ctx) { (void)ctx; }

#endif
