#ifndef _LINUX_INTERRUPT_H
#define _LINUX_INTERRUPT_H

#include <linux/types.h>

#include <linux/irqreturn.h>

typedef irqreturn_t (*irq_handler_t)(int irq, void *data);

#define IRQF_SHARED 0x80
#define IRQF_ONESHOT 0x2000

/* AerOS calls every handler from its poll loop; a handler checks its own
 * device's status, as a shared interrupt handler does anyway. */
int request_irq(unsigned int irq, irq_handler_t handler, unsigned long flags, const char *name,
		void *data);
void free_irq(unsigned int irq, void *data);
static inline void disable_irq(unsigned int irq) { (void)irq; }
static inline void enable_irq(unsigned int irq) { (void)irq; }
static inline void synchronize_irq(unsigned int irq) { (void)irq; }

#endif
