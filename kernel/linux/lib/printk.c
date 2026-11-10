#include <linux/kernel.h>
#include <linux/device.h>
#include <linux/string.h>
#include <aeros/ops.h>

struct sink {
	char *buffer;
	size_t size;
	size_t length;
};

static void put(struct sink *sink, char c)
{
	if (sink->length + 1 < sink->size)
		sink->buffer[sink->length] = c;
	sink->length++;
}

static void put_number(struct sink *sink, unsigned long long value, unsigned base, bool upper,
		       bool negative, int width, char pad, bool left)
{
	const char *digits = upper ? "0123456789ABCDEF" : "0123456789abcdef";
	char scratch[24];
	int count = 0;

	do {
		scratch[count++] = digits[value % base];
		value /= base;
	} while (value);
	if (negative)
		scratch[count++] = '-';
	if (negative && pad == '0') {
		put(sink, '-');
		count--;
		width--;
	}
	if (!left)
		for (int i = count; i < width; i++)
			put(sink, pad);
	while (count)
		put(sink, scratch[--count]);
	if (left)
		for (int i = count; i < width; i++)
			put(sink, ' ');
}

int vsnprintf(char *buffer, size_t size, const char *format, va_list args)
{
	struct sink sink = { buffer, size, 0 };

	for (; *format; format++) {
		if (*format != '%') {
			put(&sink, *format);
			continue;
		}
		format++;
		bool left = false, alternate = false;
		char pad = ' ';
		for (;; format++) {
			if (*format == '-')
				left = true;
			else if (*format == '0')
				pad = '0';
			else if (*format == '#')
				alternate = true;
			else if (*format != '+' && *format != ' ')
				break;
		}
		int width = 0;
		if (*format == '*') {
			width = va_arg(args, int);
			format++;
		}
		while (*format >= '0' && *format <= '9')
			width = width * 10 + (*format++ - '0');
		int precision = -1;
		if (*format == '.') {
			format++;
			precision = 0;
			if (*format == '*') {
				precision = va_arg(args, int);
				format++;
			}
			while (*format >= '0' && *format <= '9')
				precision = precision * 10 + (*format++ - '0');
		}
		int longs = 0;
		while (*format == 'l' || *format == 'z' || *format == 'h' || *format == 'j' ||
		       *format == 't') {
			if (*format == 'l' || *format == 'z' || *format == 'j' || *format == 't')
				longs++;
			format++;
		}
		unsigned long long value;
		switch (*format) {
		case 'd':
		case 'i': {
			long long signed_value = longs ? va_arg(args, long long) : va_arg(args, int);
			bool negative = signed_value < 0;
			put_number(&sink, negative ? -(unsigned long long)signed_value : signed_value, 10,
				   false, negative, width, pad, left);
			break;
		}
		case 'u':
		case 'x':
		case 'X':
		case 'o':
			value = longs ? va_arg(args, unsigned long long) : va_arg(args, unsigned int);
			if (alternate && *format != 'u') {
				put(&sink, '0');
				width--;
				if (*format != 'o') {
					put(&sink, *format);
					width--;
				}
			}
			put_number(&sink, value, *format == 'u' ? 10 : *format == 'o' ? 8 : 16,
				   *format == 'X', false, width, pad, left);
			break;
		case 'p':
			value = (unsigned long)va_arg(args, void *);
			if (format[1] == 'V') {
				const struct va_format *nested = (const struct va_format *)value;
				size_t at = sink.length < sink.size ? sink.length : sink.size;
				va_list copy;

				format++;
				va_copy(copy, *nested->va);
				sink.length += (size_t)vsnprintf(buffer + at, sink.size - at, nested->fmt, copy);
				va_end(copy);
				break;
			}
			/* Other %p extensions (%pR, %pe, ...) print as a plain pointer. */
			while ((format[1] >= 'a' && format[1] <= 'z') || (format[1] >= 'A' && format[1] <= 'Z'))
				format++;
			put(&sink, '0');
			put(&sink, 'x');
			put_number(&sink, value, 16, false, false, 0, ' ', false);
			break;
		case 'c':
			put(&sink, (char)va_arg(args, int));
			break;
		case 's': {
			const char *text = va_arg(args, const char *);
			if (!text)
				text = "(null)";
			size_t length = strnlen(text, precision < 0 ? (size_t)-1 : (size_t)precision);
			if (!left)
				for (int i = (int)length; i < width; i++)
					put(&sink, ' ');
			for (size_t i = 0; i < length; i++)
				put(&sink, text[i]);
			if (left)
				for (int i = (int)length; i < width; i++)
					put(&sink, ' ');
			break;
		}
		case '%':
			put(&sink, '%');
			break;
		case '\0':
			format--;
			break;
		default:
			put(&sink, '%');
			put(&sink, *format);
		}
	}
	if (size)
		buffer[sink.length < size ? sink.length : size - 1] = '\0';
	return (int)sink.length;
}

int snprintf(char *buffer, size_t size, const char *format, ...)
{
	va_list args;
	va_start(args, format);
	int length = vsnprintf(buffer, size, format, args);
	va_end(args);
	return length;
}

int scnprintf(char *buffer, size_t size, const char *format, ...)
{
	va_list args;
	va_start(args, format);
	int length = vsnprintf(buffer, size, format, args);
	va_end(args);
	if (!size)
		return 0;
	return length < (int)size ? length : (int)size - 1;
}

int vprintk(const char *format, va_list args)
{
	char line[512];

	if (format[0] == KERN_SOH[0] && format[1])
		format += 2;
	int length = vsnprintf(line, sizeof(line), format, args);
	if (length >= (int)sizeof(line))
		length = sizeof(line) - 1;
	aeros->log(line, (unsigned long)length);
	return length;
}

int printk(const char *format, ...)
{
	va_list args;
	va_start(args, format);
	int length = vprintk(format, args);
	va_end(args);
	return length;
}

int dev_printk(const char *level, const struct device *dev, const char *format, ...)
{
	char line[480];
	va_list args;

	(void)level;
	va_start(args, format);
	vsnprintf(line, sizeof(line), format, args);
	va_end(args);
	return printk("%s: %s", dev ? dev->name : "?", line);
}

void aeros_bug(const char *file, int line)
{
	printk("BUG at %s:%d\n", file, line);
	for (;;)
		__asm__ volatile("cli; hlt");
}

char *kvasprintf(gfp_t gfp, const char *format, va_list args)
{
	va_list copy;
	int length;
	char *text;

	va_copy(copy, args);
	length = vsnprintf(NULL, 0, format, copy);
	va_end(copy);
	text = kmalloc((size_t)length + 1, gfp);
	if (text)
		vsnprintf(text, (size_t)length + 1, format, args);
	return text;
}

char *kasprintf(gfp_t gfp, const char *format, ...)
{
	va_list args;
	char *text;

	va_start(args, format);
	text = kvasprintf(gfp, format, args);
	va_end(args);
	return text;
}
