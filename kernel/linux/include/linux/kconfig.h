#ifndef _LINUX_KCONFIG_H
#define _LINUX_KCONFIG_H

/* IS_ENABLED(CONFIG_X) is 1 when CONFIG_X is defined as 1, else 0; AerOS
 * builds nothing as a loadable module. */
#define __aeros_placeholder_1 0,
#define __aeros_second(ignored, value, ...) value
#define __aeros_defined(x) __aeros_defined2(__aeros_placeholder_##x)
#define __aeros_defined2(arg) __aeros_second(arg 1, 0)
#define IS_BUILTIN(option) __aeros_defined2(__aeros_placeholder_##option)
#define IS_ENABLED(option) IS_BUILTIN(option)
#define IS_MODULE(option) 0
#define IS_REACHABLE(option) IS_BUILTIN(option)

#endif
