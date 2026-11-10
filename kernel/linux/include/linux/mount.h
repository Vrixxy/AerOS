#ifndef _LINUX_MOUNT_H
#define _LINUX_MOUNT_H

struct vfsmount {
	struct super_block *mnt_sb;
};

#endif
