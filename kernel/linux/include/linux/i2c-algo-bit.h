#ifndef _AEROS_LINUX_I2C_ALGO_BIT_H
#define _AEROS_LINUX_I2C_ALGO_BIT_H

#include <linux/i2c.h>
struct i2c_algo_bit_data {
	void *data;
	void (*setsda)(void *data, int state);
	void (*setscl)(void *data, int state);
	int (*getsda)(void *data);
	int (*getscl)(void *data);
	int (*pre_xfer)(struct i2c_adapter *);
	void (*post_xfer)(struct i2c_adapter *);
	int udelay;
	int timeout;
	bool can_do_atomic;
};
/* Bit-banged DDC is only used by pre-DCN display blocks: not provided. */
static inline int i2c_bit_add_bus(struct i2c_adapter *adapter) { (void)adapter; return 0; }
#endif
