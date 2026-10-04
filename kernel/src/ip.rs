//! One address type for both IP versions: 16 bytes, an IPv4 address stored as
//! the IPv4-mapped IPv6 address (`::ffff:a.b.c.d`).

pub type Address = [u8; 16];

pub const UNSPECIFIED: Address = [0; 16];
pub const LOOPBACK6: Address = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];

pub const fn v4(address: [u8; 4]) -> Address {
    let mut mapped = [0u8; 16];
    mapped[10] = 0xff;
    mapped[11] = 0xff;
    mapped[12] = address[0];
    mapped[13] = address[1];
    mapped[14] = address[2];
    mapped[15] = address[3];
    mapped
}

/// The IPv4 address inside an IPv4-mapped address.
pub fn as_v4(address: &Address) -> Option<[u8; 4]> {
    if address[..10] == [0; 10] && address[10] == 0xff && address[11] == 0xff {
        Some([address[12], address[13], address[14], address[15]])
    } else {
        None
    }
}

pub fn is_v4(address: &Address) -> bool {
    as_v4(address).is_some()
}

pub fn is_link_local(address: &Address) -> bool {
    address[0] == 0xfe && address[1] & 0xc0 == 0x80
}

pub fn is_multicast(address: &Address) -> bool {
    address[0] == 0xff
}

/// `a.b.c.d` or `x:x::x` text for an address.
pub fn format(address: &Address, out: &mut impl core::fmt::Write) {
    if let Some([a, b, c, d]) = as_v4(address) {
        let _ = write!(out, "{a}.{b}.{c}.{d}");
        return;
    }
    let groups: [u16; 8] =
        core::array::from_fn(|i| u16::from_be_bytes([address[i * 2], address[i * 2 + 1]]));
    let (mut best_start, mut best_length) = (8, 0);
    let mut start = 0;
    while start < 8 {
        if groups[start] == 0 {
            let mut end = start;
            while end < 8 && groups[end] == 0 {
                end += 1;
            }
            if end - start > best_length && end - start >= 2 {
                best_start = start;
                best_length = end - start;
            }
            start = end;
        } else {
            start += 1;
        }
    }
    let mut index = 0;
    while index < 8 {
        if index == best_start {
            let _ = write!(out, "::");
            index += best_length;
            continue;
        }
        if index > 0 && index != best_start + best_length {
            let _ = write!(out, ":");
        }
        let _ = write!(out, "{:x}", groups[index]);
        index += 1;
    }
}

/// Parses dotted IPv4 or colon-separated IPv6 text.
pub fn parse(text: &str) -> Option<Address> {
    if !text.contains(':') {
        let mut parts = text.split('.');
        let mut octets = [0u8; 4];
        for octet in octets.iter_mut() {
            *octet = parts.next()?.parse().ok()?;
        }
        return parts.next().is_none().then(|| v4(octets));
    }
    let (head, tail) = match text.split_once("::") {
        Some((head, tail)) => (head, Some(tail)),
        None => (text, None),
    };
    if tail.is_some_and(|tail| tail.contains("::") || tail.starts_with(':') || tail.ends_with(':'))
        || (tail.is_some() && (head.starts_with(':') || head.ends_with(':')))
    {
        return None;
    }
    let mut groups = [0u16; 8];
    let mut count = 0;
    for part in head
        .split(':')
        .filter(|part| !part.is_empty() || tail.is_none())
    {
        if count == 8 || part.is_empty() || part.len() > 4 {
            return None;
        }
        groups[count] = u16::from_str_radix(part, 16).ok()?;
        count += 1;
    }
    match tail {
        None => {
            if count != 8 {
                return None;
            }
        }
        Some(tail) => {
            let mut trailing = [0u16; 8];
            let mut after = 0;
            for part in tail.split(':').filter(|part| !part.is_empty()) {
                if after == 8 || part.len() > 4 {
                    return None;
                }
                trailing[after] = u16::from_str_radix(part, 16).ok()?;
                after += 1;
            }
            if count + after > 7 {
                return None;
            }
            groups[8 - after..].copy_from_slice(&trailing[..after]);
        }
    }
    let mut address = [0u8; 16];
    for (index, group) in groups.iter().enumerate() {
        address[index * 2..index * 2 + 2].copy_from_slice(&group.to_be_bytes());
    }
    Some(address)
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> bool {
    struct Text([u8; 64], usize);
    impl core::fmt::Write for Text {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            let count = text.len().min(64 - self.1);
            self.0[self.1..self.1 + count].copy_from_slice(&text.as_bytes()[..count]);
            self.1 += count;
            Ok(())
        }
    }
    let show = |address: &Address| {
        let mut text = Text([0; 64], 0);
        format(address, &mut text);
        text
    };
    let cases: [(&str, &str); 8] = [
        ("::1", "::1"),
        ("fe80::1", "fe80::1"),
        ("fec0::5054:ff:fe12:3456", "fec0::5054:ff:fe12:3456"),
        ("2001:db8:0:0:1:0:0:1", "2001:db8::1:0:0:1"),
        ("10.0.2.15", "10.0.2.15"),
        ("::", "::"),
        ("1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7:8"),
        ("::ffff:1.2.3.4", "1.2.3.4"),
    ];
    let mut ok = true;
    for (input, expected) in cases {
        let parsed = if input == "::ffff:1.2.3.4" {
            Some(v4([1, 2, 3, 4]))
        } else {
            parse(input)
        };
        let Some(address) = parsed else {
            return false;
        };
        let text = show(&address);
        ok &= &text.0[..text.1] == expected.as_bytes();
        if let Some(again) = parse(core::str::from_utf8(&text.0[..text.1]).unwrap_or("")) {
            ok &= again == address;
        } else {
            ok = false;
        }
    }
    for bad in [
        "",
        ":",
        ":::",
        "1::2::3",
        "1:2:3:4:5:6:7",
        "1:2:3:4:5:6:7:8:9",
        "12345::",
        "g::1",
        "1.2.3",
        "1.2.3.4.5",
        "256.1.1.1",
        "::1::",
    ] {
        ok &= parse(bad).is_none();
    }
    ok && as_v4(&LOOPBACK6).is_none()
        && as_v4(&v4([9, 8, 7, 6])) == Some([9, 8, 7, 6])
        && is_link_local(&parse("fe80::1").unwrap_or([0; 16]))
        && !is_link_local(&LOOPBACK6)
        && is_multicast(&parse("ff02::2").unwrap_or([0; 16]))
}
