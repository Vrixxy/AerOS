#ifndef _LINUX_LLIST_H
#define _LINUX_LLIST_H

#include <linux/kernel.h>

struct llist_node {
	struct llist_node *next;
};
struct llist_head {
	struct llist_node *first;
};

#define LLIST_HEAD_INIT(name) { NULL }
#define LLIST_HEAD(name) struct llist_head name = LLIST_HEAD_INIT(name)
static inline void init_llist_head(struct llist_head *list) { list->first = NULL; }
static inline bool llist_empty(const struct llist_head *head) { return __atomic_load_n(&head->first, __ATOMIC_RELAXED) == NULL; }
static inline bool llist_add(struct llist_node *node, struct llist_head *head)
{
	struct llist_node *first = __atomic_load_n(&head->first, __ATOMIC_RELAXED);

	do {
		node->next = first;
	} while (!__atomic_compare_exchange_n(&head->first, &first, node, false, __ATOMIC_SEQ_CST, __ATOMIC_RELAXED));
	return first == NULL;
}
static inline struct llist_node *llist_del_all(struct llist_head *head)
{
	return __atomic_exchange_n(&head->first, NULL, __ATOMIC_SEQ_CST);
}
static inline struct llist_node *llist_reverse_order(struct llist_node *head)
{
	struct llist_node *reversed = NULL;

	while (head) {
		struct llist_node *next = head->next;

		head->next = reversed;
		reversed = head;
		head = next;
	}
	return reversed;
}
#define llist_entry(ptr, type, member) container_of(ptr, type, member)
#define llist_for_each_entry_safe(pos, n, node, member) \
	for (pos = (node) ? llist_entry((node), __typeof__(*pos), member) : NULL; \
	     pos && ((n = pos->member.next ? llist_entry(pos->member.next, __typeof__(*pos), member) : NULL), 1); \
	     pos = n)
#define llist_for_each_entry(pos, node, member) \
	for (pos = (node) ? llist_entry((node), __typeof__(*pos), member) : NULL; pos; \
	     pos = pos->member.next ? llist_entry(pos->member.next, __typeof__(*pos), member) : NULL)

#endif
