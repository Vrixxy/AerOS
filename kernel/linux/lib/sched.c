/* Timers, work queues and interrupt handlers. Everything runs from
 * aeros_linux_poll, which AerOS calls from its main loop: one thread, so
 * a work item or timer never runs while another is running. */
#include <linux/interrupt.h>
#include <linux/workqueue.h>
#include <linux/slab.h>
#include <linux/errno.h>
#include <linux/wait.h>
#include <linux/spinlock.h>
#include <linux/mutex.h>
#include <linux/list.h>
#include <linux/atomic.h>
#include <linux/completion.h>
#include <linux/rcupdate.h>
#include <linux/irq_work.h>
#include <aeros/ops.h>

#define MAX_IRQS 16

static struct timer_list *timers;
static struct work_struct *work_head, *work_tail;
static struct workqueue_struct system_queue = { "events" };
struct workqueue_struct *system_wq = &system_queue;

static struct {
	unsigned int irq;
	irq_handler_t handler;
	void *data;
} irqs[MAX_IRQS];

unsigned long aeros_jiffies(void)
{
	return (unsigned long)(aeros->nanoseconds() / 1000000);
}

void timer_setup(struct timer_list *timer, void (*function)(struct timer_list *), unsigned int flags)
{
	(void)flags;
	timer->next = 0;
	timer->function = function;
	timer->pending = false;
}

int del_timer(struct timer_list *timer)
{
	for (struct timer_list **link = &timers; *link; link = &(*link)->next) {
		if (*link == timer) {
			*link = timer->next;
			timer->pending = false;
			return 1;
		}
	}
	return 0;
}

int mod_timer(struct timer_list *timer, unsigned long expires)
{
	int was = del_timer(timer);

	timer->expires = expires;
	timer->pending = true;
	timer->next = timers;
	timers = timer;
	return was;
}

void add_timer(struct timer_list *timer)
{
	mod_timer(timer, timer->expires);
}

bool queue_work(struct workqueue_struct *queue, struct work_struct *work)
{
	(void)queue;
	if (work->pending)
		return false;
	work->pending = true;
	work->next = 0;
	if (work_tail)
		work_tail->next = work;
	else
		work_head = work;
	work_tail = work;
	return true;
}

void aeros_delayed_fire(struct timer_list *timer)
{
	struct delayed_work *work = container_of(timer, struct delayed_work, timer);

	queue_work(system_wq, &work->work);
}

void aeros_init_delayed_work(struct delayed_work *work, work_func_t func)
{
	INIT_WORK(&work->work, func);
	timer_setup(&work->timer, aeros_delayed_fire, 0);
}

bool queue_delayed_work(struct workqueue_struct *queue, struct delayed_work *work,
			unsigned long delay)
{
	if (work->work.pending || work->timer.pending)
		return false;
	if (!delay)
		return queue_work(queue, &work->work);
	mod_timer(&work->timer, jiffies + delay);
	return true;
}

bool mod_delayed_work(struct workqueue_struct *queue, struct delayed_work *work,
		      unsigned long delay)
{
	bool was = cancel_delayed_work(work);

	queue_delayed_work(queue, work, delay);
	return was;
}

static bool unqueue(struct work_struct *work)
{
	struct work_struct *previous = 0;

	for (struct work_struct *at = work_head; at; previous = at, at = at->next) {
		if (at != work)
			continue;
		if (previous)
			previous->next = at->next;
		else
			work_head = at->next;
		if (work_tail == at)
			work_tail = previous;
		work->pending = false;
		return true;
	}
	return false;
}

bool cancel_work_sync(struct work_struct *work)
{
	return unqueue(work);
}

bool cancel_delayed_work(struct delayed_work *work)
{
	bool timer = del_timer(&work->timer) != 0;

	return unqueue(&work->work) || timer;
}

bool flush_work(struct work_struct *work)
{
	if (!unqueue(work))
		return false;
	work->func(work);
	return true;
}

static int work_depth;

static void run_work(void)
{
	/* Only what was queued before this pass, so work that requeues itself
	 * waits for the next poll. */
	struct work_struct *last = work_tail;

	while (work_head) {
		struct work_struct *work = work_head;
		bool final = work == last;

		work_head = work->next;
		if (!work_head)
			work_tail = 0;
		work->pending = false;
		work_depth++;
		work->func(work);
		work_depth--;
		if (final)
			break;
	}
}

void flush_workqueue(struct workqueue_struct *queue)
{
	(void)queue;
	run_work();
}

struct workqueue_struct *alloc_workqueue(const char *name, unsigned int flags, int max_active, ...)
{
	struct workqueue_struct *queue = kzalloc(sizeof(*queue), GFP_KERNEL);

	(void)flags;
	(void)max_active;
	if (queue)
		queue->name = name;
	return queue;
}

static void run_timers(void)
{
	unsigned long now = jiffies;
	struct timer_list *due = 0;

	for (struct timer_list **link = &timers; *link;) {
		struct timer_list *timer = *link;
		if (time_after_eq(now, timer->expires)) {
			*link = timer->next;
			timer->pending = false;
			timer->next = due;
			due = timer;
		} else {
			link = &timer->next;
		}
	}
	while (due) {
		struct timer_list *timer = due;
		due = timer->next;
		timer->next = 0;
		timer->function(timer);
	}
}

int request_irq(unsigned int irq, irq_handler_t handler, unsigned long flags, const char *name,
		void *data)
{
	(void)flags;
	(void)name;
	for (int i = 0; i < MAX_IRQS; i++) {
		if (!irqs[i].handler) {
			irqs[i].irq = irq;
			irqs[i].handler = handler;
			irqs[i].data = data;
			return 0;
		}
	}
	return -EBUSY;
}

void free_irq(unsigned int irq, void *data)
{
	for (int i = 0; i < MAX_IRQS; i++)
		if (irqs[i].handler && irqs[i].irq == irq && irqs[i].data == data)
			irqs[i].handler = 0;
}

void aeros_lock_twice(const void *lock)
{
	printk("lock %p taken twice (the Linux side is single-threaded)\n", lock);
}

static struct irq_work *irq_works;
static struct rcu_head *rcu_callbacks;

bool irq_work_queue(struct irq_work *work)
{
	if (work->pending)
		return false;
	work->pending = true;
	work->next = irq_works;
	irq_works = work;
	return true;
}

static void run_irq_works(void)
{
	while (irq_works) {
		struct irq_work *work = irq_works;

		irq_works = work->next;
		work->pending = false;
		work->func(work);
	}
}

void irq_work_sync(struct irq_work *work)
{
	if (work->pending)
		run_irq_works();
}

void call_rcu(struct rcu_head *head, rcu_callback_t func)
{
	head->func = func;
	head->next = rcu_callbacks;
	rcu_callbacks = head;
}

void aeros_kfree_rcu(struct rcu_head *head, unsigned long offset)
{
	call_rcu(head, (rcu_callback_t)offset);
}

static void run_rcu(void)
{
	struct rcu_head *head = rcu_callbacks;

	rcu_callbacks = NULL;
	while (head) {
		struct rcu_head *next = head->next;

		if ((unsigned long)head->func < 4096)
			kfree((char *)head - (unsigned long)head->func);
		else
			head->func(head);
		head = next;
	}
}

void rcu_barrier(void)
{
	while (rcu_callbacks)
		run_rcu();
}

/* One step of a wait: interrupt handlers, timers, and queued work unless
 * the waiter is itself a work item (which must not run the others
 * underneath itself). */
void aeros_wait_step(void)
{
	for (int i = 0; i < MAX_IRQS; i++)
		if (irqs[i].handler)
			irqs[i].handler((int)irqs[i].irq, irqs[i].data);
	run_irq_works();
	run_timers();
	if (!work_depth)
		run_work();
	__asm__ volatile("pause");
}

void aeros_linux_poll(void)
{
	for (int i = 0; i < MAX_IRQS; i++)
		if (irqs[i].handler)
			irqs[i].handler((int)irqs[i].irq, irqs[i].data);
	run_irq_works();
	run_timers();
	run_work();
	run_rcu();
}
