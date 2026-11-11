#include <linux/ctype.h>
#include <linux/errno.h>
#include <linux/limits.h>
#include <linux/random.h>
#include <linux/string.h>
#include <linux/slab.h>

void *memcpy(void *destination, const void *source, size_t length)
{
	void *d = destination;
	const void *s = source;

	__asm__ volatile("rep movsb" : "+D"(d), "+S"(s), "+c"(length) : : "memory");
	return destination;
}

void *memmove(void *destination, const void *source, size_t length)
{
	unsigned char *d = destination;
	const unsigned char *s = source;

	if (d < s || d >= s + length)
		return memcpy(destination, source, length);
	while (length--)
		d[length] = s[length];
	return destination;
}

void *memset(void *destination, int value, size_t length)
{
	void *d = destination;

	__asm__ volatile("rep stosb" : "+D"(d), "+c"(length) : "a"(value) : "memory");
	return destination;
}

int memcmp(const void *a, const void *b, size_t length)
{
	const unsigned char *x = a, *y = b;

	for (; length; length--, x++, y++)
		if (*x != *y)
			return *x - *y;
	return 0;
}

size_t strlen(const char *text)
{
	size_t length = 0;

	while (text[length])
		length++;
	return length;
}

size_t strnlen(const char *text, size_t limit)
{
	size_t length = 0;

	while (length < limit && text[length])
		length++;
	return length;
}

int strcmp(const char *a, const char *b)
{
	while (*a && *a == *b)
		a++, b++;
	return (unsigned char)*a - (unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t limit)
{
	for (; limit; limit--, a++, b++) {
		if (*a != *b)
			return (unsigned char)*a - (unsigned char)*b;
		if (!*a)
			break;
	}
	return 0;
}

char *strchr(const char *text, int c)
{
	for (;; text++) {
		if (*text == (char)c)
			return (char *)text;
		if (!*text)
			return 0;
	}
}

size_t strscpy(char *destination, const char *source, size_t size)
{
	size_t length = strnlen(source, size);

	if (!size)
		return (size_t)-7;
	if (length == size)
		length = size - 1;
	memcpy(destination, source, length);
	destination[length] = '\0';
	return length;
}

char *kstrdup(const char *text, gfp_t flags)
{
	size_t length = strlen(text) + 1;
	char *copy = kmalloc(length, flags);

	if (copy)
		memcpy(copy, text, length);
	return copy;
}

void *kmemdup(const void *source, size_t length, gfp_t gfp)
{
	void *copy = kmalloc(length, gfp);

	if (copy)
		memcpy(copy, source, length);
	return copy;
}

char *kstrndup(const char *text, size_t limit, gfp_t gfp)
{
	size_t length = text ? strnlen(text, limit) : 0;
	char *copy;

	if (!text)
		return NULL;
	copy = kmalloc(length + 1, gfp);
	if (copy) {
		memcpy(copy, text, length);
		copy[length] = 0;
	}
	return copy;
}

char *strcpy(char *destination, const char *source)
{
	return memcpy(destination, source, strlen(source) + 1);
}

ssize_t strscpy_pad(char *destination, const char *source, size_t size)
{
	size_t length = strscpy(destination, source, size);

	if (size && length < size)
		memset(destination + length, 0, size - length);
	return (ssize_t)length;
}

unsigned long long simple_strtoull(const char *string, char **end, unsigned int base)
{
	unsigned long long value = 0;

	if (!base) {
		base = 10;
		if (string[0] == '0') {
			base = 8;
			if ((string[1] | 0x20) == 'x' && isxdigit(string[2])) {
				base = 16;
				string += 2;
			}
		}
	} else if (base == 16 && string[0] == '0' && (string[1] | 0x20) == 'x') {
		string += 2;
	}
	for (;; string++) {
		unsigned int digit;

		if (isdigit(*string))
			digit = (unsigned int)(*string - '0');
		else if (isalpha(*string))
			digit = (unsigned int)((*string | 0x20) - 'a' + 10);
		else
			break;
		if (digit >= base)
			break;
		value = value * base + digit;
	}
	if (end)
		*end = (char *)string;
	return value;
}

unsigned long simple_strtoul(const char *string, char **end, unsigned int base)
{
	return (unsigned long)simple_strtoull(string, end, base);
}

long simple_strtol(const char *string, char **end, unsigned int base)
{
	if (*string == '-')
		return -(long)simple_strtoull(string + 1, end, base);
	return (long)simple_strtoull(string, end, base);
}

static int parse_whole(const char *string, unsigned int base, unsigned long long *result, bool negative_ok, bool *negative)
{
	char *end;

	*negative = false;
	if (*string == '+') {
		string++;
	} else if (*string == '-' && negative_ok) {
		*negative = true;
		string++;
	}
	if (!*string)
		return -EINVAL;
	*result = simple_strtoull(string, &end, base);
	if (end == string)
		return -EINVAL;
	if (*end == '\n')
		end++;
	return *end ? -EINVAL : 0;
}

int kstrtoint(const char *string, unsigned int base, int *result)
{
	unsigned long long value;
	bool negative;
	int ret = parse_whole(string, base, &value, true, &negative);

	if (ret)
		return ret;
	if (value > (negative ? 0x80000000ull : 0x7fffffffull))
		return -ERANGE;
	*result = negative ? -(int)(value - 1) - 1 : (int)value;
	return 0;
}

int kstrtouint(const char *string, unsigned int base, unsigned int *result)
{
	unsigned long long value;
	bool negative;
	int ret = parse_whole(string, base, &value, false, &negative);

	if (ret)
		return ret;
	if (value > 0xffffffffull)
		return -ERANGE;
	*result = (unsigned int)value;
	return 0;
}

int kstrtoul(const char *string, unsigned int base, unsigned long *result)
{
	unsigned long long value;
	bool negative;
	int ret = parse_whole(string, base, &value, false, &negative);

	if (!ret)
		*result = (unsigned long)value;
	return ret;
}

int kstrtobool(const char *string, bool *result)
{
	if (!string)
		return -EINVAL;
	switch (string[0]) {
	case 'y': case 'Y': case '1':
		*result = true;
		return 0;
	case 'n': case 'N': case '0':
		*result = false;
		return 0;
	case 'o': case 'O':
		if ((string[1] | 0x20) == 'n') {
			*result = true;
			return 0;
		}
		if ((string[1] | 0x20) == 'f') {
			*result = false;
			return 0;
		}
	}
	return -EINVAL;
}

char *strnstr(const char *haystack, const char *needle, size_t length)
{
	size_t n = strlen(needle);

	if (!n)
		return (char *)haystack;
	while (length >= n) {
		if (!memcmp(haystack, needle, n))
			return (char *)haystack;
		if (!*haystack)
			break;
		length--;
		haystack++;
	}
	return NULL;
}

char *strsep(char **stringp, const char *delimiters)
{
	char *start = *stringp, *end;

	if (!start)
		return NULL;
	end = start + strcspn(start, delimiters);
	if (*end)
		*end++ = 0;
	else
		end = NULL;
	*stringp = end;
	return start;
}

static int kstrto_signed(const char *string, unsigned int base, long long *result, long long min, long long max)
{
	unsigned long long value;
	bool negative;
	int ret = parse_whole(string, base, &value, true, &negative);

	if (ret)
		return ret;
	if (negative ? value > (unsigned long long)max + 1 : value > (unsigned long long)max)
		return -ERANGE;
	*result = negative ? -(long long)(value - 1) - 1 : (long long)value;
	if (*result < min)
		return -ERANGE;
	return 0;
}

static int kstrto_unsigned(const char *string, unsigned int base, unsigned long long *result,
			   unsigned long long max)
{
	bool negative;
	int ret = parse_whole(string, base, result, false, &negative);

	if (!ret && *result > max)
		return -ERANGE;
	return ret;
}

int kstrtol(const char *string, unsigned int base, long *result)
{
	long long value;
	int ret = kstrto_signed(string, base, &value, LONG_MIN, LONG_MAX);

	if (!ret)
		*result = (long)value;
	return ret;
}

int kstrtoll(const char *string, unsigned int base, long long *result)
{
	return kstrto_signed(string, base, result, LLONG_MIN, LLONG_MAX);
}

int kstrtoull(const char *string, unsigned int base, unsigned long long *result)
{
	return kstrto_unsigned(string, base, result, ULLONG_MAX);
}

int kstrtou32(const char *string, unsigned int base, u32 *result)
{
	unsigned long long value;
	int ret = kstrto_unsigned(string, base, &value, 0xffffffffull);

	if (!ret)
		*result = (u32)value;
	return ret;
}

int kstrtou16(const char *string, unsigned int base, u16 *result)
{
	unsigned long long value;
	int ret = kstrto_unsigned(string, base, &value, 0xffffull);

	if (!ret)
		*result = (u16)value;
	return ret;
}

int kstrtou8(const char *string, unsigned int base, u8 *result)
{
	unsigned long long value;
	int ret = kstrto_unsigned(string, base, &value, 0xffull);

	if (!ret)
		*result = (u8)value;
	return ret;
}

/* Integers (d i u x X o, with hh h l ll z modifiers and widths), strings, characters and %n. */
int vsscanf(const char *string, const char *format, va_list args)
{
	const char *s = string;
	int assigned = 0;

	while (*format) {
		int width = 0, size = 0;
		bool skip = false;

		if (isspace(*format)) {
			while (isspace(*format))
				format++;
			while (isspace(*s))
				s++;
			continue;
		}
		if (*format != '%' || format[1] == '%') {
			if (*format == '%')
				format++;
			if (*s != *format)
				break;
			s++;
			format++;
			continue;
		}
		format++;
		if (*format == '*') {
			skip = true;
			format++;
		}
		while (isdigit(*format))
			width = width * 10 + (*format++ - '0');
		while (*format == 'h' || *format == 'l' || *format == 'z' || *format == 'L') {
			if (*format == 'h')
				size = size > -2 ? size - 1 : -2;
			else
				size = size < 2 ? size + 1 : 2;
			format++;
		}
		if (*format == 'n') {
			if (!skip)
				*va_arg(args, int *) = (int)(s - string);
			format++;
			continue;
		}
		if (*format == 'c') {
			int count = width ? width : 1;
			char *out = skip ? NULL : va_arg(args, char *);

			while (count--) {
				if (!*s)
					return assigned;
				if (out)
					*out++ = *s;
				s++;
			}
			if (!skip)
				assigned++;
			format++;
			continue;
		}
		while (isspace(*s))
			s++;
		if (!*s)
			break;
		if (*format == 's') {
			char *out = skip ? NULL : va_arg(args, char *);

			while (*s && !isspace(*s) && (!width || width--)) {
				if (out)
					*out++ = *s;
				s++;
			}
			if (out) {
				*out = 0;
				assigned++;
			}
			format++;
			continue;
		}
		{
			char buffer[32];
			unsigned int base = 10;
			bool is_signed = false;
			int n = 0, limit = width && width < 31 ? width : 31;
			unsigned long long value;
			char *end;

			switch (*format) {
			case 'd': is_signed = true; break;
			case 'i': is_signed = true; base = 0; break;
			case 'u': break;
			case 'x': case 'X': base = 16; break;
			case 'o': base = 8; break;
			default: return assigned;
			}
			if ((*s == '-' || *s == '+') && n < limit)
				buffer[n++] = *s++;
			while (*s && (isxdigit(*s) || (*s | 0x20) == 'x') && n < limit)
				buffer[n++] = *s++;
			buffer[n] = 0;
			if (buffer[0] == '-')
				value = (unsigned long long)-(long long)simple_strtoull(buffer + 1, &end, base);
			else
				value = simple_strtoull(buffer[0] == '+' ? buffer + 1 : buffer, &end, base);
			if (end == buffer || (end == buffer + 1 && (buffer[0] == '-' || buffer[0] == '+')))
				return assigned;
			s -= (buffer + n) - end;
			format++;
			if (skip)
				continue;
			if (size <= -2)
				*va_arg(args, u8 *) = (u8)value;
			else if (size == -1)
				*va_arg(args, u16 *) = (u16)value;
			else if (size == 0)
				*va_arg(args, u32 *) = (u32)value;
			else
				*va_arg(args, u64 *) = value;
			(void)is_signed;
			assigned++;
		}
	}
	return assigned;
}

int sscanf(const char *string, const char *format, ...)
{
	va_list args;
	int n;

	va_start(args, format);
	n = vsscanf(string, format, args);
	va_end(args);
	return n;
}

size_t strspn(const char *s, const char *accept)
{
	size_t n = 0;

	while (s[n] && strchr(accept, s[n]))
		n++;
	return n;
}

size_t strcspn(const char *s, const char *reject)
{
	size_t n = 0;

	while (s[n] && !strchr(reject, s[n]))
		n++;
	return n;
}

char *strstr(const char *haystack, const char *needle)
{
	return strnstr(haystack, needle, strlen(haystack));
}

int strncasecmp(const char *a, const char *b, size_t n)
{
	while (n--) {
		int ca = tolower(*a++), cb = tolower(*b++);

		if (ca != cb || !ca)
			return ca - cb;
	}
	return 0;
}

int strcasecmp(const char *a, const char *b)
{
	return strncasecmp(a, b, (size_t)-1);
}

char *skip_spaces(const char *s)
{
	while (isspace(*s))
		s++;
	return (char *)s;
}

char *strim(char *s)
{
	size_t size = strlen(s);
	char *end;

	if (!size)
		return s;
	end = s + size - 1;
	while (end >= s && isspace(*end))
		end--;
	*(end + 1) = 0;
	return skip_spaces(s);
}

static u64 random_state = 0x9e3779b97f4a7c15ull;

u64 get_random_u64(void)
{
	u64 value;
	u32 eax = 1, ebx, ecx = 0, edx;
	unsigned char ok = 0;
	u32 low, high;

	__asm__ volatile("cpuid" : "+a"(eax), "=b"(ebx), "+c"(ecx), "=d"(edx));
	if (ecx & (1u << 30)) {
		for (int tries = 0; tries < 10 && !ok; tries++)
			__asm__ volatile("rdrand %0; setc %1" : "=r"(value), "=qm"(ok));
		if (ok)
			return value;
	}
	__asm__ volatile("rdtsc" : "=a"(low), "=d"(high));
	random_state ^= ((u64)high << 32 | low) + 0x9e3779b97f4a7c15ull;
	random_state ^= random_state << 13;
	random_state ^= random_state >> 7;
	random_state ^= random_state << 17;
	return random_state;
}

void get_random_bytes(void *buffer, size_t length)
{
	u8 *bytes = buffer;

	while (length) {
		u64 value = get_random_u64();
		size_t n = length < sizeof(value) ? length : sizeof(value);

		memcpy(bytes, &value, n);
		bytes += n;
		length -= n;
	}
}

size_t strlcpy(char *dest, const char *src, size_t size)
{
	size_t length = strlen(src);

	if (size) {
		size_t n = length >= size ? size - 1 : length;

		memcpy(dest, src, n);
		dest[n] = 0;
	}
	return length;
}

size_t strlcat(char *dest, const char *src, size_t count)
{
	size_t dsize = strnlen(dest, count);

	if (dsize == count)
		return count + strlen(src);
	return dsize + strlcpy(dest + dsize, src, count - dsize);
}

void guid_gen(guid_t *guid)
{
	get_random_bytes(guid, sizeof(*guid));
	guid->b[7] = (guid->b[7] & 0x0f) | 0x40;
	guid->b[8] = (guid->b[8] & 0x3f) | 0x80;
}
