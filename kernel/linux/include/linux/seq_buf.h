#ifndef _LINUX_SEQ_BUF_H
#define _LINUX_SEQ_BUF_H

#include <linux/kernel.h>
#include <linux/stdarg.h>

struct seq_buf {
	char *buffer;
	size_t size;
	size_t len;
};
#define DECLARE_SEQ_BUF(name, length) \
	char name##_buffer[length] = { 0 }; \
	struct seq_buf name = { .buffer = name##_buffer, .size = (length), .len = 0 }
static inline void seq_buf_init(struct seq_buf *s, char *buffer, size_t size)
{
	s->buffer = buffer;
	s->size = size;
	s->len = 0;
	if (size)
		buffer[0] = 0;
}
static inline bool seq_buf_has_overflowed(struct seq_buf *s) { return s->len > s->size; }
static inline const char *seq_buf_str(struct seq_buf *s)
{
	if (s->size)
		s->buffer[s->len < s->size ? s->len : s->size - 1] = 0;
	return s->buffer;
}
int seq_buf_printf(struct seq_buf *s, const char *fmt, ...) __attribute__((format(printf, 2, 3)));
int seq_buf_puts(struct seq_buf *s, const char *str);

#endif
