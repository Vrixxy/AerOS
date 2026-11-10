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
