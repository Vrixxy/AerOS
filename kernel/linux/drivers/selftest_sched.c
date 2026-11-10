// SPDX-License-Identifier: MIT
/* Boot self-test of the unmodified DRM GPU scheduler (how amdgpu feeds
 * every ring): two entities submit jobs to a pretend ring whose hardware
 * fences complete 3 ms after a job starts; one job waits on an outside
 * fence. Checks order, the dependency, credits and that every job is
 * freed. */
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/timer.h>
#include <linux/device.h>
#include <linux/wait.h>
#include <drm/gpu_scheduler.h>

#define JOBS 8
#define BLOCKED 5

struct test_job {
	struct drm_sched_job base;
	struct timer_list timer;
	struct dma_fence *hw;
	int id;
};

static DEFINE_SPINLOCK(hw_lock);
static u64 hw_context;
static u64 hw_seqno;
static int ran[JOBS], ran_count, freed, in_flight, most_in_flight;
static struct dma_fence *outside;
static u64 outside_signalled_ns, blocked_ran_ns;

static const char *hw_driver(struct dma_fence *fence) { return "aeros"; }
static const char *hw_timeline(struct dma_fence *fence) { return "test-ring"; }
static const struct dma_fence_ops hw_ops = {
	.get_driver_name = hw_driver,
	.get_timeline_name = hw_timeline,
};

static void hw_complete(struct timer_list *timer)
{
	struct test_job *job = container_of(timer, struct test_job, timer);

	in_flight--;
	dma_fence_signal(job->hw);
}

static struct dma_fence *run_job(struct drm_sched_job *sched_job)
{
	struct test_job *job = container_of(sched_job, struct test_job, base);

	job->hw = kzalloc(sizeof(*job->hw), GFP_KERNEL);
	dma_fence_init(job->hw, &hw_ops, &hw_lock, hw_context, ++hw_seqno);
	if (ran_count < JOBS)
		ran[ran_count++] = job->id;
	if (job->id == BLOCKED)
		blocked_ran_ns = ktime_get_ns();
	if (++in_flight > most_in_flight)
		most_in_flight = in_flight;
	timer_setup(&job->timer, hw_complete, 0);
	mod_timer(&job->timer, jiffies + msecs_to_jiffies(3));
	return dma_fence_get(job->hw);
}

static enum drm_gpu_sched_stat timedout_job(struct drm_sched_job *sched_job)
{
	pr_err("sched selftest: job %d timed out\n", container_of(sched_job, struct test_job, base)->id);
	return DRM_GPU_SCHED_STAT_NOMINAL;
}

static void free_job(struct drm_sched_job *sched_job)
{
	struct test_job *job = container_of(sched_job, struct test_job, base);

	drm_sched_job_cleanup(sched_job);
	dma_fence_put(job->hw);
	kfree(job);
	freed++;
}

static const struct drm_sched_backend_ops backend = {
	.run_job = run_job,
	.timedout_job = timedout_job,
	.free_job = free_job,
};

static void release_outside(struct timer_list *timer)
{
	outside_signalled_ns = ktime_get_ns();
	dma_fence_signal(outside);
}

static int __init sched_selftest(void)
{
	static struct device device;
	static struct drm_gpu_scheduler sched;
	static struct drm_sched_entity entities[2];
	struct drm_gpu_scheduler *list[1] = { &sched };
	struct dma_fence *finished[JOBS];
	struct timer_list outside_timer;
	int i, error, last_seen[2] = { -1, -1 };

	hw_context = dma_fence_context_alloc(1);
	error = drm_sched_init(&sched, &backend, NULL, DRM_SCHED_PRIORITY_COUNT, 4, 0,
			       msecs_to_jiffies(1000), NULL, NULL, "aeros-test", &device);
	if (error) {
		pr_err("sched selftest: drm_sched_init failed (%d)\n", error);
		return error;
	}
	for (i = 0; i < 2; i++) {
		error = drm_sched_entity_init(&entities[i], DRM_SCHED_PRIORITY_NORMAL, list, 1, NULL);
		if (error)
			return error;
	}
	outside = kzalloc(sizeof(*outside), GFP_KERNEL);
	dma_fence_init(outside, &hw_ops, &hw_lock, dma_fence_context_alloc(1), 1);
	timer_setup(&outside_timer, release_outside, 0);
	mod_timer(&outside_timer, jiffies + msecs_to_jiffies(30));

	for (i = 0; i < JOBS; i++) {
		struct test_job *job = kzalloc(sizeof(*job), GFP_KERNEL);

		job->id = i;
		error = drm_sched_job_init(&job->base, &entities[i % 2], 1, NULL, 0);
		if (error)
			return error;
		if (i == BLOCKED)
			drm_sched_job_add_dependency(&job->base, dma_fence_get(outside));
		drm_sched_job_arm(&job->base);
		finished[i] = dma_fence_get(&job->base.s_fence->finished);
		drm_sched_entity_push_job(&job->base);
	}
	for (i = 0; i < JOBS; i++) {
		if (dma_fence_wait_timeout(finished[i], false, msecs_to_jiffies(1000)) <= 0) {
			pr_err("sched selftest: job %d never finished (%d ran)\n", i, ran_count);
			return -EINVAL;
		}
		dma_fence_put(finished[i]);
	}
	for (i = 0; i < ran_count; i++) {
		int entity = ran[i] % 2;

		if (ran[i] <= last_seen[entity]) {
			pr_err("sched selftest: entity %d ran job %d after job %d\n", entity, ran[i], last_seen[entity]);
			return -EINVAL;
		}
		last_seen[entity] = ran[i];
	}
	if (ran_count != JOBS || blocked_ran_ns < outside_signalled_ns || most_in_flight > 4) {
		pr_err("sched selftest: ran %d jobs, blocked job early %d, %d in flight at once\n", ran_count,
		       blocked_ran_ns < outside_signalled_ns, most_in_flight);
		return -EINVAL;
	}
	{
		DECLARE_WAIT_QUEUE_HEAD(idle);

		if (!wait_event_timeout(idle, freed == JOBS, msecs_to_jiffies(500))) {
			pr_err("sched selftest: only %d of %d jobs freed\n", freed, JOBS);
			return -EINVAL;
		}
	}
	for (i = 0; i < 2; i++)
		drm_sched_entity_destroy(&entities[i]);
	drm_sched_fini(&sched);
	dma_fence_put(outside);
	pr_info("sched selftest: %d jobs ran in entity order (%d %d %d %d %d %d %d %d), at most %d on the ring, job %d started %llu us after its dependency signalled, %d freed\n",
		ran_count, ran[0], ran[1], ran[2], ran[3], ran[4], ran[5], ran[6], ran[7], most_in_flight, BLOCKED,
		(blocked_ran_ns - outside_signalled_ns) / 1000, freed);
	return 0;
}

module_init(sched_selftest);
