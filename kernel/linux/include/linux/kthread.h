#ifndef _LINUX_KTHREAD_H
#define _LINUX_KTHREAD_H

#include <linux/sched.h>
#include <linux/errno.h>

/* No kernel threads: the Linux side is one cooperative thread. */
static inline bool kthread_should_stop(void) { return false; }
static inline bool kthread_should_park(void) { return false; }
static inline void kthread_parkme(void) { }
#define kthread_run(fn, data, namefmt, ...) ((struct task_struct *)ERR_PTR(-ENOSYS))
#define kthread_create(fn, data, namefmt, ...) ((struct task_struct *)ERR_PTR(-ENOSYS))
static inline int kthread_stop(struct task_struct *task) { (void)task; return 0; }
static inline int kthread_park(struct task_struct *task) { (void)task; return 0; }
static inline void kthread_unpark(struct task_struct *task) { (void)task; }

#include <linux/workqueue.h>

/* kthread workers run on the shared cooperative work queue. */
struct kthread_work;
typedef void (*kthread_work_func_t)(struct kthread_work *work);
struct kthread_work {
	struct work_struct base;
	kthread_work_func_t func;
};
struct kthread_worker {
	const char *name;
};
void aeros_kthread_work_run(struct work_struct *work);
static inline void kthread_init_work(struct kthread_work *work, kthread_work_func_t func)
{
	INIT_WORK(&work->base, aeros_kthread_work_run);
	work->func = func;
}
static inline bool kthread_queue_work(struct kthread_worker *worker, struct kthread_work *work)
{
	(void)worker;
	return queue_work(system_wq, &work->base);
}
static inline void kthread_flush_work(struct kthread_work *work) { flush_work(&work->base); }
static inline bool kthread_cancel_work_sync(struct kthread_work *work) { return cancel_work_sync(&work->base); }
static inline void kthread_flush_worker(struct kthread_worker *worker) { (void)worker; flush_workqueue(system_wq); }
struct kthread_worker *kthread_create_worker(unsigned int flags, const char namefmt[], ...);
static inline void kthread_destroy_worker(struct kthread_worker *worker) { kfree(worker); }

#endif
