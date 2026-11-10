#ifndef _LINUX_SEQ_FILE_H
#define _LINUX_SEQ_FILE_H

#include <linux/types.h>

/* With no buffer, output goes to the Linux log (LINUX.LOG). */
struct seq_file {
	char *buf;
	size_t size;
	size_t count;
	void *private;
};

int seq_printf(struct seq_file *file, const char *format, ...) __attribute__((format(printf, 2, 3)));
int seq_vprintf(struct seq_file *file, const char *format, va_list args);
void seq_puts(struct seq_file *file, const char *text);
void seq_putc(struct seq_file *file, char c);
void seq_write(struct seq_file *file, const void *data, size_t length);
static inline bool seq_has_overflowed(struct seq_file *file) { return file->buf && file->count == file->size; }

#endif
