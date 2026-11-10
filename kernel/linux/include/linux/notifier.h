#ifndef _LINUX_NOTIFIER_H
#define _LINUX_NOTIFIER_H

#include <linux/types.h>
struct notifier_block {
	int (*notifier_call)(struct notifier_block *nb, unsigned long action, void *data);
	struct notifier_block *next;
	int priority;
};
struct blocking_notifier_head {
	struct notifier_block *head;
};
struct atomic_notifier_head {
	struct notifier_block *head;
};
#define NOTIFY_DONE 0x0000
#define NOTIFY_OK 0x0001
#define NOTIFY_STOP_MASK 0x8000
#define NOTIFY_BAD (NOTIFY_STOP_MASK | 0x0002)
#define BLOCKING_NOTIFIER_HEAD(name) struct blocking_notifier_head name = { NULL }
#define BLOCKING_INIT_NOTIFIER_HEAD(name) ((name)->head = NULL)
static inline int blocking_notifier_chain_register(struct blocking_notifier_head *head, struct notifier_block *nb)
{
	nb->next = head->head;
	head->head = nb;
	return 0;
}
static inline int blocking_notifier_chain_unregister(struct blocking_notifier_head *head, struct notifier_block *nb)
{
	struct notifier_block **link = &head->head;

	while (*link && *link != nb)
		link = &(*link)->next;
	if (*link)
		*link = nb->next;
	return 0;
}
static inline int blocking_notifier_call_chain(struct blocking_notifier_head *head, unsigned long action, void *data)
{
	struct notifier_block *nb;
	int result = NOTIFY_DONE;

	for (nb = head->head; nb; nb = nb->next) {
		result = nb->notifier_call(nb, action, data);
		if (result & NOTIFY_STOP_MASK)
			break;
	}
	return result;
}

#endif
