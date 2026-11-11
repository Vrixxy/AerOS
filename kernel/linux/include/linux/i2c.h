#ifndef _LINUX_I2C_H
#define _LINUX_I2C_H

#include <linux/device.h>
#include <linux/mutex.h>

#define I2C_M_RD 0x0001
#define I2C_M_TEN 0x0010
#define I2C_M_RECV_LEN 0x0400
#define I2C_M_NOSTART 0x4000
#define I2C_M_STOP 0x8000
#define I2C_FUNC_I2C 0x00000001
#define I2C_FUNC_10BIT_ADDR 0x00000002
#define I2C_FUNC_SMBUS_EMUL 0x0eff0008
#define I2C_FUNC_SMBUS_READ_BLOCK_DATA 0x01000000
#define I2C_CLASS_DDC (1 << 3)
#define I2C_NAME_SIZE 20

struct i2c_msg {
	u16 addr;
	u16 flags;
	u16 len;
	u8 *buf;
};

struct i2c_adapter;
struct i2c_algorithm {
	int (*master_xfer)(struct i2c_adapter *adapter, struct i2c_msg *msgs, int num);
	int (*master_xfer_atomic)(struct i2c_adapter *adapter, struct i2c_msg *msgs, int num);
	u32 (*functionality)(struct i2c_adapter *adapter);
};

struct i2c_lock_operations {
	void (*lock_bus)(struct i2c_adapter *adapter, unsigned int flags);
	int (*trylock_bus)(struct i2c_adapter *adapter, unsigned int flags);
	void (*unlock_bus)(struct i2c_adapter *adapter, unsigned int flags);
};

struct i2c_adapter_quirks {
	u64 flags;
	int max_num_msgs;
	u16 max_write_len, max_read_len, max_comb_1st_msg_len, max_comb_2nd_msg_len;
};

struct i2c_adapter {
	struct module *owner;
	unsigned int class;
	const struct i2c_algorithm *algo;
	void *algo_data;
	const struct i2c_lock_operations *lock_ops;
	const struct i2c_adapter_quirks *quirks;
	int timeout;
	int retries;
	struct device dev;
	int nr;
	char name[48];
};

struct i2c_client {
	unsigned short flags;
	unsigned short addr;
	char name[I2C_NAME_SIZE];
	struct i2c_adapter *adapter;
	struct device dev;
};

#define I2C_LOCK_ROOT_ADAPTER 0x01
#define I2C_LOCK_SEGMENT 0x02

static inline void *i2c_get_adapdata(const struct i2c_adapter *adapter) { return dev_get_drvdata(&adapter->dev); }
static inline void i2c_set_adapdata(struct i2c_adapter *adapter, void *data) { dev_set_drvdata(&adapter->dev, data); }
static inline int i2c_transfer(struct i2c_adapter *adapter, struct i2c_msg *msgs, int num)
{
	int ret = -EOPNOTSUPP, tries;

	if (!adapter->algo || !adapter->algo->master_xfer)
		return ret;
	for (tries = 0; tries <= adapter->retries; tries++) {
		ret = adapter->algo->master_xfer(adapter, msgs, num);
		if (ret != -EAGAIN)
			break;
	}
	return ret;
}
#define __i2c_transfer i2c_transfer
static inline int i2c_add_adapter(struct i2c_adapter *adapter) { (void)adapter; return 0; }
#define i2c_add_numbered_adapter i2c_add_adapter
static inline void i2c_del_adapter(struct i2c_adapter *adapter) { (void)adapter; }
static inline void i2c_lock_bus(struct i2c_adapter *adapter, unsigned int flags) { (void)adapter; (void)flags; }
static inline void i2c_unlock_bus(struct i2c_adapter *adapter, unsigned int flags) { (void)adapter; (void)flags; }
static inline u8 i2c_8bit_addr_from_msg(const struct i2c_msg *msg) { return (u8)((msg->addr << 1) | (msg->flags & I2C_M_RD)); }

#define I2C_CLASS_HWMON (1 << 0)
#define I2C_CLASS_SPD (1 << 7)
#define I2C_AQ_NO_ZERO_LEN_READ (1 << 5)
#define I2C_AQ_NO_ZERO_LEN_WRITE (1 << 6)
#define I2C_AQ_NO_ZERO_LEN (I2C_AQ_NO_ZERO_LEN_READ | I2C_AQ_NO_ZERO_LEN_WRITE)
#define I2C_AQ_COMB_WRITE_THEN_READ 0x00000004
#define I2C_AQ_NO_CLK_STRETCH (1 << 4)

#define I2C_AQ_COMB (1 << 0)
#define I2C_AQ_COMB_WRITE_FIRST (1 << 1)
#define I2C_AQ_COMB_READ_SECOND (1 << 2)
#define I2C_AQ_COMB_SAME_ADDR (1 << 3)

#define I2C_FUNC_SMBUS_QUICK 0x00010000
#define I2C_FUNC_SMBUS_READ_BYTE 0x00020000
#define I2C_FUNC_SMBUS_WRITE_BYTE 0x00040000
#define I2C_FUNC_SMBUS_READ_BYTE_DATA 0x00080000
#define I2C_FUNC_SMBUS_WRITE_BYTE_DATA 0x00100000
#define I2C_FUNC_SMBUS_READ_WORD_DATA 0x00200000
#define I2C_FUNC_SMBUS_WRITE_WORD_DATA 0x00400000
#define I2C_FUNC_SMBUS_PROC_CALL 0x00800000
#define I2C_FUNC_SMBUS_WRITE_BLOCK_DATA 0x02000000
#define I2C_FUNC_SMBUS_READ_I2C_BLOCK 0x04000000
#define I2C_FUNC_SMBUS_WRITE_I2C_BLOCK 0x08000000
#define I2C_FUNC_SMBUS_BLOCK_PROC_CALL 0x00008000
#define I2C_FUNC_PROTOCOL_MANGLING 0x00000004
#define I2C_FUNC_SMBUS_PEC 0x00000008
#define I2C_FUNC_NOSTART 0x00000010

#endif
