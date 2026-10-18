use proto_host::wpa::*;

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

#[test]
fn sha1_and_hmac_vectors() {
    assert_eq!(sha1(b"abc"), hex("a9993e364706816aba3e25717850c26c9cd0d89d")[..]);
    assert_eq!(
        sha1(&[b'a'; 1000]),
        hex("291e9a6c66994949b57ba5e650361e98fc36b1ba")[..]
    );
    assert_eq!(
        hmac_sha1(b"Jefe", &[b"what do ya ", b"want for nothing?"]),
        hex("effcdf6ae5eb2fa2d27416d5f184df9c259a7c79")[..]
    );
}

#[test]
fn pmk_matches_the_ieee_vectors() {
    assert_eq!(
        passphrase_to_pmk("password", b"IEEE").unwrap(),
        hex("f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e")[..]
    );
    assert_eq!(
        passphrase_to_pmk("ThisIsAPassword", b"ThisIsASSID").unwrap(),
        hex("0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af")[..]
    );
    assert!(passphrase_to_pmk("short", b"IEEE").is_none());
    assert!(passphrase_to_pmk(&"x".repeat(64), b"IEEE").is_none());
    assert!(passphrase_to_pmk("password", b"").is_none());
}

#[test]
fn ptk_matches_an_independent_computation() {
    let pmk: [u8; 32] =
        hex("f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e").try_into().unwrap();
    let aa = [0xa0, 0xa1, 0xa1, 0xa1, 0xa1, 0xa1];
    let spa = [0xa2, 0xa2, 0xa2, 0xa2, 0xa2, 0xa3];
    let mut anonce = [0x11u8; 32];
    anonce[31] = 0x10;
    let mut snonce = [0x22u8; 32];
    snonce[31] = 0x21;
    let expected = hex("1ef244f95a2a040d3d4328bd808770dec430fc464ae3ba96d37746b5746ad35cba03da4c6697d8b1f54265fbf00f07c3");
    let ptk = derive_ptk(&pmk, &aa, &spa, &anonce, &snonce);
    assert_eq!(ptk.kck[..], expected[..16]);
    assert_eq!(ptk.kek[..], expected[16..32]);
    assert_eq!(ptk.tk[..], expected[32..]);
    // The order of the two sides does not matter.
    assert_eq!(derive_ptk(&pmk, &spa, &aa, &snonce, &anonce), ptk);
}

#[test]
fn aes_and_key_wrap_vectors() {
    let key: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
    let keys = expand_key(&key);
    let mut block: [u8; 16] = hex("00112233445566778899aabbccddeeff").try_into().unwrap();
    encrypt_block(&keys, &mut block);
    assert_eq!(block[..], hex("69c4e0d86a7b0430d8cdb78070b4c55a")[..]);
    decrypt_block(&keys, &mut block);
    assert_eq!(block[..], hex("00112233445566778899aabbccddeeff")[..]);

    // RFC 3394 section 4.1.
    let plain = hex("00112233445566778899aabbccddeeff");
    let wrapped = hex("1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5");
    let mut out = [0u8; 24];
    aes_wrap(&key, &plain, &mut out);
    assert_eq!(out[..], wrapped[..]);
    let mut back = [0u8; 16];
    assert!(aes_unwrap(&key, &wrapped, &mut back));
    assert_eq!(back[..], plain[..]);
    let mut damaged = wrapped.clone();
    damaged[5] ^= 1;
    assert!(!aes_unwrap(&key, &damaged, &mut back));
}

const AA: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
const SPA: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];

fn rsn() -> Vec<u8> {
    hex("30140100000fac040100000fac040100000fac020000")
}

/// The authenticator side: message 1 and message 3, built the way an access
/// point builds them.
struct Authenticator {
    pmk: [u8; 32],
    anonce: [u8; 32],
}

impl Authenticator {
    fn message1(&self, replay: u64) -> Vec<u8> {
        let mut out = [0u8; MAX_FRAME];
        let length = build_key_frame(
            &mut out,
            INFO_VERSION_2 | INFO_PAIRWISE | INFO_ACK,
            replay,
            &self.anonce,
            &[],
            None,
        );
        out[..length].to_vec()
    }

    fn message3(&self, replay: u64, snonce: &[u8; 32], gtk: &[u8], tamper: bool) -> Vec<u8> {
        let ptk = derive_ptk(&self.pmk, &AA, &SPA, &self.anonce, snonce);
        let mut data = rsn();
        data.extend_from_slice(&[0xdd, 6 + gtk.len() as u8, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
        data.extend_from_slice(gtk);
        if data.len() % 8 != 0 {
            data.push(0xdd);
            while data.len() % 8 != 0 {
                data.push(0);
            }
        }
        let mut wrapped = vec![0u8; data.len() + 8];
        aes_wrap(&ptk.kek, &data, &mut wrapped);
        let mut out = [0u8; MAX_FRAME];
        let info = INFO_VERSION_2
            | INFO_PAIRWISE
            | INFO_INSTALL
            | INFO_ACK
            | INFO_MIC
            | INFO_SECURE
            | INFO_ENCRYPTED;
        let length = build_key_frame(&mut out, info, replay, &self.anonce, &wrapped, Some(&ptk.kck));
        if tamper {
            out[length - 1] ^= 1;
        }
        out[..length].to_vec()
    }
}

fn supplicant(pmk: [u8; 32], snonce: [u8; 32]) -> Supplicant {
    Supplicant::new(pmk, AA, SPA, snonce, &rsn()).unwrap()
}

#[test]
fn four_way_handshake_completes() {
    let pmk = passphrase_to_pmk("correct horse", b"AerOS").unwrap();
    let anonce = [0x5au8; 32];
    let snonce = [0xa5u8; 32];
    let access_point = Authenticator { pmk, anonce };
    let mut station = supplicant(pmk, snonce);
    let mut out = [0u8; MAX_FRAME];

    let Reply::Send(length) = station.handle(&access_point.message1(1), &mut out).unwrap() else {
        panic!("message 1 must be answered");
    };
    let reply = parse_key_frame(&out[..length]).unwrap();
    assert_eq!(reply.nonce, snonce);
    assert_eq!(reply.replay, 1);
    assert_eq!(reply.data, &rsn()[..]);
    let ptk = derive_ptk(&pmk, &AA, &SPA, &anonce, &snonce);
    assert!(mic_matches(&ptk.kck, &out[..length], &reply.mic));

    let gtk = [0x77u8; 16];
    let Reply::Complete { send, tk, gtk: key, gtk_length, gtk_index } = station
        .handle(&access_point.message3(2, &snonce, &gtk, false), &mut out)
        .unwrap()
    else {
        panic!("message 3 must finish the handshake");
    };
    assert_eq!(tk, ptk.tk);
    assert_eq!(&key[..gtk_length], &gtk[..]);
    assert_eq!(gtk_index, 1);
    let last = parse_key_frame(&out[..send]).unwrap();
    assert!(last.info & INFO_SECURE != 0 && last.data.is_empty());
    assert!(mic_matches(&ptk.kck, &out[..send], &last.mic));
}

#[test]
fn a_wrong_key_or_a_forged_message_is_refused() {
    let anonce = [0x5au8; 32];
    let snonce = [0xa5u8; 32];
    let pmk = passphrase_to_pmk("correct horse", b"AerOS").unwrap();
    let access_point = Authenticator { pmk, anonce };
    let mut out = [0u8; MAX_FRAME];

    // The access point derived its keys from a different passphrase.
    let other = Authenticator {
        pmk: passphrase_to_pmk("battery staple", b"AerOS").unwrap(),
        anonce,
    };
    let mut station = supplicant(pmk, snonce);
    station.handle(&access_point.message1(1), &mut out).unwrap();
    assert_eq!(
        station.handle(&other.message3(2, &snonce, &[1; 16], false), &mut out).err(),
        Some(Error::BadMic)
    );

    // A damaged message 3.
    let mut station = supplicant(pmk, snonce);
    station.handle(&access_point.message1(1), &mut out).unwrap();
    assert_eq!(
        station.handle(&access_point.message3(2, &snonce, &[1; 16], true), &mut out).err(),
        Some(Error::BadMic)
    );

    // A replayed message 3 and one before message 1.
    let mut station = supplicant(pmk, snonce);
    assert_eq!(
        station.handle(&access_point.message3(2, &snonce, &[1; 16], false), &mut out).err(),
        Some(Error::UnexpectedMessage)
    );
    station.handle(&access_point.message1(5), &mut out).unwrap();
    assert_eq!(
        station.handle(&access_point.message3(5, &snonce, &[1; 16], false), &mut out).err(),
        Some(Error::Replay)
    );

    // Nonsense is not a frame.
    assert_eq!(station.handle(&[0u8; 20], &mut out).err(), Some(Error::Malformed));
}
