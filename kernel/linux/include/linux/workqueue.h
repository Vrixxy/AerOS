#ifndef _LINUX_WORKQUEUE_H
#define _LINUX_WORKQUEUE_H

#include <linux/types.h>
#include <linux/timer.h>

struct work_struct;
typedef void (*work_func_t)(struct work_struct *work);

struct work_struct {
	struct work_struct *next;
	work_func_t func;
	bool pending;
};

struct delayed_work {
	struct work_struct work;
	struct timer_list timer;
};

struct workqueue_struct {
	const char *name;
};

extern struct workqueue_struct *system_wq;

void aeros_delayed_fire(struct timer_list *timer);
#define DECLARE_DELAYED_WORK_AEROS(name, f) 	struct delayed_work name = { .work = { .func = (f) }, .timer = { .function = aeros_delayed_fire } }
#define DECLARE_DELAYED_WORK(name, f) DECLARE_DELAYED_WORK_AEROS(name, f)
#define DECLARE_WORK(name, f) struct work_struct name = { .func = (f) }

#define INIT_WORK(w, f) do { (w)->next = 0; (w)->func = (f); (w)->pending = false; } while (0)
void aeros_init_delayed_work(struct delayed_work *work, work_func_t func);
#define INIT_DELAYED_WORK(w, f) aeros_init_delayed_work(w, f)
#define to_delayed_work(w) container_of(w, struct delayed_work, work)

bool queue_work(struct workqueue_struct *queue, struct work_struct *work);
bool queue_delayed_work(struct workqueue_struct *queue, struct delayed_work *work,
			unsigned long delay);
bool mod_delayed_work(struct workqueue_struct *queue, struct delayed_work *work,
		      unsigned long delay);
static inline bool schedule_work(struct work_struct *work) { return queue_work(system_wq, work); }
static inline bool schedule_delayed_work(struct delayed_work *work, unsigned long delay)
{
	return queue_delayed_work(system_wq, work, delay);
}
bool cancel_work_sync(struct work_struct *work);
#define queue_work_node(node, queue, work) queue_work((queue), (work))
#define queue_work_on(cpu, queue, work) queue_work((queue), (work))
#define drain_workqueue flush_workqueue
bool cancel_delayed_work(struct delayed_work *work);
#define cancel_delayed_work_sync cancel_delayed_work
bool flush_work(struct work_struct *work);
static inline bool flush_delayed_work(struct delayed_work *work)
{
	if (cancel_delayed_work(work)) {
		work->work.func(&work->work);
		return true;
	}
	return flush_work(&work->work);
}
#define WQ_UNBOUND 0x2u
#define WQ_FREEZABLE 0x4u
#define WQ_MEM_RECLAIM 0x8u
#define WQ_HIGHPRI 0x10u
#define WQ_CPU_INTENSIVE 0x20u
#define WQ_SYSFS 0x40u
#define WQ_POWER_EFFICIENT 0x80u
#define WQ_PERCPU 0x100u
#define WQ_MAX_ACTIVE 512
struct workqueue_struct *alloc_workqueue(const char *name, unsigned int flags, int max_active, ...);
#define alloc_ordered_workqueue(name, flags, ...) alloc_workqueue(name, flags, 1)
static inline void destroy_workqueue(struct workqueue_struct *queue) { (void)queue; }
void flush_workqueue(struct workqueue_struct *queue);

#endif
