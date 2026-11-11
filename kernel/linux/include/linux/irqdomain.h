#ifndef _AEROS_LINUX_IRQDOMAIN_H
#define _AEROS_LINUX_IRQDOMAIN_H

#include <linux/interrupt.h>

/* A linear interrupt domain for a device's sub-interrupts; handlers run in the caller. */
typedef unsigned long irq_hw_number_t;
struct irq_domain;
struct irq_data {
	unsigned int irq;
	irq_hw_number_t hwirq;
	void *chip_data;
};
struct irq_chip {
	const char *name;
	void (*irq_mask)(struct irq_data *data);
	void (*irq_unmask)(struct irq_data *data);
};
struct irq_domain_ops {
	int (*map)(struct irq_domain *d, unsigned int virq, irq_hw_number_t hw);
	void (*unmap)(struct irq_domain *d, unsigned int virq);
};
typedef void (*irq_flow_handler_t)(void *desc);
static inline void handle_simple_irq(void *desc) { (void)desc; }
static inline struct irq_domain *irq_domain_add_linear(void *of_node, unsigned int size,
						       const struct irq_domain_ops *ops, void *host_data)
{
	(void)of_node; (void)size; (void)ops; (void)host_data;
	return NULL;
}
static inline void irq_domain_remove(struct irq_domain *domain) { (void)domain; }
static inline unsigned int irq_create_mapping(struct irq_domain *domain, irq_hw_number_t hwirq)
{
	(void)domain; (void)hwirq;
	return 0;
}
static inline int generic_handle_domain_irq(struct irq_domain *domain, unsigned int hwirq)
{
	(void)domain; (void)hwirq;
	return -EINVAL;
}
static inline void irq_set_chip_and_handler(unsigned int irq, const struct irq_chip *chip, irq_flow_handler_t handle)
{
	(void)irq; (void)chip; (void)handle;
}
static inline void irq_set_chip_data(unsigned int irq, void *data) { (void)irq; (void)data; }
static inline void *irq_data_get_irq_chip_data(struct irq_data *d) { return d->chip_data; }
#endif
