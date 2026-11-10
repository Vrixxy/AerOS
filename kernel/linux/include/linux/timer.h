#ifndef _LINUX_TIMER_H
#define _LINUX_TIMER_H

#include <linux/types.h>
#include <linux/kernel.h>
#include <linux/jiffies.h>

struct timer_list {
	struct timer_list *next;
	unsigned long expires;
	void (*function)(struct timer_list *timer);
	bool pending;
};

void timer_setup(struct timer_list *timer, void (*function)(struct timer_list *), unsigned int flags);
int mod_timer(struct timer_list *timer, unsigned long expires);
void add_timer(struct timer_list *timer);
int del_timer(struct timer_list *timer);
#define del_timer_sync del_timer
#define timer_delete del_timer
#define timer_delete_sync del_timer
static inline int timer_pending(const struct timer_list *timer) { return timer->pending; }

#define from_timer(var, timer, field) container_of(timer, __typeof__(*var), field)

#endif
