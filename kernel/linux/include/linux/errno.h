#ifndef _LINUX_ERRNO_H
#define _LINUX_ERRNO_H

#define EPERM 1
#define ENOENT 2
#define EINTR 4
#define EIO 5
#define ENXIO 6
#define E2BIG 7
#define EAGAIN 11
#define ENOMEM 12
#define EACCES 13
#define EFAULT 14
#define EBUSY 16
#define EEXIST 17
#define ENODEV 19
#define EINVAL 22
#define ENOSPC 28
#define ERANGE 34
#define ENOSYS 38
#define ENODATA 61
#define ETIME 62
#define EPROTO 71
#define EOVERFLOW 75
#define EOPNOTSUPP 95
#define ETIMEDOUT 110
#define EPROBE_DEFER 517
#define ESRCH 3
#define EBADF 9
#define ECHILD 10
#define ENOTBLK 15
#define EXDEV 18
#define ENOTDIR 20
#define EISDIR 21
#define ENFILE 23
#define EMFILE 24
#define ENOTTY 25
#define ETXTBSY 26
#define EFBIG 27
#define ESPIPE 29
#define EROFS 30
#define EMLINK 31
#define EPIPE 32
#define EDOM 33
#define EDEADLK 35
#define ENAMETOOLONG 36
#define ENOLCK 37
#define ENOTEMPTY 39
#define ELOOP 40
#define ENOMSG 42
#define EIDRM 43
#define ENOSTR 60
#define ENOSR 63
#define ENOLINK 67
#define EBADMSG 74
#define ENOTUNIQ 76
#define EBADFD 77
#define EILSEQ 84
#define EUSERS 87
#define ENOTSOCK 88
#define EMSGSIZE 90
#define ENOPROTOOPT 92
#define EPROTONOSUPPORT 93
#define EADDRINUSE 98
#define ENETDOWN 100
#define ECONNABORTED 103
#define ECONNRESET 104
#define ENOBUFS 105
#define ENOTCONN 107
#define ECONNREFUSED 111
#define EHOSTUNREACH 113
#define EALREADY 114
#define EINPROGRESS 115
#define ESTALE 116
#define EREMOTEIO 121
#define EDQUOT 122
#define ENOMEDIUM 123
#define EMEDIUMTYPE 124
#define ECANCELED 125
#define ENOKEY 126
#define EKEYEXPIRED 127
#define EKEYREVOKED 128
#define EKEYREJECTED 129
#define EOWNERDEAD 130
#define ENOTRECOVERABLE 131
#define ERFKILL 132
#define EHWPOISON 133
#define ERESTARTSYS 512
#define ERESTARTNOINTR 513
#define ERESTARTNOHAND 514
#define ENOIOCTLCMD 515
#define ERESTART_RESTARTBLOCK 516
#define EBADHANDLE 521
#define ENOTSUPP 524
#define ETOOSMALL 525
#define ESERVERFAULT 526
#define EBADTYPE 527
#define EJUKEBOX 528
#define EIOCBQUEUED 529
#define EMULTIHOP 72
#define EREMCHG 78
#define EWOULDBLOCK EAGAIN
#define EDEADLOCK EDEADLK

#define MAX_ERRNO 4095
#define IS_ERR_VALUE(x) ((unsigned long)(void *)(x) >= (unsigned long)-MAX_ERRNO)

static inline void *ERR_PTR(long error) { return (void *)error; }
static inline long PTR_ERR(const void *pointer) { return (long)pointer; }
static inline int IS_ERR(const void *pointer) { return IS_ERR_VALUE((unsigned long)pointer); }
static inline int IS_ERR_OR_NULL(const void *pointer) { return !pointer || IS_ERR(pointer); }

static inline void *ERR_CAST(const void *pointer) { return (void *)pointer; }
static inline int PTR_ERR_OR_ZERO(const void *pointer) { return IS_ERR(pointer) ? (int)PTR_ERR(pointer) : 0; }

#endif
