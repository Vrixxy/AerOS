#ifndef _AEROS_LINUX_CONSOLE_H
#define _AEROS_LINUX_CONSOLE_H

static inline void console_lock(void) { }
static inline void console_unlock(void) { }
static inline int console_trylock(void) { return 1; }
#endif
