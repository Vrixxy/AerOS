#ifndef _KUNIT_TEST_BUG_H
#define _KUNIT_TEST_BUG_H
#define kunit_get_current_test() NULL
#define kunit_fail_current_test(fmt, ...) do { } while (0)
#endif
