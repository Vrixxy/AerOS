#ifndef _LINUX_MMZONE_H
#define _LINUX_MMZONE_H
#include <linux/mm_types.h>
#define MAX_PAGE_ORDER 10
#define NR_PAGE_ORDERS (MAX_PAGE_ORDER + 1)
#define MAX_ORDER MAX_PAGE_ORDER
#define MAX_NUMNODES 1
#define NUMA_NO_NODE (-1)
#endif
