#ifndef _LINUX_SORT_H
#define _LINUX_SORT_H

#include <linux/types.h>

typedef int (*cmp_func_t)(const void *a, const void *b);
typedef void (*swap_func_t)(void *a, void *b, int size);
void sort(void *base, size_t num, size_t size, cmp_func_t cmp, swap_func_t swap);
#define sort_r(base, num, size, cmp, swap, priv) sort((base), (num), (size), (cmp_func_t)(cmp), (swap))

#endif
