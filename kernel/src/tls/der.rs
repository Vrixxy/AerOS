//! A small DER reader: definite lengths, single-byte tags.

#[derive(Clone, Copy)]
pub struct Tlv<'a> {
    pub tag: u8,
    pub value: &'a [u8],
    /// The element with its header, as it appears in the input.
    pub raw: &'a [u8],
}

#[derive(Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

pub const SEQUENCE: u8 = 0x30;
pub const INTEGER: u8 = 0x02;
pub const BIT_STRING: u8 = 0x03;
pub const OCTET_STRING: u8 = 0x04;
pub const OID: u8 = 0x06;
pub const BOOLEAN: u8 = 0x01;
pub const UTC_TIME: u8 = 0x17;
pub const GENERALIZED_TIME: u8 = 0x18;

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.position >= self.data.len()
    }

    pub fn peek_tag(&self) -> Option<u8> {
        self.data.get(self.position).copied()
    }

    pub fn read(&mut self) -> Option<Tlv<'a>> {
        let start = self.position;
        let tag = *self.data.get(start)?;
        if tag & 0x1f == 0x1f {
            return None;
        }
        let first = *self.data.get(start + 1)?;
        let (length, header) = if first < 0x80 {
            (usize::from(first), 2)
        } else {
            let count = usize::from(first & 0x7f);
            if count == 0 || count > 4 {
                return None;
            }
            let mut length = 0usize;
            for index in 0..count {
                length = (length << 8) | usize::from(*self.data.get(start + 2 + index)?);
            }
            if length < 0x80 || (count > 1 && length < (1 << (8 * (count - 1)))) {
                return None;
            }
            (length, 2 + count)
        };
        let end = start.checked_add(header)?.checked_add(length)?;
        let value = self.data.get(start + header..end)?;
        self.position = end;
        Some(Tlv {
            tag,
            value,
            raw: &self.data[start..end],
        })
    }

    /// The next element's value if it has `tag`.
    pub fn expect(&mut self, tag: u8) -> Option<&'a [u8]> {
        let saved = self.position;
        match self.read() {
            Some(element) if element.tag == tag => Some(element.value),
            _ => {
                self.position = saved;
                None
            }
        }
    }

    pub fn expect_tlv(&mut self, tag: u8) -> Option<Tlv<'a>> {
        let saved = self.position;
        match self.read() {
            Some(element) if element.tag == tag => Some(element),
            _ => {
                self.position = saved;
                None
            }
        }
    }
}

/// Magnitude of a non-negative INTEGER, without its leading zero byte.
pub fn unsigned_integer(value: &[u8]) -> Option<&[u8]> {
    match value {
        [] => None,
        [first, ..] if first & 0x80 != 0 => None,
        [0, rest @ ..] if !rest.is_empty() => Some(rest),
        _ => Some(value),
    }
}

/// The two integers of an ECDSA-Sig-Value.
pub fn ecdsa_signature(der: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut outer = Reader::new(der);
    let body = outer.expect(SEQUENCE)?;
    if !outer.is_empty() {
        return None;
    }
    let mut inner = Reader::new(body);
    let r = unsigned_integer(inner.expect(INTEGER)?)?;
    let s = unsigned_integer(inner.expect(INTEGER)?)?;
    inner.is_empty().then_some((r, s))
}
