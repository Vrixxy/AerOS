#ifndef _LINUX_PAGEVEC_H
#define _LINUX_PAGEVEC_H

#include <linux/mm.h>
#define PAGEVEC_SIZE 31
struct folio_batch {
	unsigned char nr;
	struct folio *folios[PAGEVEC_SIZE];
};
static inline void folio_batch_init(struct folio_batch *fbatch) { fbatch->nr = 0; }
static inline unsigned int folio_batch_count(struct folio_batch *fbatch) { return fbatch->nr; }
static inline unsigned int folio_batch_space(struct folio_batch *fbatch) { return PAGEVEC_SIZE - fbatch->nr; }
static inline unsigned int folio_batch_add(struct folio_batch *fbatch, struct folio *folio)
{
	fbatch->folios[fbatch->nr++] = folio;
	return folio_batch_space(fbatch);
}
#define __folio_batch_release folio_batch_release
static inline void folio_batch_release(struct folio_batch *fbatch)
{
	unsigned int i;

	for (i = 0; i < fbatch->nr; i++)
		folio_put(fbatch->folios[i]);
	fbatch->nr = 0;
}

#endif
