//! Signed system updates. A new boot image is accepted only with an Ed25519
//! signature from a key the administrator trusted (`pkg trust`, the same
//! store the package manager uses). It is written to `EFI/BOOT/BOOTX64.NEW`
//! on the boot volume, read back and hashed, the current image is kept as
//! `BOOTX64.OLD`, and the new one replaces `BOOTX64.EFI` with one
//! crash-safe rename, so a power cut leaves either the old or the new image
//! in place, never a torn one. `rollback` puts the old image back.

#[cfg(feature = "boot-test")]
use core::fmt::Write;

use crate::auth::Sha256;
use crate::block::Disk;
use crate::fatfs::{Fs, FsError, Node};
use crate::measure;
use crate::memory::PageBuffer;
use crate::pkg::{self, Failure};
use crate::vfs::{self, VfsError};

const IMAGE: &[u8] = b"BOOTX64.EFI";
const STAGED: &[u8] = b"BOOTX64.NEW";
const PREVIOUS: &[u8] = b"BOOTX64.OLD";
const DIRECTORY: &[u8] = b"/EFI/BOOT";
const MAX_IMAGE: u64 = 64 << 20;
const CHUNK: usize = 64 * 1024;

pub struct Applied {
    pub bytes: u64,
    pub digest: [u8; 32],
    pub kept_previous: bool,
}

fn storage(error: FsError) -> Failure {
    Failure::Storage(match error {
        FsError::NotFound => VfsError::NotFound,
        FsError::NoSpace => VfsError::NodeLimit,
        FsError::TooLarge => VfsError::FileTooLarge,
        _ => VfsError::Busy,
    })
}

type Reader<'a> = dyn FnMut(&mut Fs, u64, &mut [u8]) -> Result<usize, Failure> + 'a;

/// Creates `name` in `dir` (replacing any file of that name) and fills it
/// from `reader`. Returns the new file, its size and its digest.
fn copy_into(
    fs: &mut Fs,
    dir: u32,
    name: &[u8],
    buffer: &mut [u8],
    reader: &mut Reader,
) -> Result<(Node, u64, [u8; 32]), Failure> {
    if let Ok(existing) = fs.find(dir, name) {
        fs.remove(&existing).map_err(storage)?;
    }
    let mut node = fs.create_file(dir, name).map_err(storage)?;
    let mut hash = Sha256::new();
    let mut offset = 0u64;
    loop {
        let count = reader(fs, offset, buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        if fs
            .write_at(&mut node, offset, &buffer[..count])
            .map_err(storage)?
            != count
        {
            return Err(Failure::TooLarge);
        }
        offset += count as u64;
    }
    Ok((node, offset, hash.finish()))
}

fn hash_node(fs: &mut Fs, node: &mut Node, buffer: &mut [u8]) -> Result<[u8; 32], Failure> {
    let mut hash = Sha256::new();
    let mut offset = 0u64;
    loop {
        let count = fs.read_at(node, offset, buffer).map_err(storage)?;
        if count == 0 {
            return Ok(hash.finish());
        }
        hash.update(&buffer[..count]);
        offset += count as u64;
    }
}

/// SHA-256 of the image the volume would boot.
pub fn boot_image_digest(fs: &mut Fs) -> Option<[u8; 32]> {
    let mut buffer = PageBuffer::new(CHUNK)?;
    let mut node = fs.resolve(b"/EFI/BOOT/BOOTX64.EFI").ok()?;
    hash_node(fs, &mut node, buffer.as_mut_slice()).ok()
}

/// Installs `image` on the volume if `signature` is valid for a trusted key
/// under `key_root`.
pub fn apply_to_volume(
    fs: &mut Fs,
    key_root: &str,
    image: &str,
    signature: &str,
) -> Result<Applied, Failure> {
    let mut keys = [[0u8; 32]; 8];
    let key_count = pkg::trusted_keys(key_root, &mut keys);
    if key_count == 0 {
        return Err(Failure::NoTrustedKeys);
    }
    let mut raw = [0u8; 65];
    if pkg::read_all(signature, &mut raw)? != 64 {
        return Err(Failure::BadSignature);
    }
    let mut signature_bytes = [0u8; 64];
    signature_bytes.copy_from_slice(&raw[..64]);
    if !pkg::signature_valid(image, &signature_bytes, &keys[..key_count])? {
        return Err(Failure::BadSignature);
    }
    if vfs::metadata(image)?.size > MAX_IMAGE {
        return Err(Failure::TooLarge);
    }
    let directory = fs.resolve(DIRECTORY).map_err(storage)?;
    if !directory.is_directory() {
        return Err(Failure::Missing);
    }
    let dir = directory.as_dir();
    let mut buffer = PageBuffer::new(CHUNK).ok_or(Failure::TooLarge)?;
    let source = vfs::open_file(image, false, false, false, 0, false)?;
    let staged = copy_into(
        fs,
        dir,
        STAGED,
        buffer.as_mut_slice(),
        &mut |_, offset, out| vfs::read_at(source, offset as usize, out).map_err(Failure::Storage),
    );
    let _ = vfs::close(source);
    let (mut staged, bytes, digest) = staged?;
    if hash_node(fs, &mut staged, buffer.as_mut_slice())? != digest {
        let _ = fs.remove(&staged);
        return Err(Failure::HashMismatch);
    }
    let mut kept_previous = false;
    if let Ok(mut current) = fs.find(dir, IMAGE) {
        copy_into(
            fs,
            dir,
            PREVIOUS,
            buffer.as_mut_slice(),
            &mut |fs, offset, out| fs.read_at(&mut current, offset, out).map_err(storage),
        )?;
        kept_previous = true;
    }
    fs.rename(&staged, dir, IMAGE, true).map_err(storage)?;
    Ok(Applied {
        bytes,
        digest,
        kept_previous,
    })
}

/// Puts the previous image back.
pub fn rollback_volume(fs: &mut Fs) -> Result<[u8; 32], Failure> {
    let directory = fs.resolve(DIRECTORY).map_err(storage)?;
    let dir = directory.as_dir();
    let mut previous = fs.find(dir, PREVIOUS).map_err(|_| Failure::NoPrevious)?;
    let mut buffer = PageBuffer::new(CHUNK).ok_or(Failure::TooLarge)?;
    let digest = hash_node(fs, &mut previous, buffer.as_mut_slice())?;
    fs.rename(&previous, dir, IMAGE, true).map_err(storage)?;
    Ok(digest)
}

fn boot_volume() -> Result<crate::fatfs::Box64, Failure> {
    let disk: Disk = crate::blockdev::boot_disk().ok_or(Failure::Missing)?;
    let start = crate::fat::volume_start().ok_or(Failure::Missing)?;
    Fs::mount(disk, start).map_err(storage)
}

/// Seals the digest, drops the cached view of the volume the other FAT
/// driver and the block cache hold, after a change through this one.
fn settle(digest: Option<&[u8; 32]>) {
    if let Some(disk) = crate::blockdev::boot_disk() {
        crate::block::invalidate_disk(disk);
    }
    crate::fat::invalidate_cache();
    if let Some(digest) = digest {
        measure::seal_digest(measure::REFERENCE_PATH, digest);
    }
}

/// `update <image> <signature>` on the volume the machine boots from.
pub fn apply_to_boot_volume(
    key_root: &str,
    image: &str,
    signature: &str,
) -> Result<Applied, Failure> {
    let mut volume = boot_volume()?;
    let result = apply_to_volume(&mut volume, key_root, image, signature);
    drop(volume);
    settle(result.as_ref().ok().map(|applied| &applied.digest));
    if let Ok(applied) = &result {
        crate::audit::record(
            "UPDATE",
            format_args!(
                "installed {} bytes digest={}",
                applied.bytes,
                measure::hex_text(&applied.digest).as_str()
            ),
        );
    }
    result
}

pub fn rollback_boot_volume() -> Result<[u8; 32], Failure> {
    let mut volume = boot_volume()?;
    let result = rollback_volume(&mut volume);
    drop(volume);
    settle(result.as_ref().ok());
    if let Ok(digest) = &result {
        crate::audit::record(
            "UPDATE",
            format_args!(
                "rolled back to digest={}",
                measure::hex_text(digest).as_str()
            ),
        );
    }
    result
}

#[cfg(feature = "boot-test")]
pub struct TestReport {
    pub applied: bool,
    pub previous_kept: bool,
    pub bad_signature_rejected: bool,
    pub tampered_rejected: bool,
    pub untrusted_rejected: bool,
    pub rollback: bool,
    pub crash_cases: u32,
    pub torn: u32,
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> TestReport {
    use crate::auth::sha256;
    use crate::pkg_vectors as vectors;

    let keys = "/tmp/updkeys";
    let empty = "/tmp/updnokeys";
    let image = "/tmp/update.img";
    let tampered = "/tmp/update.bad";
    let good_signature = "/tmp/update.sig";
    let other_signature = "/tmp/update.other";
    let marker: &[u8] = b"previous boot image";
    let new_digest = sha256(vectors::UPDATE_IMAGE);
    let old_digest = sha256(marker);
    let write = |name: &str, data: &[u8]| {
        vfs::open_file(name, true, false, true, 0o644, true)
            .ok()
            .is_some_and(|handle| {
                let ok = vfs::write(handle, data, false) == Ok(data.len());
                let _ = vfs::close(handle);
                ok
            })
    };
    let mut changed = [0u8; 3000];
    changed.copy_from_slice(vectors::UPDATE_IMAGE);
    changed[1234] ^= 1;
    let ready = write(image, vectors::UPDATE_IMAGE)
        && write(tampered, &changed)
        && write(good_signature, vectors::UPDATE_IMAGE_SIG)
        && write(other_signature, vectors::UPDATE_IMAGE_SIG_OTHER)
        && pkg::trust(keys, &vectors::KEY).is_ok();

    // A volume with an old image in EFI/BOOT, as the firmware would find it.
    let fresh = || -> Option<()> {
        Fs::format(Disk::Ram, 0, crate::blockdev::RAM_DISK_SECTORS, b"UPDVOL").ok()?;
        let mut volume = Fs::mount(Disk::Ram, 0).ok()?;
        let root = volume.root().as_dir();
        let efi = volume.create_dir(root, b"EFI").ok()?;
        let boot = volume.create_dir(efi.as_dir(), b"BOOT").ok()?;
        let mut old = volume.create_file(boot.as_dir(), IMAGE).ok()?;
        volume.write_at(&mut old, 0, marker).ok()?;
        Some(())
    };
    let digest_of = |name: &[u8]| -> Option<[u8; 32]> {
        let mut volume = Fs::mount(Disk::Ram, 0).ok()?;
        let mut path = pkg::Buf::<64>::new();
        let _ = write!(path, "/EFI/BOOT/{}", core::str::from_utf8(name).ok()?);
        let mut node = volume.resolve(path.as_str().as_bytes()).ok()?;
        let mut buffer = PageBuffer::new(CHUNK)?;
        hash_node(&mut volume, &mut node, buffer.as_mut_slice()).ok()
    };
    let exists = |name: &[u8]| digest_of(name).is_some();

    let mut report = TestReport {
        applied: false,
        previous_kept: false,
        bad_signature_rejected: false,
        tampered_rejected: false,
        untrusted_rejected: false,
        rollback: false,
        crash_cases: 0,
        torn: 0,
    };
    if ready && fresh().is_some() {
        let mut volume = Fs::mount(Disk::Ram, 0).ok();
        let mut run = |key_root: &str, image: &str, signature: &str| {
            volume
                .as_mut()
                .map(|volume| apply_to_volume(volume, key_root, image, signature))
        };
        report.untrusted_rejected = matches!(
            run(empty, image, good_signature),
            Some(Err(Failure::NoTrustedKeys))
        );
        report.bad_signature_rejected = matches!(
            run(keys, image, other_signature),
            Some(Err(Failure::BadSignature))
        );
        report.tampered_rejected = matches!(
            run(keys, tampered, good_signature),
            Some(Err(Failure::BadSignature))
        );
        let untouched = digest_of(IMAGE) == Some(old_digest) && !exists(STAGED);
        report.bad_signature_rejected &= untouched;
        let outcome = run(keys, image, good_signature);
        drop(volume);
        report.applied = outcome.as_ref().is_some_and(|result| {
            result.as_ref().is_ok_and(|done| {
                done.digest == new_digest && done.bytes == vectors::UPDATE_IMAGE.len() as u64
            })
        }) && digest_of(IMAGE) == Some(new_digest)
            && !exists(STAGED);
        report.previous_kept = outcome
            .as_ref()
            .is_some_and(|result| result.as_ref().is_ok_and(|done| done.kept_previous))
            && digest_of(PREVIOUS) == Some(old_digest);
        report.rollback = Fs::mount(Disk::Ram, 0)
            .ok()
            .is_some_and(|mut volume| rollback_volume(&mut volume).ok() == Some(old_digest))
            && digest_of(IMAGE) == Some(old_digest)
            && !exists(PREVIOUS);
    }

    // Power cut after every number of sector writes: the boot image is
    // always the old one or the new one, whole.
    for limit in 0..400u32 {
        if !ready || fresh().is_none() {
            break;
        }
        crate::blockdev::inject_power_loss_after(limit);
        if let Ok(mut volume) = Fs::mount(Disk::Ram, 0) {
            let _ = apply_to_volume(&mut volume, keys, image, good_signature);
        }
        let dropped = crate::blockdev::clear_power_loss();
        report.crash_cases += 1;
        let current = digest_of(IMAGE);
        if current != Some(old_digest) && current != Some(new_digest) {
            report.torn += 1;
        }
        if dropped == 0 {
            break;
        }
    }
    for name in [
        image,
        tampered,
        good_signature,
        other_signature,
        "/tmp/updkeys/trusted.keys",
    ] {
        let _ = vfs::remove(name, false);
    }
    for name in [keys, empty] {
        let _ = vfs::remove(name, true);
    }
    report
}
