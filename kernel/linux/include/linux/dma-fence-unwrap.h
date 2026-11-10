#ifndef _LINUX_DMA_FENCE_UNWRAP_H
#define _LINUX_DMA_FENCE_UNWRAP_H

#include <linux/dma-fence.h>
#include <linux/dma-fence-array.h>
#include <linux/dma-fence-chain.h>
/* Walks the plain fences inside chains and arrays. */
struct dma_fence_unwrap {
	struct dma_fence *chain;
	struct dma_fence *array;
	unsigned int index;
};
struct dma_fence *dma_fence_unwrap_first(struct dma_fence *head, struct dma_fence_unwrap *cursor);
struct dma_fence *dma_fence_unwrap_next(struct dma_fence_unwrap *cursor);
#define dma_fence_unwrap_for_each(fence, cursor, head) \
	for (fence = dma_fence_unwrap_first(head, cursor); fence; fence = dma_fence_unwrap_next(cursor))
struct dma_fence *__dma_fence_unwrap_merge(unsigned int count, struct dma_fence **fences,
					   struct dma_fence_unwrap *cursors);
#define dma_fence_unwrap_merge(...) ({ \
	struct dma_fence *__f[] = { __VA_ARGS__ }; \
	struct dma_fence_unwrap __c[ARRAY_SIZE(__f)]; \
	__dma_fence_unwrap_merge(ARRAY_SIZE(__f), __f, __c); })

#endif
