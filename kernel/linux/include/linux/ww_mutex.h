#ifndef _LINUX_WW_MUTEX_H
#define _LINUX_WW_MUTEX_H

#include <linux/mutex.h>
#include <linux/errno.h>

/* Single-threaded: a wound/wait lock only has to report self-deadlock. */
struct ww_class {
	const char *name;
};
struct ww_acquire_ctx {
	struct ww_class *ww_class;
	unsigned int acquired;
};
struct ww_mutex {
	struct mutex base;
	struct ww_acquire_ctx *ctx;
};

#define DEFINE_WW_CLASS(classname) struct ww_class classname = { #classname }
#define DEFINE_WD_CLASS(classname) DEFINE_WW_CLASS(classname)

static inline void ww_mutex_init(struct ww_mutex *lock, struct ww_class *ww_class)
{
	(void)ww_class;
	mutex_init(&lock->base);
	lock->ctx = NULL;
}
static inline void ww_mutex_destroy(struct ww_mutex *lock) { (void)lock; }
static inline bool ww_mutex_is_locked(struct ww_mutex *lock) { return mutex_is_locked(&lock->base); }
static inline void ww_acquire_init(struct ww_acquire_ctx *ctx, struct ww_class *ww_class)
{
	ctx->ww_class = ww_class;
	ctx->acquired = 0;
}
static inline void ww_acquire_done(struct ww_acquire_ctx *ctx) { (void)ctx; }
static inline void ww_acquire_fini(struct ww_acquire_ctx *ctx) { (void)ctx; }
static inline int ww_mutex_lock(struct ww_mutex *lock, struct ww_acquire_ctx *ctx)
{
	if (lock->base.held) {
		if (ctx && lock->ctx == ctx)
			return -EALREADY;
		aeros_lock_twice(lock);
	}
	lock->base.held = 1;
	lock->ctx = ctx;
	if (ctx)
		ctx->acquired++;
	return 0;
}
#define ww_mutex_lock_interruptible ww_mutex_lock
#define ww_mutex_lock_slow(lock, ctx) ((void)ww_mutex_lock(lock, ctx))
#define ww_mutex_lock_slow_interruptible ww_mutex_lock
static inline int ww_mutex_trylock(struct ww_mutex *lock, struct ww_acquire_ctx *ctx)
{
	if (lock->base.held)
		return 0;
	return ww_mutex_lock(lock, ctx) == 0;
}
static inline void ww_mutex_unlock(struct ww_mutex *lock)
{
	if (lock->ctx)
		lock->ctx->acquired--;
	lock->ctx = NULL;
	lock->base.held = 0;
}

#endif
