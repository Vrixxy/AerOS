#ifndef _LINUX_SHRINKER_H
#define _LINUX_SHRINKER_H

#include <linux/types.h>
#include <linux/gfp.h>

/* Nothing asks memory back yet: shrinkers are kept but never called. */
struct shrink_control {
	gfp_t gfp_mask;
	int nid;
	unsigned long nr_to_scan;
	unsigned long nr_scanned;
};
#define SHRINK_STOP (~0UL)
#define SHRINK_EMPTY (~0UL - 1)
struct shrinker {
	unsigned long (*count_objects)(struct shrinker *shrinker, struct shrink_control *sc);
	unsigned long (*scan_objects)(struct shrinker *shrinker, struct shrink_control *sc);
	long batch;
	int seeks;
	unsigned int flags;
	void *private_data;
};
#define DEFAULT_SEEKS 2
struct shrinker *shrinker_alloc(unsigned int flags, const char *fmt, ...);
void shrinker_register(struct shrinker *shrinker);
void shrinker_free(struct shrinker *shrinker);

#endif
