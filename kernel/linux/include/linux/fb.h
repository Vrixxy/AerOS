#ifndef _LINUX_FB_H
#define _LINUX_FB_H

#include <linux/types.h>
struct fb_info;
struct linux_fb_info;
struct fb_var_screeninfo;
struct fb_fix_screeninfo;

#define KHZ2PICOS(a) (1000000000UL / (a))
#define PICOS2KHZ(a) (1000000000UL / (a))

#endif
