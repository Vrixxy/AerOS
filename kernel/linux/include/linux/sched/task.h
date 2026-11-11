#ifndef _AEROS_LINUX_SCHED_TASK_H
#define _AEROS_LINUX_SCHED_TASK_H

#include <linux/sched.h>
static inline struct task_struct *get_task_struct(struct task_struct *t) { return t; }
static inline void put_task_struct(struct task_struct *t) { (void)t; }
#endif
