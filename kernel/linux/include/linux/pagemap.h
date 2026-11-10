#ifndef _LINUX_PAGEMAP_H
#define _LINUX_PAGEMAP_H
#include <linux/mm.h>
struct address_space {
	void *host;
	gfp_t gfp_mask;
};
#define mapping_set_unevictable(mapping) do { (void)(mapping); } while (0)
#define mapping_clear_unevictable(mapping) do { (void)(mapping); } while (0)
#define check_move_unevictable_folios(fbatch) do { (void)(fbatch); } while (0)
#define mapping_gfp_constraint(mapping, gfp) (mapping_gfp_mask(mapping) & (gfp))
static inline gfp_t mapping_gfp_mask(struct address_space *mapping) { return mapping ? mapping->gfp_mask : GFP_KERNEL; }
static inline void mapping_set_gfp_mask(struct address_space *mapping, gfp_t mask) { mapping->gfp_mask = mask; }
static inline void address_space_init_once(struct address_space *mapping) { mapping->host = NULL; mapping->gfp_mask = GFP_KERNEL; }
#endif
