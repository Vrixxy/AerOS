#ifndef _LINUX_STRINGIFY_H
#define _LINUX_STRINGIFY_H
#define __stringify_1(x...) #x
#define __stringify(x...) __stringify_1(x)
#endif
