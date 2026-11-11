// SPDX-License-Identifier: MIT
#include <linux/kernel.h>
#include <linux/sort.h>
#include <linux/list_sort.h>

static void swap_bytes(void *a, void *b, int size)
{
	char *x = a, *y = b;

	while (size--) {
		char t = *x;

		*x++ = *y;
		*y++ = t;
	}
}

/* Heapsort: no recursion, no allocation, not stable (as in Linux). */
void sort(void *base, size_t num, size_t size, cmp_func_t cmp, swap_func_t swap_fn)
{
	char *array = base;
	size_t start, end, root, child;

	if (!swap_fn)
		swap_fn = swap_bytes;
	if (num < 2)
		return;
	for (start = num / 2; start-- > 0;) {
		for (root = start; (child = 2 * root + 1) < num; root = child) {
			if (child + 1 < num && cmp(array + child * size, array + (child + 1) * size) < 0)
				child++;
			if (cmp(array + root * size, array + child * size) >= 0)
				break;
			swap_fn(array + root * size, array + child * size, (int)size);
		}
	}
	for (end = num - 1; end > 0; end--) {
		swap_fn(array, array + end * size, (int)size);
		for (root = 0; (child = 2 * root + 1) < end; root = child) {
			if (child + 1 < end && cmp(array + child * size, array + (child + 1) * size) < 0)
				child++;
			if (cmp(array + root * size, array + child * size) >= 0)
				break;
			swap_fn(array + root * size, array + child * size, (int)size);
		}
	}
}

/* Stable bottom-up merge sort on a singly linked chain, then relinked. */
static struct list_head *merge(void *priv, list_cmp_func_t cmp, struct list_head *a, struct list_head *b)
{
	struct list_head head, *tail = &head;

	while (a && b) {
		if (cmp(priv, a, b) <= 0) {
			tail->next = a;
			a = a->next;
		} else {
			tail->next = b;
			b = b->next;
		}
		tail = tail->next;
	}
	tail->next = a ? a : b;
	return head.next;
}

void list_sort(void *priv, struct list_head *head, list_cmp_func_t cmp)
{
	struct list_head *runs[64] = { NULL }, *chain, *prev;
	int i;

	if (list_empty(head) || head->next == head->prev)
		return;
	head->prev->next = NULL;
	chain = head->next;
	while (chain) {
		struct list_head *one = chain;

		chain = chain->next;
		one->next = NULL;
		for (i = 0; runs[i]; i++) {
			one = merge(priv, cmp, runs[i], one);
			runs[i] = NULL;
		}
		runs[i] = one;
	}
	chain = NULL;
	for (i = 0; i < 64; i++)
		if (runs[i])
			chain = chain ? merge(priv, cmp, runs[i], chain) : runs[i];
	prev = head;
	for (; chain; chain = chain->next) {
		prev->next = chain;
		chain->prev = prev;
		prev = chain;
	}
	prev->next = head;
	head->prev = prev;
}
