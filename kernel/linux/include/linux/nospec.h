#ifndef _LINUX_NOSPEC_H
#define _LINUX_NOSPEC_H

#define array_index_nospec(index, size) ((index) < (size) ? (index) : 0)

#endif
