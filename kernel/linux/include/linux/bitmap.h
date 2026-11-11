#ifndef _LINUX_BITMAP_H
#define _LINUX_BITMAP_H

#include <linux/bitops.h>
#include <linux/string.h>

#define BITMAP_FIRST_WORD_MASK(start) (~0UL << ((start) & (BITS_PER_LONG - 1)))
#define BITMAP_LAST_WORD_MASK(nbits) (~0UL >> (-(nbits) & (BITS_PER_LONG - 1)))

static inline void bitmap_zero(unsigned long *dst, unsigned int nbits)
{
	memset(dst, 0, BITS_TO_LONGS(nbits) * sizeof(unsigned long));
}
static inline void bitmap_fill(unsigned long *dst, unsigned int nbits)
{
	memset(dst, 0xff, BITS_TO_LONGS(nbits) * sizeof(unsigned long));
}
static inline void bitmap_copy(unsigned long *dst, const unsigned long *src, unsigned int nbits)
{
	memcpy(dst, src, BITS_TO_LONGS(nbits) * sizeof(unsigned long));
}
static inline void bitmap_set(unsigned long *map, unsigned int start, unsigned int count)
{
	while (count--)
		__set_bit(start++, map);
}
static inline void bitmap_clear(unsigned long *map, unsigned int start, unsigned int count)
{
	while (count--)
		__clear_bit(start++, map);
}
static inline unsigned int bitmap_weight(const unsigned long *src, unsigned int nbits)
{
	unsigned int i, weight = 0;

	for (i = 0; i < nbits / BITS_PER_LONG; i++)
		weight += (unsigned int)hweight_long(src[i]);
	if (nbits % BITS_PER_LONG)
		weight += (unsigned int)hweight_long(src[i] & BITMAP_LAST_WORD_MASK(nbits));
	return weight;
}
static inline bool bitmap_empty(const unsigned long *src, unsigned int nbits)
{
	return bitmap_weight(src, nbits) == 0;
}
static inline bool bitmap_full(const unsigned long *src, unsigned int nbits)
{
	return bitmap_weight(src, nbits) == nbits;
}
static inline void bitmap_or(unsigned long *dst, const unsigned long *a, const unsigned long *b, unsigned int nbits)
{
	unsigned int i;

	for (i = 0; i < BITS_TO_LONGS(nbits); i++)
		dst[i] = a[i] | b[i];
}
static inline bool bitmap_and(unsigned long *dst, const unsigned long *a, const unsigned long *b, unsigned int nbits)
{
	unsigned int i;
	unsigned long any = 0;

	for (i = 0; i < BITS_TO_LONGS(nbits); i++)
		any |= dst[i] = a[i] & b[i];
	return any != 0;
}
static inline bool bitmap_equal(const unsigned long *a, const unsigned long *b, unsigned int nbits)
{
	unsigned int i;

	for (i = 0; i < nbits; i++)
		if (test_bit(i, a) != test_bit(i, b))
			return false;
	return true;
}
static inline unsigned long find_last_bit(const unsigned long *addr, unsigned long size)
{
	while (size--)
		if (test_bit((long)size, addr))
			return size;
	return size;
}

#include <linux/slab.h>
static inline unsigned long *bitmap_zalloc(unsigned int nbits, gfp_t flags)
{
	return kcalloc(BITS_TO_LONGS(nbits), sizeof(unsigned long), flags);
}
static inline unsigned long *bitmap_alloc(unsigned int nbits, gfp_t flags)
{
	return kmalloc_array(BITS_TO_LONGS(nbits), sizeof(unsigned long), flags);
}
static inline void bitmap_free(const unsigned long *bitmap) { kfree(bitmap); }

static inline void bitmap_complement(unsigned long *dst, const unsigned long *src, unsigned int nbits)
{
	unsigned int i;

	for (i = 0; i < BITS_TO_LONGS(nbits); i++)
		dst[i] = ~src[i];
}

static inline void bitmap_to_arr32(u32 *buf, const unsigned long *bitmap, unsigned int nbits)
{
	unsigned int i;

	for (i = 0; i < (nbits + 31) / 32; i++)
		buf[i] = (u32)(bitmap[i / 2] >> ((i % 2) * 32));
	if (nbits % 32)
		buf[i - 1] &= (1u << (nbits % 32)) - 1;
}
static inline void bitmap_from_arr32(unsigned long *bitmap, const u32 *buf, unsigned int nbits)
{
	unsigned int i;

	for (i = 0; i < BITS_TO_LONGS(nbits); i++)
		bitmap[i] = buf[2 * i] | ((2 * i + 1 < (nbits + 31) / 32) ? (unsigned long)buf[2 * i + 1] << 32 : 0);
}

#endif
