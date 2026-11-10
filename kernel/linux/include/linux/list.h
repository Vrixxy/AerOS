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
#define list_for_each(pos, head) for (pos = (head)->next; pos != (head); pos = pos->next)
#define list_for_each_entry(pos, head, member) \
	for (pos = list_first_entry(head, __typeof__(*pos), member); &pos->member != (head); \
	     pos = list_next_entry(pos, member))
#define list_for_each_entry_safe(pos, n, head, member) \
	for (pos = list_first_entry(head, __typeof__(*pos), member), n = list_next_entry(pos, member); \
	     &pos->member != (head); pos = n, n = list_next_entry(n, member))

#endif
