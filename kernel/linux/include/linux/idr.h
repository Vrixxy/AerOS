#ifndef _LINUX_IDR_H
#define _LINUX_IDR_H

#include <linux/types.h>
#include <linux/slab.h>

/* A growable array of pointers; ids start at 0 (or 1 with IDR_INIT_BASE). */
struct idr {
	void **slots;
	unsigned int capacity;
	unsigned int base;
	unsigned int next;
};

#define IDR_INIT_BASE(name, start) { NULL, 0, (start), 0 }
#define IDR_INIT(name) IDR_INIT_BASE(name, 0)
#define DEFINE_IDR(name) struct idr name = IDR_INIT(name)
static inline void idr_init_base(struct idr *idr, int base)
{
	idr->slots = NULL;
	idr->capacity = 0;
	idr->base = (unsigned int)base;
	idr->next = 0;
}
static inline void idr_init(struct idr *idr) { idr_init_base(idr, 0); }
static inline void idr_preload(gfp_t gfp) { (void)gfp; }
static inline void idr_preload_end(void) { }

int idr_alloc(struct idr *idr, void *ptr, int start, int end, gfp_t gfp);
int idr_alloc_cyclic(struct idr *idr, void *ptr, int start, int end, gfp_t gfp);
void *idr_find(const struct idr *idr, unsigned long id);
void *idr_remove(struct idr *idr, unsigned long id);
void *idr_replace(struct idr *idr, void *ptr, unsigned long id);
void *idr_get_next(struct idr *idr, int *nextid);
int idr_for_each(const struct idr *idr, int (*fn)(int id, void *p, void *data), void *data);
void idr_destroy(struct idr *idr);
static inline bool idr_is_empty(const struct idr *idr)
{
	int id = 0;

	return idr_get_next((struct idr *)idr, &id) == NULL;
}
#define idr_for_each_entry(idr, entry, id) \
	for ((id) = 0; ((entry) = idr_get_next((idr), &(id))) != NULL; (id) += 1U)

struct ida {
	struct idr idr;
};
#define DEFINE_IDA(name) struct ida name = { IDR_INIT(name) }
static inline void ida_init(struct ida *ida) { idr_init(&ida->idr); }
int ida_alloc_range(struct ida *ida, unsigned int min, unsigned int max, gfp_t gfp);
void ida_free(struct ida *ida, unsigned int id);
void ida_destroy(struct ida *ida);
#define ida_alloc(ida, gfp) ida_alloc_range((ida), 0, ~0U >> 1, (gfp))
#define ida_alloc_min(ida, min, gfp) ida_alloc_range((ida), (min), ~0U >> 1, (gfp))
#define ida_alloc_max(ida, max, gfp) ida_alloc_range((ida), 0, (max), (gfp))
#define ida_simple_get(ida, start, end, gfp) ida_alloc_range((ida), (start), (end) ? (end) - 1 : ~0U >> 1, (gfp))
#define ida_simple_remove(ida, id) ida_free((ida), (id))

#endif
