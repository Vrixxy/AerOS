// SPDX-License-Identifier: MIT
/* Boot self-test of the dma-fence layer every GPU driver synchronises
 * with: fences signalled from timers, callbacks, a wait that has to time
 * out, fence arrays and chains (signalled through irq_work) and a
 * reservation object. */
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/timer.h>
#include <linux/jiffies.h>
#include <linux/dma-fence.h>
#include <linux/dma-fence-array.h>
#include <linux/dma-fence-chain.h>
#include <linux/dma-resv.h>

static DEFINE_SPINLOCK(fence_lock);
static u64 context;
static u64 seqno;

static const char *driver_name(struct dma_fence *fence) { return "aeros"; }
static const char *timeline_name(struct dma_fence *fence) { return "selftest"; }
static const struct dma_fence_ops ops = {
	.get_driver_name = driver_name,
	.get_timeline_name = timeline_name,
};

struct timed_fence {
	struct timer_list timer;
	struct dma_fence *fence;
};

static struct dma_fence *new_fence(void)
{
	struct dma_fence *fence = kzalloc(sizeof(*fence), GFP_KERNEL);

	if (fence)
		dma_fence_init(fence, &ops, &fence_lock, context, ++seqno);
	return fence;
}

static void fire(struct timer_list *timer)
{
	struct timed_fence *timed = container_of(timer, struct timed_fence, timer);

	dma_fence_signal(timed->fence);
}

static void signal_later(struct timed_fence *timed, struct dma_fence *fence, unsigned int ms)
{
	timed->fence = fence;
	timer_setup(&timed->timer, fire, 0);
	mod_timer(&timed->timer, jiffies + msecs_to_jiffies(ms));
}

static int callbacks;
static void counted(struct dma_fence *fence, struct dma_fence_cb *cb) { callbacks++; }

#define FAIL(what) do { pr_err("fence selftest: %s\n", what); return -EINVAL; } while (0)

static int __init fence_selftest(void)
{
	struct timed_fence timers[4];
	struct dma_fence *fence, *idle, **parts;
	struct dma_fence_array *array;
	struct dma_fence_chain *first, *second;
	struct dma_fence_cb cb;
	struct dma_resv resv;
	u64 start;
	long left;
	int i;

	context = dma_fence_context_alloc(1);

	fence = new_fence();
	if (!fence || dma_fence_add_callback(fence, &cb, counted))
		FAIL("could not create a fence");
	start = ktime_get_ns();
	signal_later(&timers[0], fence, 20);
	left = dma_fence_wait_timeout(fence, false, msecs_to_jiffies(500));
	if (left <= 0 || !dma_fence_is_signaled(fence) || callbacks != 1)
		FAIL("timer-signalled fence did not complete its wait");
	pr_info("fence selftest: signalled after %llu ms, callback ran, timestamp %lld\n",
		(ktime_get_ns() - start) / 1000000, (long long)dma_fence_timestamp(fence));
	dma_fence_put(fence);

	idle = new_fence();
	start = ktime_get_ns();
	if (dma_fence_wait_timeout(idle, false, msecs_to_jiffies(30)) != 0)
		FAIL("wait on an unsignalled fence did not time out");
	if ((ktime_get_ns() - start) / 1000000 < 29)
		FAIL("timeout came back early");

	parts = kcalloc(3, sizeof(*parts), GFP_KERNEL);
	for (i = 0; i < 3; i++) {
		parts[i] = new_fence();
		signal_later(&timers[i], parts[i], 10 * (i + 1));
	}
	array = dma_fence_array_create(3, parts, dma_fence_context_alloc(1), 1, false);
	if (!array)
		FAIL("could not create a fence array");
	if (dma_fence_wait_timeout(&array->base, false, msecs_to_jiffies(500)) <= 0)
		FAIL("fence array never signalled");
	dma_fence_put(&array->base);

	first = dma_fence_chain_alloc();
	second = dma_fence_chain_alloc();
	fence = new_fence();
	dma_fence_chain_init(first, NULL, fence, 1);
	dma_fence_chain_init(second, &first->base, dma_fence_get(idle), 2);
	signal_later(&timers[0], fence, 5);
	if (dma_fence_wait_timeout(&first->base, false, msecs_to_jiffies(500)) <= 0)
		FAIL("first chain link never signalled");
	if (dma_fence_is_signaled(&second->base))
		FAIL("chain signalled before its last fence");
	dma_fence_signal(idle);
	if (dma_fence_wait_timeout(&second->base, false, msecs_to_jiffies(500)) <= 0)
		FAIL("chain never signalled");
	dma_fence_put(&second->base);

	dma_resv_init(&resv);
	fence = new_fence();
	if (dma_resv_lock(&resv, NULL) || dma_resv_reserve_fences(&resv, 1))
		FAIL("could not lock the reservation object");
	dma_resv_add_fence(&resv, fence, DMA_RESV_USAGE_WRITE);
	dma_resv_unlock(&resv);
	if (dma_resv_test_signaled(&resv, DMA_RESV_USAGE_READ))
		FAIL("reservation object idle with a pending fence");
	signal_later(&timers[3], fence, 15);
	if (dma_resv_wait_timeout(&resv, DMA_RESV_USAGE_READ, false, msecs_to_jiffies(500)) <= 0 ||
	    !dma_resv_test_signaled(&resv, DMA_RESV_USAGE_READ))
		FAIL("reservation object never went idle");
	dma_fence_put(fence);
	dma_resv_fini(&resv);
	dma_fence_put(idle);

	pr_info("fence selftest: timeout, array, chain and reservation object ok\n");
	return 0;
}

module_init(fence_selftest);
