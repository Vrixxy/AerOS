#ifndef _LINUX_EXPORT_H
#define _LINUX_EXPORT_H

/* Everything is linked into one module; a declaration keeps the trailing
 * semicolon legal at file scope. */
#define EXPORT_SYMBOL(sym) extern __typeof__(sym) sym
#define EXPORT_SYMBOL_GPL(sym) EXPORT_SYMBOL(sym)
#define EXPORT_SYMBOL_NS(sym, ns) EXPORT_SYMBOL(sym)
#define EXPORT_SYMBOL_NS_GPL(sym, ns) EXPORT_SYMBOL(sym)

#endif
