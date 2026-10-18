use proto_host::microcode::*;

fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn dword_sum(bytes: &[u8]) -> u32 {
    bytes
        .chunks_exact(4)
        .fold(0u32, |total, chunk| total.wrapping_add(u32::from_le_bytes(chunk.try_into().unwrap())))
}

/// An Intel update file; `data` is the payload size (0 for the default
/// 2000/2048 layout), `extended` the extra (signature, flags) pairs.
fn intel(signature: u32, flags: u32, revision: u32, data: usize, extended: &[(u32, u32)]) -> Vec<u8> {
    let payload = if data == 0 { 2000 } else { data };
    let main = 48 + payload;
    let table = if extended.is_empty() { 0 } else { 20 + 12 * extended.len() };
    let mut bytes = vec![0u8; main + table];
    put(&mut bytes, 0, 1);
    put(&mut bytes, 4, revision);
    put(&mut bytes, 8, 0x0112_2024);
    put(&mut bytes, 12, signature);
    put(&mut bytes, 20, 1);
    put(&mut bytes, 24, flags);
    put(&mut bytes, 28, data as u32);
    put(&mut bytes, 32, if data == 0 && table == 0 { 0 } else { (main + table) as u32 });
    for index in 48..main {
        bytes[index] = (index * 7) as u8;
    }
    let sum = dword_sum(&bytes[..main]);
    put(&mut bytes, 16, 0u32.wrapping_sub(sum));
    if table != 0 {
        let checksum = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let original = signature.wrapping_add(flags).wrapping_add(checksum);
        put(&mut bytes, main, extended.len() as u32);
        for (index, (sig, pf)) in extended.iter().enumerate() {
            let at = main + 20 + index * 12;
            put(&mut bytes, at, *sig);
            put(&mut bytes, at + 4, *pf);
            put(&mut bytes, at + 8, original.wrapping_sub(sig.wrapping_add(*pf)));
        }
        let total = dword_sum(&bytes[main..]);
        put(&mut bytes, main + 4, 0u32.wrapping_sub(total));
    }
    bytes
}

fn cpu(signature: u32, platform: u32, revision: u32) -> Cpu {
    Cpu {
        vendor: Vendor::Intel,
        signature,
        platform,
        revision,
    }
}

#[test]
fn finds_the_update_for_a_signature() {
    let blob = intel(0x000906ea, 0x20, 0xf4, 0, &[]);
    let found = find(&blob, &cpu(0x000906ea, 0x20, 0xea)).unwrap();
    assert_eq!(found.offset, 48);
    assert_eq!(found.revision, 0xf4);
    assert_eq!(found.length, 2000);
    assert_eq!(find(&blob, &cpu(0x000906ea, 0x20, 0xf4)), None, "same revision");
    assert_eq!(find(&blob, &cpu(0x000906ea, 0x20, 0xf6)), None, "newer already running");
    assert_eq!(find(&blob, &cpu(0x000906eb, 0x20, 0)), None, "other model");
    assert_eq!(find(&blob, &cpu(0x000906ea, 0x01, 0)), None, "other platform");
}

#[test]
fn takes_the_newest_of_several() {
    let mut blob = Vec::new();
    blob.extend(intel(0x000306c3, 0x02, 0x10, 0, &[]));
    blob.extend(intel(0x000906ea, 0x20, 0xe0, 0, &[]));
    blob.extend(intel(0x000906ea, 0x20, 0xf8, 0, &[]));
    blob.extend(intel(0x000906ea, 0x20, 0xf0, 0, &[]));
    let found = find(&blob, &cpu(0x000906ea, 0x20, 0xd0)).unwrap();
    assert_eq!(found.revision, 0xf8);
    assert_eq!(found.offset, 2048 * 2 + 48);
}

#[test]
fn revisions_compare_as_signed() {
    let blob = intel(0x000206a7, 0x10, 0x29, 0, &[]);
    assert!(find(&blob, &cpu(0x000206a7, 0x10, 0xffff_fff0)).is_some());
    let blob = intel(0x000206a7, 0x10, 0xffff_fff0, 0, &[]);
    assert!(find(&blob, &cpu(0x000206a7, 0x10, 0x29)).is_none());
}

#[test]
fn a_zero_platform_mask_matches_only_a_zero_platform() {
    let blob = intel(0x000506e3, 0, 0x100, 0, &[]);
    assert!(find(&blob, &cpu(0x000506e3, 0, 0)).is_some());
    assert!(find(&blob, &cpu(0x000506e3, 0x08, 0)).is_none());
}

#[test]
fn a_damaged_update_is_skipped() {
    let mut blob = intel(0x000906ea, 0x20, 0xf4, 0, &[]);
    blob[900] ^= 0x40;
    assert_eq!(find(&blob, &cpu(0x000906ea, 0x20, 0)), None);
    let mut both = blob.clone();
    both.extend(intel(0x000906ea, 0x20, 0xf0, 0, &[]));
    let found = find(&both, &cpu(0x000906ea, 0x20, 0)).unwrap();
    assert_eq!(found.revision, 0xf0);
}

#[test]
fn extended_signatures_match_other_steppings() {
    let blob = intel(0x000906ea, 0x20, 0xf4, 1024, &[(0x000906eb, 0x02), (0x000906ec, 0x20)]);
    assert_eq!(blob.len() % 4, 0);
    let found = find(&blob, &cpu(0x000906ec, 0x20, 0)).unwrap();
    assert_eq!(found.length, 1024);
    assert!(find(&blob, &cpu(0x000906eb, 0x02, 0)).is_some());
    assert!(find(&blob, &cpu(0x000906eb, 0x20, 0)).is_none());
    assert!(find(&blob, &cpu(0x000906ea, 0x20, 0)).is_some());

    let mut broken = blob.clone();
    let at = broken.len() - 12 + 8;
    broken[at] ^= 1;
    assert!(find(&broken, &cpu(0x000906ec, 0x20, 0)).is_none());
}

#[test]
fn truncated_and_unknown_blobs_find_nothing() {
    let blob = intel(0x000906ea, 0x20, 0xf4, 0, &[]);
    assert_eq!(find(&blob[..1500], &cpu(0x000906ea, 0x20, 0)), None);
    assert_eq!(find(&[], &cpu(1, 1, 0)), None);
    assert_eq!(find(&[0u8; 4096], &cpu(1, 1, 0)), None);
    let mut wrong = blob.clone();
    wrong[0] = 2;
    assert_eq!(find(&wrong, &cpu(0x000906ea, 0x20, 0)), None);
}

fn amd_container(equivalence: &[(u32, u16)], patches: &[(u16, u32)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x0041_4d44u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(((equivalence.len() + 1) * 16) as u32).to_le_bytes());
    for (installed, equivalent) in equivalence {
        bytes.extend_from_slice(&installed.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&equivalent.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
    }
    bytes.extend_from_slice(&[0u8; 16]);
    for (target, revision) in patches {
        let mut patch = vec![0u8; 3200];
        patch[4..8].copy_from_slice(&revision.to_le_bytes());
        patch[24..26].copy_from_slice(&target.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(patch.len() as u32).to_le_bytes());
        bytes.extend(patch);
    }
    bytes
}

fn amd(signature: u32, revision: u32) -> Cpu {
    Cpu {
        vendor: Vendor::Amd,
        signature,
        platform: 0,
        revision,
    }
}

#[test]
fn amd_container_patches_are_matched_through_the_equivalence_table() {
    let blob = amd_container(&[(0x00a20f10, 0xa200), (0x00a20f12, 0xa200), (0x00860f01, 0x8601)],
        &[(0x8601, 0x0860_0107), (0xa200, 0x0a20_0102), (0xa200, 0x0a20_0104)]);
    let found = find(&blob, &amd(0x00a20f10, 0x0a20_0100)).unwrap();
    assert_eq!(found.revision, 0x0a20_0104);
    assert_eq!(found.length, 3200);
    assert_eq!(&blob[found.offset..found.offset + 4], &[0u8; 4]);
    assert_eq!(u32::from_le_bytes(blob[found.offset + 4..found.offset + 8].try_into().unwrap()), 0x0a20_0104);
    assert!(find(&blob, &amd(0x00a20f12, 0x0a20_0104)).is_none());
    assert!(find(&blob, &amd(0x00860f01, 0)).is_some_and(|patch| patch.revision == 0x0860_0107));
    assert!(find(&blob, &amd(0x00a50f00, 0)).is_none(), "no equivalence entry");
}

#[test]
fn amd_containers_can_follow_each_other() {
    let mut blob = amd_container(&[(0x00860f01, 0x8601)], &[(0x8601, 0x0860_0107)]);
    blob.extend(amd_container(&[(0x00a20f10, 0xa200)], &[(0xa200, 0x0a20_0104)]));
    assert!(find(&blob, &amd(0x00a20f10, 0)).is_some());
    assert!(find(&blob, &amd(0x00860f01, 0)).is_some());
    let truncated = &blob[..blob.len() - 100];
    assert!(find(truncated, &amd(0x00a20f10, 0)).is_none());
}
