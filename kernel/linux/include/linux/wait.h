#ifndef _LINUX_WAIT_H
#define _LINUX_WAIT_H

#include <linux/types.h>
#include <linux/jiffies.h>
#include <linux/spinlock.h>

/* Waiting keeps the Linux side running: interrupt handlers and timers are
 * polled until the condition holds, so whatever is awaited can happen. */
typedef struct {
	spinlock_t lock;
} wait_queue_head_t;

#define DECLARE_WAIT_QUEUE_HEAD(name) wait_queue_head_t name = { 0 }
static inline void init_waitqueue_head(wait_queue_head_t *queue) { spin_lock_init(&queue->lock); }
static inline void wake_up(wait_queue_head_t *queue) { (void)queue; }
#define wake_up_all wake_up
#define wake_up_interruptible wake_up
#define wake_up_interruptible_all wake_up

void aeros_wait_step(void);

#define wait_event(queue, condition) do { (void)(queue); while (!(condition)) aeros_wait_step(); } while (0)
#define wait_event_interruptible(queue, condition) ({ wait_event(queue, condition); 0; })
#define wait_event_timeout(queue, condition, timeout) ({ \
	unsigned long __end = jiffies + (timeout); \
	(void)(queue); \
	long __left = 0; \
	for (;;) { \
		if (condition) { __left = (long)(__end - jiffies); if (__left < 1) __left = 1; break; } \
		if (time_after_eq(jiffies, __end)) { __left = (condition) ? 1 : 0; break; } \
		aeros_wait_step(); \
	} \
	__left; })
#define wait_event_interruptible_timeout wait_event_timeout
#define wait_event_killable wait_event_interruptible
#define wait_event_killable_timeout wait_event_timeout

#define wait_event_lock_irq(queue, condition, lock) do { \
	spin_unlock(&(lock)); \
	wait_event(queue, condition); \
	spin_lock(&(lock)); \
} while (0)
#define wait_event_interruptible_lock_irq(queue, condition, lock) ({ wait_event_lock_irq(queue, condition, lock); 0; })

#define wake_up_all_locked wake_up
#define wake_up_locked wake_up
#define wait_event_interruptible_locked(queue, condition) ({ 	spin_unlock(&(queue).lock); 	wait_event(queue, condition); 	spin_lock(&(queue).lock); 	0; })
#define wait_event_interruptible_locked_irq wait_event_interruptible_locked

#endif
