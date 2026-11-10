#ifndef _LINUX_POLL_H
#define _LINUX_POLL_H

#include <linux/wait.h>
#include <linux/fs.h>
typedef struct poll_table_struct {
	int unused;
} poll_table;
#define EPOLLIN 0x00000001u
#define EPOLLPRI 0x00000002u
#define EPOLLOUT 0x00000004u
#define EPOLLERR 0x00000008u
#define EPOLLHUP 0x00000010u
#define EPOLLRDNORM 0x00000040u
#define EPOLLWRNORM 0x00000100u
#define poll_wait(file, queue, table) do { (void)(file); (void)(queue); (void)(table); } while (0)
#define wake_up_interruptible_poll(queue, mask) wake_up(queue)

#endif
