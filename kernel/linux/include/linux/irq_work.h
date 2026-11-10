#ifndef _LINUX_IRQ_WORK_H
#define _LINUX_IRQ_WORK_H

#include <linux/types.h>

/* Runs on the next wait step or poll, never from inside irq_work_queue. */
struct irq_work {
	struct irq_work *next;
	void (*func)(struct irq_work *work);
	bool pending;
};
#define IRQ_WORK_INIT(f) { NULL, (f), false }
#define DEFINE_IRQ_WORK(name, f) struct irq_work name = IRQ_WORK_INIT(f)
static inline void init_irq_work(struct irq_work *work, void (*func)(struct irq_work *work))
{
	work->next = NULL;
	work->func = func;
	work->pending = false;
}
bool irq_work_queue(struct irq_work *work);
void irq_work_sync(struct irq_work *work);
static inline bool irq_work_is_pending(struct irq_work *work) { return work->pending; }

#endif
