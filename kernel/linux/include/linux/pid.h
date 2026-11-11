#ifndef _AEROS_LINUX_PID_H
#define _AEROS_LINUX_PID_H

#include <linux/sched.h>
enum pid_type { PIDTYPE_PID, PIDTYPE_TGID, PIDTYPE_PGID, PIDTYPE_SID, PIDTYPE_MAX };
static inline struct task_struct *pid_task(struct pid *pid, enum pid_type type) { (void)pid; (void)type; return NULL; }
static inline struct pid *find_get_pid(int nr) { (void)nr; return NULL; }
#endif
