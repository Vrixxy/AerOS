#ifndef _AEROS_LINUX_ACPI_H
#define _AEROS_LINUX_ACPI_H

#include <linux/types.h>
#include <linux/device.h>
/* Types only: AerOS has no ACPI interpreter on the Linux side, and drivers
 * are built without CONFIG_ACPI. */
typedef void *acpi_handle;
typedef u32 acpi_status;
typedef u64 acpi_size;
typedef u64 acpi_integer;
#define AE_OK 0
#define AE_NOT_FOUND 5
#define ACPI_SUCCESS(a) (!(a))
#define ACPI_FAILURE(a) (a)
#define ACPI_HANDLE(dev) ((acpi_handle)NULL)
#define ACPI_ALLOCATE_BUFFER ((acpi_size)-1)
#define ACPI_TYPE_INTEGER 1
#define ACPI_TYPE_STRING 2
#define ACPI_TYPE_BUFFER 3
#define ACPI_TYPE_PACKAGE 4
struct acpi_buffer {
	acpi_size length;
	void *pointer;
};
union acpi_object {
	u32 type;
	struct {
		u32 type;
		u64 value;
	} integer;
	struct {
		u32 type;
		u32 length;
		u8 *pointer;
	} buffer;
	struct {
		u32 type;
		u32 count;
		union acpi_object *elements;
	} package;
};
struct acpi_object_list {
	u32 count;
	union acpi_object *pointer;
};
struct acpi_table_header {
	char signature[4];
	u32 length;
	u8 revision;
	u8 checksum;
	char oem_id[6];
	char oem_table_id[8];
	u32 oem_revision;
	char asl_compiler_id[4];
	u32 asl_compiler_revision;
};
struct acpi_device;
static inline const char *acpi_format_exception(acpi_status status) { (void)status; return "no ACPI"; }
static inline bool acpi_disabled_check(void) { return true; }
#define acpi_disabled 1
#endif
