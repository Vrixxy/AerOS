#ifndef _LINUX_LIST_H
#define _LINUX_LIST_H

#include <linux/kernel.h>

#define LIST_HEAD_INIT(name) { &(name), &(name) }
#define LIST_HEAD(name) struct list_head name = LIST_HEAD_INIT(name)

static inline void INIT_LIST_HEAD(struct list_head *list) { list->next = list->prev = list; }

static inline void __list_add(struct list_head *entry, struct list_head *prev, struct list_head *next)
{
	next->prev = entry;
	entry->next = next;
	entry->prev = prev;
	prev->next = entry;
}

static inline void list_add(struct list_head *entry, struct list_head *head) { __list_add(entry, head, head->next); }
static inline void list_add_tail(struct list_head *entry, struct list_head *head) { __list_add(entry, head->prev, head); }

static inline void list_del(struct list_head *entry)
{
	entry->next->prev = entry->prev;
	entry->prev->next = entry->next;
	entry->next = entry->prev = 0;
}

static inline void list_del_init(struct list_head *entry)
{
	entry->next->prev = entry->prev;
	entry->prev->next = entry->next;
	INIT_LIST_HEAD(entry);
}

static inline void list_move_tail(struct list_head *entry, struct list_head *head)
{
	list_del(entry);
	list_add_tail(entry, head);
}

static inline void __list_del_entry(struct list_head *entry)
{
	entry->next->prev = entry->prev;
	entry->prev->next = entry->next;
}

static inline void list_move(struct list_head *entry, struct list_head *head)
{
	__list_del_entry(entry);
	list_add(entry, head);
}

static inline void list_replace(struct list_head *old, struct list_head *new_entry)
{
	new_entry->next = old->next;
	new_entry->next->prev = new_entry;
	new_entry->prev = old->prev;
	new_entry->prev->next = new_entry;
}

static inline void list_splice_tail_init(struct list_head *list, struct list_head *head)
{
	if (list->next != list) {
		struct list_head *first = list->next, *last = list->prev, *at = head->prev;
		first->prev = at;
		at->next = first;
		last->next = head;
		head->prev = last;
		list->next = list->prev = list;
	}
}

static inline void __list_splice(const struct list_head *list, struct list_head *prev, struct list_head *next)
{
	struct list_head *first = list->next, *last = list->prev;

	first->prev = prev;
	prev->next = first;
	last->next = next;
	next->prev = last;
}
static inline int list_empty(const struct list_head *head);
static inline void list_splice(const struct list_head *list, struct list_head *head)
{
	if (!list_empty(list))
		__list_splice(list, head, head->next);
}
static inline void list_splice_tail(const struct list_head *list, struct list_head *head)
{
	if (!list_empty(list))
		__list_splice(list, head->prev, head);
}
static inline void list_splice_init(struct list_head *list, struct list_head *head)
{
	if (!list_empty(list)) {
		__list_splice(list, head, head->next);
		INIT_LIST_HEAD(list);
	}
}
static inline int list_is_singular(const struct list_head *head)
{
	return head->next != head && head->next == head->prev;
}
static inline int list_is_first(const struct list_head *entry, const struct list_head *head)
{
	return entry->prev == head;
}
static inline int list_is_head(const struct list_head *entry, const struct list_head *head)
{
	return entry == head;
}
static inline void list_replace_init(struct list_head *old, struct list_head *new_entry)
{
	list_replace(old, new_entry);
	INIT_LIST_HEAD(old);
}
static inline void list_bulk_move_tail(struct list_head *head, struct list_head *first, struct list_head *last)
{
	first->prev->next = last->next;
	last->next->prev = first->prev;
	head->prev->next = first;
	first->prev = head->prev;
	last->next = head;
	head->prev = last;
}
static inline size_t list_count_nodes(const struct list_head *head)
{
	const struct list_head *pos;
	size_t count = 0;

	for (pos = head->next; pos != head; pos = pos->next)
		count++;
	return count;
}

static inline int list_is_last(const struct list_head *entry, const struct list_head *head)
{
	return entry->next == head;
}

static inline int list_empty(const struct list_head *head) { return head->next == head; }

#define list_entry(ptr, type, member) container_of(ptr, type, member)
#define list_first_entry(head, type, member) list_entry((head)->next, type, member)
#define list_first_entry_or_null(head, type, member) \
	(list_empty(head) ? NULL : list_first_entry(head, type, member))
#define list_next_entry(pos, member) list_entry((pos)->member.next, __typeof__(*(pos)), member)
#define list_last_entry(head, type, member) list_entry((head)->prev, type, member)
#define list_prev_entry(pos, member) list_entry((pos)->member.prev, __typeof__(*(pos)), member)
#define list_for_each_entry_reverse(pos, head, member) 	for (pos = list_last_entry(head, __typeof__(*pos), member); &pos->member != (head); 	     pos = list_prev_entry(pos, member))
#define list_for_each_entry_continue(pos, head, member) 	for (pos = list_next_entry(pos, member); &pos->member != (head); pos = list_next_entry(pos, member))
#define list_for_each_entry_from(pos, head, member) 	for (; &pos->member != (head); pos = list_next_entry(pos, member))
#define list_for_each_safe(pos, n, head) 	for (pos = (head)->next, n = pos->next; pos != (head); pos = n, n = pos->next)
#define list_for_each_prev(pos, head) for (pos = (head)->prev; pos != (head); pos = pos->prev)
#define list_for_each_entry_safe_reverse(pos, n, head, member) 	for (pos = list_last_entry(head, __typeof__(*pos), member), n = list_prev_entry(pos, member); 	     &pos->member != (head); pos = n, n = list_prev_entry(n, member))
#define list_for_each_entry_safe_from(pos, n, head, member) 	for (n = list_next_entry(pos, member); &pos->member != (head); pos = n, n = list_next_entry(n, member))
#define list_for_each_entry_continue_reverse(pos, head, member) 	for (pos = list_prev_entry(pos, member); &pos->member != (head); pos = list_prev_entry(pos, member))
#define list_prepare_entry(pos, head, member) ((pos) ? : list_entry(head, __typeof__(*pos), member))
#define list_last_entry_or_null(head, type, member) (list_empty(head) ? NULL : list_last_entry(head, type, member))
#define list_for_each_entry_from_reverse(pos, head, member) \
	for (; &pos->member != (head); pos = list_prev_entry(pos, member))
#define list_for_each(pos, head) for (pos = (head)->next; pos != (head); pos = pos->next)
#define list_for_each_entry(pos, head, member) \
	for (pos = list_first_entry(head, __typeof__(*pos), member); &pos->member != (head); \
	     pos = list_next_entry(pos, member))
#define list_for_each_entry_safe(pos, n, head, member) \
	for (pos = list_first_entry(head, __typeof__(*pos), member), n = list_next_entry(pos, member); \
	     &pos->member != (head); pos = n, n = list_next_entry(n, member))


struct hlist_head {
	struct hlist_node *first;
};
struct hlist_node {
	struct hlist_node *next, **pprev;
};
#define HLIST_HEAD_INIT { .first = NULL }
#define HLIST_HEAD(name) struct hlist_head name = { .first = NULL }
#define INIT_HLIST_HEAD(ptr) ((ptr)->first = NULL)
static inline void INIT_HLIST_NODE(struct hlist_node *h) { h->next = NULL; h->pprev = NULL; }
static inline int hlist_unhashed(const struct hlist_node *h) { return !h->pprev; }
static inline int hlist_empty(const struct hlist_head *h) { return !h->first; }
static inline void __hlist_del(struct hlist_node *n)
{
	struct hlist_node *next = n->next, **pprev = n->pprev;

	*pprev = next;
	if (next)
		next->pprev = pprev;
}
static inline void hlist_del(struct hlist_node *n)
{
	__hlist_del(n);
	n->next = NULL;
	n->pprev = NULL;
}
static inline void hlist_del_init(struct hlist_node *n)
{
	if (!hlist_unhashed(n)) {
		__hlist_del(n);
		INIT_HLIST_NODE(n);
	}
}
static inline void hlist_add_head(struct hlist_node *n, struct hlist_head *h)
{
	struct hlist_node *first = h->first;

	n->next = first;
	if (first)
		first->pprev = &n->next;
	h->first = n;
	n->pprev = &h->first;
}
#define hlist_del_rcu hlist_del
#define hlist_add_head_rcu hlist_add_head
#define hlist_entry(ptr, type, member) container_of(ptr, type, member)
#define hlist_entry_safe(ptr, type, member) \
	({ __typeof__(ptr) ____ptr = (ptr); ____ptr ? hlist_entry(____ptr, type, member) : NULL; })
#define hlist_for_each(pos, head) for (pos = (head)->first; pos; pos = pos->next)
#define hlist_for_each_entry(pos, head, member) \
	for (pos = hlist_entry_safe((head)->first, __typeof__(*(pos)), member); pos; \
	     pos = hlist_entry_safe((pos)->member.next, __typeof__(*(pos)), member))
#define hlist_for_each_entry_rcu(pos, head, member, ...) hlist_for_each_entry(pos, head, member)
#define hlist_for_each_entry_safe(pos, n, head, member) \
	for (pos = hlist_entry_safe((head)->first, __typeof__(*pos), member); \
	     pos && ({ n = pos->member.next; 1; }); \
	     pos = hlist_entry_safe(n, __typeof__(*pos), member))

static inline void list_rotate_to_front(struct list_head *list, struct list_head *head)
{
	list_del(head);
	list_add_tail(head, list);
}

#define list_for_each_prev_safe(pos, n, head) \
	for (pos = (head)->prev, n = pos->prev; pos != (head); pos = n, n = pos->prev)

#endif
