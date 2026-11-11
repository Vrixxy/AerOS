#ifndef _LINUX_SCHED_H
#define _LINUX_SCHED_H

#include <linux/types.h>
#include <linux/jiffies.h>
#include <linux/preempt.h>

/* One cooperative thread: yielding means letting timers, work and
 * interrupt handlers run. */
void aeros_wait_step(void);

#define TASK_RUNNING 0
#define TASK_INTERRUPTIBLE 1
#define TASK_UNINTERRUPTIBLE 2
#define TASK_COMM_LEN 16
#define TASK_NORMAL (TASK_INTERRUPTIBLE | TASK_UNINTERRUPTIBLE)
#define MAX_SCHEDULE_TIMEOUT ((long)(~0UL >> 1))

#define PF_EXITING 0x4u
#define PF_KTHREAD 0x200000u
#define SIGKILL 9

struct task_struct {
	int pid;
	char comm[TASK_COMM_LEN];
	unsigned int flags;
	int exit_code;
	struct task_struct *group_leader;
	struct mm_struct *mm;
	int tgid;
};
struct pid {
	int nr;
};
static inline struct pid *task_pid(struct task_struct *task) { (void)task; return NULL; }
static inline struct pid *task_tgid(struct task_struct *task) { (void)task; return NULL; }
static inline struct pid *get_pid(struct pid *pid) { return pid; }
static inline void put_pid(struct pid *pid) { (void)pid; }
static inline pid_t pid_nr(struct pid *pid) { return pid ? pid->nr : 0; }
static inline pid_t task_pid_vnr(struct task_struct *task) { return task->pid; }
static inline pid_t task_pid_nr_ns(struct task_struct *task, void *ns) { (void)ns; return task->pid; }
#define overflowuid 65534
#define overflowgid 65534
static inline pid_t task_tgid_nr(struct task_struct *task) { return task->tgid; }
static inline pid_t task_tgid_vnr(struct task_struct *task) { return task->tgid; }
#define get_task_comm(buf, task) strscpy((buf), (task)->comm, sizeof(buf))
extern struct task_struct aeros_task;
#define current (&aeros_task)

static inline int cond_resched(void)
{
	aeros_wait_step();
	return 0;
}
static inline void schedule(void) { aeros_wait_step(); }
static inline void yield(void) { aeros_wait_step(); }
static inline bool signal_pending(struct task_struct *task) { (void)task; return false; }
static inline bool fatal_signal_pending(struct task_struct *task) { (void)task; return false; }
static inline bool need_resched(void) { return false; }
static inline pid_t task_pid_nr(struct task_struct *task) { return task->pid; }
long schedule_timeout(long timeout);
static inline void sched_set_fifo(struct task_struct *task) { (void)task; }
static inline void sched_set_fifo_low(struct task_struct *task) { (void)task; }
static inline void sched_set_normal(struct task_struct *task, int nice) { (void)task; (void)nice; }
static inline int wake_up_state(struct task_struct *task, unsigned int state) { (void)task; (void)state; return 1; }
static inline int wake_up_process(struct task_struct *task) { (void)task; return 1; }
#define schedule_timeout_interruptible schedule_timeout
#define schedule_timeout_uninterruptible schedule_timeout
#define set_current_state(state) do { } while (0)
#define __set_current_state(state) do { } while (0)
#define might_sleep() do { } while (0)

#endif
