#ifndef _LINUX_PREEMPT_H
#define _LINUX_PREEMPT_H
#define preempt_disable() do { } while (0)
#define preempt_enable() do { } while (0)
#define preempt_enable_no_resched() do { } while (0)
#define preempt_count() 0
#define preemptible() 0
#define in_irq() 0
#define in_softirq() 0
#define in_nmi() 0
#define in_hardirq() 0
#define in_interrupt() 0
#define in_atomic() 0
#define in_task() 1
#define irqs_disabled() 0
#endif
