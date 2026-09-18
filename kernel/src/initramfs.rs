use crate::vfs::InitramfsEntry;

const INIT_LENGTH: usize = 0x2100;

pub static INIT_ELF: [u8; INIT_LENGTH] = make_init_elf();
pub static FAULT_ELF: [u8; 0x1002] = make_fault_elf();
pub static COMPILED_INIT: &[u8] = include_bytes!("../../assets/userspace/aeros-init");
pub static STD_SMOKE: &[u8] = include_bytes!("../../assets/userspace/aeros-std-smoke");
pub static RELEASE: &[u8] = b"AerOS 0.1.0 x86_64\n";

pub fn entries() -> [InitramfsEntry; 5] {
    [
        InitramfsEntry {
            path: "/bin/init",
            data: &INIT_ELF,
            mode: 0o555,
        },
        InitramfsEntry {
            path: "/etc/aeros-release",
            data: RELEASE,
            mode: 0o444,
        },
        InitramfsEntry {
            path: "/bin/fault-probe",
            data: &FAULT_ELF,
            mode: 0o555,
        },
        InitramfsEntry {
            path: "/bin/aeros-init",
            data: COMPILED_INIT,
            mode: 0o555,
        },
        InitramfsEntry {
            path: "/bin/aeros-std-smoke",
            data: STD_SMOKE,
            mode: 0o555,
        },
    ]
}

const fn make_fault_elf() -> [u8; 0x1002] {
    let mut image = [0; 0x1002];
    image[0] = 0x7f;
    image[1] = b'E';
    image[2] = b'L';
    image[3] = b'F';
    image[4] = 2;
    image[5] = 1;
    image[6] = 1;
    put_fault_u16(&mut image, 16, 3);
    put_fault_u16(&mut image, 18, 62);
    put_fault_u32(&mut image, 20, 1);
    put_fault_u64(&mut image, 24, 0x1000);
    put_fault_u64(&mut image, 32, 64);
    put_fault_u16(&mut image, 52, 64);
    put_fault_u16(&mut image, 54, 56);
    put_fault_u16(&mut image, 56, 1);
    put_fault_u32(&mut image, 64, 1);
    put_fault_u32(&mut image, 68, 5);
    put_fault_u64(&mut image, 72, 0x1000);
    put_fault_u64(&mut image, 80, 0x1000);
    put_fault_u64(&mut image, 96, 2);
    put_fault_u64(&mut image, 104, 2);
    put_fault_u64(&mut image, 112, 0x1000);
    image[0x1000] = 0x0f;
    image[0x1001] = 0x0b;
    image
}

const fn put_fault_u16(destination: &mut [u8; 0x1002], offset: usize, value: u16) {
    destination[offset] = value as u8;
    destination[offset + 1] = (value >> 8) as u8;
}

const fn put_fault_u32(destination: &mut [u8; 0x1002], offset: usize, value: u32) {
    let mut index = 0;
    while index < 4 {
        destination[offset + index] = (value >> (index * 8)) as u8;
        index += 1;
    }
}

const fn put_fault_u64(destination: &mut [u8; 0x1002], offset: usize, value: u64) {
    let mut index = 0;
    while index < 8 {
        destination[offset + index] = (value >> (index * 8)) as u8;
        index += 1;
    }
}

const fn make_init_elf() -> [u8; INIT_LENGTH] {
    let mut image = [0; INIT_LENGTH];
    image[0] = 0x7f;
    image[1] = b'E';
    image[2] = b'L';
    image[3] = b'F';
    image[4] = 2;
    image[5] = 1;
    image[6] = 1;
    image[7] = 0;
    put_u16(&mut image, 16, 3);
    put_u16(&mut image, 18, 62);
    put_u32(&mut image, 20, 1);
    put_u64(&mut image, 24, 0x1000);
    put_u64(&mut image, 32, 64);
    put_u16(&mut image, 52, 64);
    put_u16(&mut image, 54, 56);
    put_u16(&mut image, 56, 3);
    put_u32(&mut image, 64, 1);
    put_u32(&mut image, 68, 5);
    put_u64(&mut image, 72, 0x1000);
    put_u64(&mut image, 80, 0x1000);
    put_u64(&mut image, 112, 0x1000);
    put_u32(&mut image, 120, 1);
    put_u32(&mut image, 124, 6);
    put_u64(&mut image, 128, 0x2000);
    put_u64(&mut image, 136, 0x3000);
    put_u64(&mut image, 152, 0x100);
    put_u64(&mut image, 160, 0x1000);
    put_u64(&mut image, 168, 0x1000);
    put_u32(&mut image, 176, 1);
    put_u32(&mut image, 180, 4);
    put_u64(&mut image, 184, 0);
    put_u64(&mut image, 192, 0);
    put_u64(&mut image, 208, 232);
    put_u64(&mut image, 216, 232);
    put_u64(&mut image, 224, 0x1000);
    let mut cursor = 0x1000;
    emit(&mut image, &mut cursor, &[0xb8, 0x01, 0x01, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff],
    );
    emit_rip_address(&mut image, &mut cursor, 0x3000);
    emit(&mut image, &mut cursor, &[0x31, 0xd2]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xd2]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x89, 0xc7, 0x41, 0x89, 0xc4]);
    emit(&mut image, &mut cursor, &[0x31, 0xc0]);
    emit_rip_address(&mut image, &mut cursor, 0x3100);
    emit(&mut image, &mut cursor, &[0xba, 0x20, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x05, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xe7]);
    emit_rip_address(&mut image, &mut cursor, 0x3280);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x08, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xe7]);
    emit(&mut image, &mut cursor, &[0x31, 0xf6, 0x31, 0xd2]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x48, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xe7]);
    emit(&mut image, &mut cursor, &[0xbe, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x03, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xe7, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x01, 0x01, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff],
    );
    emit_rip_address(&mut image, &mut cursor, 0x3090);
    emit(&mut image, &mut cursor, &[0xba, 0x00, 0x00, 0x01, 0x00]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x41, 0x89, 0xc6]);
    emit(&mut image, &mut cursor, &[0xb8, 0xd9, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xf7]);
    emit_rip_address(&mut image, &mut cursor, 0x35c0);
    emit(&mut image, &mut cursor, &[0xba, 0x00, 0x01, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x03, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xf7, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x06, 0x01, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff],
    );
    emit_rip_address(&mut image, &mut cursor, 0x3000);
    emit_rip_register(&mut image, &mut cursor, 0x3310, 0x15);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x0b, 0x01, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff],
    );
    emit_rip_address(&mut image, &mut cursor, 0x3080);
    emit_rip_register(&mut image, &mut cursor, 0x33a0, 0x15);
    emit(
        &mut image,
        &mut cursor,
        &[0x41, 0xba, 0x20, 0x00, 0x00, 0x00, 0x0f, 0x05],
    );
    emit_rip_address(&mut image, &mut cursor, 0x33c0);
    emit(&mut image, &mut cursor, &[0x49, 0x89, 0xf2]);
    emit(&mut image, &mut cursor, &[0xb8, 0x2e, 0x01, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xff]);
    emit(&mut image, &mut cursor, &[0xbe, 0x07, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x4e, 0x01, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3400, 0x3d);
    emit(&mut image, &mut cursor, &[0xbe, 0x20, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xd2]);
    emit(
        &mut image,
        &mut cursor,
        &[0x41, 0xba, 0x53, 0x30, 0x05, 0x53, 0x0f, 0x05],
    );
    emit(&mut image, &mut cursor, &[0xb8, 0xca, 0x00, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3440, 0x3d);
    emit(&mut image, &mut cursor, &[0xbe, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xba, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x0d, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x02, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xf6]);
    emit_rip_register(&mut image, &mut cursor, 0x3480, 0x15);
    emit(
        &mut image,
        &mut cursor,
        &[0x41, 0xba, 0x08, 0x00, 0x00, 0x00],
    );
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x0e, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x02, 0x00, 0x00, 0x00]);
    emit_rip_address(&mut image, &mut cursor, 0x3510);
    emit_rip_register(&mut image, &mut cursor, 0x3500, 0x15);
    emit(
        &mut image,
        &mut cursor,
        &[0x41, 0xba, 0x08, 0x00, 0x00, 0x00],
    );
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x83, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xff]);
    emit_rip_address(&mut image, &mut cursor, 0x3520);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0xba, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0xcc, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xff]);
    emit(&mut image, &mut cursor, &[0xbe, 0x08, 0x00, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3540, 0x15);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x35, 0x01, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3550, 0x3d);
    emit_rip_address(&mut image, &mut cursor, 0x3554);
    emit(&mut image, &mut cursor, &[0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x4f, 0x00, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3560, 0x3d);
    emit(&mut image, &mut cursor, &[0xbe, 0x10, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x1c, 0x00, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3000, 0x3d);
    emit(&mut image, &mut cursor, &[0xbe, 0x00, 0x10, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x10, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbe, 0x13, 0x54, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3580, 0x15);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x27, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0xe4, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x01, 0x00, 0x00, 0x00]);
    emit_rip_address(&mut image, &mut cursor, 0x3120);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x3e, 0x01, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3140, 0x3d);
    emit(&mut image, &mut cursor, &[0xbe, 0x20, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x9e, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x02, 0x10, 0x00, 0x00]);
    emit_rip_address(&mut image, &mut cursor, 0x3180);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x9e, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x03, 0x10, 0x00, 0x00]);
    emit_rip_address(&mut image, &mut cursor, 0x3190);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x3f, 0x00, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3200, 0x3d);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0xda, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xff, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x11, 0x01, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x31a0, 0x3d);
    emit(&mut image, &mut cursor, &[0xbe, 0x18, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x0c, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xff, 0x0f, 0x05]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0x8d, 0xb8, 0x00, 0x20, 0x00, 0x00],
    );
    emit(&mut image, &mut cursor, &[0xb8, 0x0c, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x09, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xff]);
    emit(&mut image, &mut cursor, &[0xbe, 0x00, 0x10, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xba, 0x03, 0x00, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x41, 0xba, 0x22, 0x00, 0x00, 0x00],
    );
    emit(
        &mut image,
        &mut cursor,
        &[0x49, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff],
    );
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xc9, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x49, 0x89, 0xc5]);
    emit(&mut image, &mut cursor, &[0xb8, 0x0a, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x4c, 0x89, 0xef]);
    emit(&mut image, &mut cursor, &[0xbe, 0x00, 0x10, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xba, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x0b, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x4c, 0x89, 0xef]);
    emit(&mut image, &mut cursor, &[0xbe, 0x00, 0x10, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x29, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x02, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbe, 0x02, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xba, 0x11, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05, 0x41, 0x89, 0xc7]);
    emit(&mut image, &mut cursor, &[0xb8, 0x2a, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff]);
    emit_rip_address(&mut image, &mut cursor, 0x30c0);
    emit(&mut image, &mut cursor, &[0xba, 0x10, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x2c, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff]);
    emit_rip_address(&mut image, &mut cursor, 0x30a0);
    emit(&mut image, &mut cursor, &[0xba, 0x1b, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xd2]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xc0]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xc9, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x2d, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff]);
    emit_rip_address(&mut image, &mut cursor, 0x3600);
    emit(&mut image, &mut cursor, &[0xba, 0x00, 0x02, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xd2]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xc0]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xc9, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x03, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x01, 0x01, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff],
    );
    emit_rip_address(&mut image, &mut cursor, 0x3000);
    emit(&mut image, &mut cursor, &[0x31, 0xd2]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x41, 0x89, 0xc7]);
    emit(&mut image, &mut cursor, &[0xb8, 0x11, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff]);
    emit_rip_address(&mut image, &mut cursor, 0x3700);
    emit(&mut image, &mut cursor, &[0xba, 0x05, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x48, 0x83, 0xec, 0x20]);
    emit_rip_register(&mut image, &mut cursor, 0x3710, 0x05);
    emit(&mut image, &mut cursor, &[0x48, 0x89, 0x04, 0x24]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0x44, 0x24, 0x08, 0x05, 0x00, 0x00, 0x00],
    );
    emit_rip_register(&mut image, &mut cursor, 0x3720, 0x05);
    emit(&mut image, &mut cursor, &[0x48, 0x89, 0x44, 0x24, 0x10]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0x44, 0x24, 0x18, 0x05, 0x00, 0x00, 0x00],
    );
    emit(&mut image, &mut cursor, &[0xb8, 0x13, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff]);
    emit(&mut image, &mut cursor, &[0x48, 0x89, 0xe6]);
    emit(&mut image, &mut cursor, &[0xba, 0x02, 0x00, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x0f, 0x05, 0x48, 0x83, 0xc4, 0x20],
    );
    emit(&mut image, &mut cursor, &[0xb8, 0x15, 0x00, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3000, 0x3d);
    emit(&mut image, &mut cursor, &[0xbe, 0x04, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x4c, 0x01, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff],
    );
    emit_rip_address(&mut image, &mut cursor, 0x3000);
    emit(&mut image, &mut cursor, &[0x31, 0xd2]);
    emit(
        &mut image,
        &mut cursor,
        &[0x41, 0xba, 0xff, 0x07, 0x00, 0x00],
    );
    emit_rip_register(&mut image, &mut cursor, 0x3800, 0x05);
    emit(&mut image, &mut cursor, &[0x49, 0x89, 0xc0]);
    emit(&mut image, &mut cursor, &[0xb8, 0x4c, 0x01, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x48, 0x83, 0xec, 0x20]);
    emit_rip_register(&mut image, &mut cursor, 0x3040, 0x05);
    emit(&mut image, &mut cursor, &[0x48, 0x89, 0x04, 0x24]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0x44, 0x24, 0x08, 0x05, 0x00, 0x00, 0x00],
    );
    emit_rip_register(&mut image, &mut cursor, 0x3045, 0x05);
    emit(&mut image, &mut cursor, &[0x48, 0x89, 0x44, 0x24, 0x10]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0x44, 0x24, 0x18, 0x01, 0x00, 0x00, 0x00],
    );
    emit(&mut image, &mut cursor, &[0xb8, 0x14, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x48, 0x89, 0xe6]);
    emit(&mut image, &mut cursor, &[0xba, 0x02, 0x00, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x0f, 0x05, 0x48, 0x83, 0xc4, 0x20],
    );
    emit(&mut image, &mut cursor, &[0xb8, 0x03, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x01, 0x01, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff],
    );
    emit_rip_address(&mut image, &mut cursor, 0x30d0);
    emit(&mut image, &mut cursor, &[0xba, 0x42, 0x02, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x41, 0xba, 0x80, 0x01, 0x00, 0x00],
    );
    emit(&mut image, &mut cursor, &[0x0f, 0x05, 0x41, 0x89, 0xc7]);
    emit(&mut image, &mut cursor, &[0xb8, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff]);
    emit_rip_address(&mut image, &mut cursor, 0x3040);
    emit(&mut image, &mut cursor, &[0xba, 0x06, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x08, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff]);
    emit(&mut image, &mut cursor, &[0x31, 0xf6, 0x31, 0xd2]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x31, 0xc0]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff]);
    emit_rip_address(&mut image, &mut cursor, 0x3900);
    emit(&mut image, &mut cursor, &[0xba, 0x06, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x03, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0xc9, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xff, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x60, 0x00, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3a00, 0x3d);
    emit(&mut image, &mut cursor, &[0x31, 0xf6, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x23, 0x00, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x30e0, 0x3d);
    emit(&mut image, &mut cursor, &[0x31, 0xf6, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0xe6, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xf6]);
    emit_rip_register(&mut image, &mut cursor, 0x30e0, 0x15);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x50, 0x00, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x30f0, 0x3d);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x4f, 0x00, 0x00, 0x00]);
    emit_rip_register(&mut image, &mut cursor, 0x3b00, 0x3d);
    emit(&mut image, &mut cursor, &[0xbe, 0x10, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x01, 0x01, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x48, 0xc7, 0xc7, 0x9c, 0xff, 0xff, 0xff],
    );
    emit_rip_address(&mut image, &mut cursor, 0x30f8);
    emit(&mut image, &mut cursor, &[0x31, 0xd2]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xd2, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x41, 0x89, 0xc7]);
    emit(&mut image, &mut cursor, &[0xb8, 0x09, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x31, 0xff]);
    emit(&mut image, &mut cursor, &[0xbe, 0x00, 0x10, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xba, 0x01, 0x00, 0x00, 0x00]);
    emit(
        &mut image,
        &mut cursor,
        &[0x41, 0xba, 0x02, 0x00, 0x00, 0x00],
    );
    emit(&mut image, &mut cursor, &[0x45, 0x89, 0xf8]);
    emit(&mut image, &mut cursor, &[0x45, 0x31, 0xc9, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x49, 0x89, 0xc5]);
    emit(&mut image, &mut cursor, &[0xb8, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x4c, 0x89, 0xee]);
    emit(&mut image, &mut cursor, &[0xba, 0x06, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x0b, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x4c, 0x89, 0xef]);
    emit(&mut image, &mut cursor, &[0xbe, 0x00, 0x10, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x20, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x41, 0x89, 0xc6]);
    emit(&mut image, &mut cursor, &[0xb8, 0x03, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xff, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x05, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xf7]);
    emit_rip_address(&mut image, &mut cursor, 0x3b40);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x21, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x44, 0x89, 0xf6, 0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0x89, 0xc7]);
    emit(&mut image, &mut cursor, &[0xb8, 0x03, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x24, 0x01, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x02, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbe, 0x0b, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xba, 0x00, 0x00, 0x08, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05, 0x89, 0xc7]);
    emit(&mut image, &mut cursor, &[0xb8, 0x03, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x48, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbe, 0x06, 0x04, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xba, 0x0c, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05, 0x89, 0xc7]);
    emit(&mut image, &mut cursor, &[0xb8, 0x03, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x01, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x01, 0x00, 0x00, 0x00]);
    emit_rip_address(&mut image, &mut cursor, 0x3040);
    emit(&mut image, &mut cursor, &[0xba, 0x19, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05]);
    emit(&mut image, &mut cursor, &[0xb8, 0x3c, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0xbf, 0x49, 0x00, 0x00, 0x00]);
    emit(&mut image, &mut cursor, &[0x0f, 0x05, 0x0f, 0x0b]);
    put_u64(&mut image, 96, (cursor - 0x1000) as u64);
    put_u64(&mut image, 104, (cursor - 0x1000) as u64);
    let mut data_cursor = 0x2000;
    emit(&mut image, &mut data_cursor, b"/etc/aeros-release\0");
    data_cursor = 0x2040;
    emit(&mut image, &mut data_cursor, b"AerOS init via Linux ABI\n");
    data_cursor = 0x2080;
    emit(&mut image, &mut data_cursor, b"/proc/self/exe\0");
    data_cursor = 0x2090;
    emit(&mut image, &mut data_cursor, b"/bin\0");
    data_cursor = 0x20a0;
    emit(
        &mut image,
        &mut data_cursor,
        b"\xb5\x12\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x09localhost\x00\x00\x01\x00\x01",
    );
    data_cursor = 0x20c0;
    emit(
        &mut image,
        &mut data_cursor,
        &[2, 0, 0, 53, 10, 0, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    data_cursor = 0x20d0;
    emit(&mut image, &mut data_cursor, b"/tmp/probe\0");
    data_cursor = 0x20e0;
    emit(
        &mut image,
        &mut data_cursor,
        &[0, 0, 0, 0, 0, 0, 0, 0, 0x10, 0x27, 0, 0, 0, 0, 0, 0],
    );
    data_cursor = 0x20f0;
    emit(&mut image, &mut data_cursor, b"/tmp\0");
    data_cursor = 0x20f8;
    emit(&mut image, &mut data_cursor, b"probe\0");
    image
}

const fn emit(destination: &mut [u8; INIT_LENGTH], cursor: &mut usize, bytes: &[u8]) {
    let mut index = 0;
    while index < bytes.len() {
        destination[*cursor] = bytes[index];
        *cursor += 1;
        index += 1;
    }
}

const fn emit_rip_address(destination: &mut [u8; INIT_LENGTH], cursor: &mut usize, target: usize) {
    emit(destination, cursor, &[0x48, 0x8d, 0x35]);
    let displacement = target.wrapping_sub(*cursor + 4) as u32;
    put_u32(destination, *cursor, displacement);
    *cursor += 4;
}

const fn emit_rip_register(
    destination: &mut [u8; INIT_LENGTH],
    cursor: &mut usize,
    target: usize,
    register: u8,
) {
    emit(destination, cursor, &[0x48, 0x8d, register]);
    let displacement = target.wrapping_sub(*cursor + 4) as u32;
    put_u32(destination, *cursor, displacement);
    *cursor += 4;
}

const fn put_u16(destination: &mut [u8; INIT_LENGTH], offset: usize, value: u16) {
    destination[offset] = value as u8;
    destination[offset + 1] = (value >> 8) as u8;
}

const fn put_u32(destination: &mut [u8; INIT_LENGTH], offset: usize, value: u32) {
    let mut index = 0;
    while index < 4 {
        destination[offset + index] = (value >> (index * 8)) as u8;
        index += 1;
    }
}

const fn put_u64(destination: &mut [u8; INIT_LENGTH], offset: usize, value: u64) {
    let mut index = 0;
    while index < 8 {
        destination[offset + index] = (value >> (index * 8)) as u8;
        index += 1;
    }
}
