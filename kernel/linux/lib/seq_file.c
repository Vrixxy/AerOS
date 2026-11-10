// SPDX-License-Identifier: MIT
#include <linux/kernel.h>
#include <linux/seq_file.h>

void seq_write(struct seq_file *file, const void *data, size_t length)
{
	size_t room;

	if (!file->buf) {
		printk("%.*s", (int)length, (const char *)data);
		return;
	}
	room = file->size - file->count;
	if (length > room)
		length = room;
	memcpy(file->buf + file->count, data, length);
	file->count += length;
}

int seq_vprintf(struct seq_file *file, const char *format, va_list args)
{
	char line[256];
	int length = vsnprintf(line, sizeof(line), format, args);

	if (length > (int)sizeof(line) - 1)
		length = sizeof(line) - 1;
	seq_write(file, line, (size_t)length);
	return 0;
}

int seq_printf(struct seq_file *file, const char *format, ...)
{
	va_list args;

	va_start(args, format);
	seq_vprintf(file, format, args);
	va_end(args);
	return 0;
}

void seq_puts(struct seq_file *file, const char *text)
{
	seq_write(file, text, strlen(text));
}

void seq_putc(struct seq_file *file, char c)
{
	seq_write(file, &c, 1);
}
