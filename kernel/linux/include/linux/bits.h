#ifndef _LINUX_BITS_H
#define _LINUX_BITS_H

#define BIT(n) (1UL << (n))
#define BIT_ULL(n) (1ULL << (n))
#define GENMASK(h, l) ((~0UL << (l)) & (~0UL >> (63 - (h))))
#define GENMASK_ULL(h, l) ((~0ULL << (l)) & (~0ULL >> (63 - (h))))

#endif
