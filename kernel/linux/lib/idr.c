// SPDX-License-Identifier: MIT
#include <linux/idr.h>
#include <linux/errno.h>
#include <linux/string.h>

static int grow(struct idr *idr, unsigned int needed)
{
	unsigned int capacity = idr->capacity ? idr->capacity : 16;
	void **slots;

	while (capacity < needed)
		capacity *= 2;
	if (capacity == idr->capacity)
		return 0;
	slots = kzalloc(capacity * sizeof(void *), GFP_KERNEL);
	if (!slots)
		return -ENOMEM;
	if (idr->slots) {
		memcpy(slots, idr->slots, idr->capacity * sizeof(void *));
		kfree(idr->slots);
	}
	idr->slots = slots;
	idr->capacity = capacity;
	return 0;
}

static int alloc_from(struct idr *idr, void *ptr, unsigned int start, unsigned int end)
{
	unsigned int id;

	if (start < idr->base)
		start = idr->base;
	for (id = start; id < end; id++) {
		unsigned int slot = id - idr->base;

		if (slot >= idr->capacity && grow(idr, slot + 1))
			return -ENOMEM;
		if (!idr->slots[slot]) {
			idr->slots[slot] = ptr ? ptr : (void *)1;
			idr->next = id + 1;
			return (int)id;
		}
	}
	return -ENOSPC;
}

int idr_alloc(struct idr *idr, void *ptr, int start, int end, gfp_t gfp)
{
	(void)gfp;
	if (start < 0)
		return -EINVAL;
	return alloc_from(idr, ptr, (unsigned int)start, end <= 0 ? 0x80000000u : (unsigned int)end);
}

int idr_alloc_cyclic(struct idr *idr, void *ptr, int start, int end, gfp_t gfp)
{
	unsigned int limit = end <= 0 ? 0x80000000u : (unsigned int)end;
	unsigned int from = idr->next > (unsigned int)start ? idr->next : (unsigned int)start;
	int id;

	(void)gfp;
	id = alloc_from(idr, ptr, from, limit);
	if (id == -ENOSPC && from > (unsigned int)start)
		id = alloc_from(idr, ptr, (unsigned int)start, limit);
	return id;
}

static void **slot_of(const struct idr *idr, unsigned long id)
{
	if (id < idr->base || id - idr->base >= idr->capacity)
		return NULL;
	return &idr->slots[id - idr->base];
}

void *idr_find(const struct idr *idr, unsigned long id)
{
	void **slot = slot_of(idr, id);

	return slot && *slot != (void *)1 ? *slot : NULL;
}

void *idr_remove(struct idr *idr, unsigned long id)
{
	void **slot = slot_of(idr, id);
	void *old;

	if (!slot)
		return NULL;
	old = *slot;
	*slot = NULL;
	return old == (void *)1 ? NULL : old;
}

void *idr_replace(struct idr *idr, void *ptr, unsigned long id)
{
	void **slot = slot_of(idr, id);
	void *old;

	if (!slot || !*slot)
		return ERR_PTR(-ENOENT);
	old = *slot;
	*slot = ptr ? ptr : (void *)1;
	return old == (void *)1 ? NULL : old;
}

void *idr_get_next(struct idr *idr, int *nextid)
{
	unsigned long id = *nextid < (int)idr->base ? idr->base : (unsigned long)*nextid;

	for (; id - idr->base < idr->capacity; id++) {
		void *entry = idr->slots[id - idr->base];

		if (entry && entry != (void *)1) {
			*nextid = (int)id;
			return entry;
		}
	}
	return NULL;
}

int idr_for_each(const struct idr *idr, int (*fn)(int id, void *p, void *data), void *data)
{
	unsigned int slot;

	for (slot = 0; slot < idr->capacity; slot++) {
		void *entry = idr->slots[slot];

		if (entry && entry != (void *)1) {
			int error = fn((int)(slot + idr->base), entry, data);

			if (error)
				return error;
		}
	}
	return 0;
}

void idr_destroy(struct idr *idr)
{
	kfree(idr->slots);
	idr->slots = NULL;
	idr->capacity = 0;
	idr->next = 0;
}

int ida_alloc_range(struct ida *ida, unsigned int min, unsigned int max, gfp_t gfp)
{
	(void)gfp;
	if (min > max || max >= 0x80000000u)
		return -EINVAL;
	return alloc_from(&ida->idr, NULL, min, max + 1);
}

void ida_free(struct ida *ida, unsigned int id)
{
	idr_remove(&ida->idr, id);
}

void ida_destroy(struct ida *ida)
{
	idr_destroy(&ida->idr);
}
