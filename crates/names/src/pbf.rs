//! The Protocol Buffers wire format, as far as vector tiles need it: fields read with their raw
//! bytes kept (so anything not understood can be written back as it was), and writers.

use anyhow::{bail, Result};

/// A field's value by wire type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wire<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

/// A field: its number, value, and every byte of it as read (key included).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Field<'a> {
    pub num: u32,
    pub wire: Wire<'a>,
    pub raw: &'a [u8],
}

/// Reads the fields of a message in order.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for i in 0..10 {
            let Some(&b) = self.buf.get(self.pos) else { bail!("truncated varint") };
            self.pos += 1;
            if i == 9 && b > 1 {
                bail!("varint over 64 bits");
            }
            v |= u64::from(b & 0x7f) << (7 * i);
            if b < 0x80 {
                return Ok(v);
            }
        }
        bail!("varint over 10 bytes")
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.buf.len());
        let Some(end) = end else { bail!("field runs past the end of its message") };
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// The next field, or `None` at the end.
    pub fn next_field(&mut self) -> Result<Option<Field<'a>>> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let start = self.pos;
        let key = self.varint()?;
        let num = key >> 3;
        if num == 0 || num > (1 << 29) - 1 {
            bail!("field number {num} out of range");
        }
        let wire = match key & 7 {
            0 => Wire::Varint(self.varint()?),
            1 => Wire::Fixed64(u64::from_le_bytes(fixed(self.take(8)?))),
            2 => {
                let n = self.varint()?;
                let Ok(n) = usize::try_from(n) else { bail!("length {n} too large") };
                Wire::Bytes(self.take(n)?)
            }
            5 => Wire::Fixed32(u32::from_le_bytes(fixed(self.take(4)?))),
            w => bail!("wire type {w} (groups and unknown types are not read)"),
        };
        Ok(Some(Field { num: num as u32, wire, raw: &self.buf[start..self.pos] }))
    }
}

fn fixed<const N: usize>(s: &[u8]) -> [u8; N] {
    let mut a = [0u8; N];
    a.copy_from_slice(&s[..N]);
    a
}

/// The varints of a packed field, each of which must fit 32 bits.
pub(crate) fn packed_u32(bytes: &[u8], out: &mut Vec<u32>) -> Result<()> {
    let mut r = Reader::new(bytes);
    while r.pos < bytes.len() {
        let v = r.varint()?;
        let Ok(v) = u32::try_from(v) else { bail!("packed value {v} over 32 bits") };
        out.push(v);
    }
    Ok(())
}

pub(crate) fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub(crate) fn put_key(out: &mut Vec<u8>, num: u32, wire: u8) {
    put_varint(out, (u64::from(num) << 3) | u64::from(wire));
}

pub(crate) fn put_bytes(out: &mut Vec<u8>, num: u32, bytes: &[u8]) {
    put_key(out, num, 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

pub(crate) fn put_uint(out: &mut Vec<u8>, num: u32, v: u64) {
    put_key(out, num, 0);
    put_varint(out, v);
}

/// A packed field (written only when it has values).
pub(crate) fn put_packed(out: &mut Vec<u8>, num: u32, values: &[u32]) {
    if values.is_empty() {
        return;
    }
    let len: usize = values.iter().map(|v| varint_len(u64::from(*v))).sum();
    put_key(out, num, 2);
    put_varint(out, len as u64);
    for v in values {
        put_varint(out, u64::from(*v));
    }
}

fn varint_len(v: u64) -> usize {
    (64 - (v | 1).leading_zeros() as usize).div_ceil(7)
}

pub(crate) fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

pub(crate) fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints() {
        for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
            let mut b = Vec::new();
            put_varint(&mut b, v);
            assert_eq!(b.len(), varint_len(v));
            assert_eq!(Reader::new(&b).varint().expect("varint"), v);
        }
        // Not minimal, but valid.
        assert_eq!(Reader::new(&[0x80, 0x00]).varint().expect("varint"), 0);
        assert!(Reader::new(&[0x80]).varint().is_err());
        assert!(Reader::new(&[0xff; 10]).varint().is_err());
        for v in [0i64, -1, 1, i64::MIN, i64::MAX, -12345] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        assert_eq!(zigzag(-1), 1);
        assert_eq!(zigzag(1), 2);
    }

    #[test]
    fn fields() {
        let mut b = Vec::new();
        put_uint(&mut b, 15, 2);
        put_bytes(&mut b, 1, b"water");
        put_key(&mut b, 2, 5);
        b.extend_from_slice(&1.5f32.to_bits().to_le_bytes());
        put_key(&mut b, 3, 1);
        b.extend_from_slice(&2.5f64.to_bits().to_le_bytes());
        let mut r = Reader::new(&b);
        let f = r.next_field().expect("ok").expect("field");
        assert_eq!((f.num, f.wire, f.raw), (15, Wire::Varint(2), &b[..2]));
        let f = r.next_field().expect("ok").expect("field");
        assert_eq!((f.num, f.wire), (1, Wire::Bytes(b"water")));
        let f = r.next_field().expect("ok").expect("field");
        assert_eq!(f.wire, Wire::Fixed32(1.5f32.to_bits()));
        let f = r.next_field().expect("ok").expect("field");
        assert_eq!(f.wire, Wire::Fixed64(2.5f64.to_bits()));
        assert!(r.next_field().expect("ok").is_none());
        // Truncated, field 0, groups.
        assert!(Reader::new(&[0x0a, 0x05, b'a']).next_field().is_err());
        assert!(Reader::new(&[0x00, 0x01]).next_field().is_err());
        assert!(Reader::new(&[0x0b]).next_field().is_err());
        let mut p = Vec::new();
        put_packed(&mut p, 4, &[9, 50, 34, u32::MAX]);
        let f = Reader::new(&p).next_field().expect("ok").expect("field");
        let Wire::Bytes(body) = f.wire else { panic!("packed is length-delimited") };
        let mut vals = Vec::new();
        packed_u32(body, &mut vals).expect("packed");
        assert_eq!(vals, vec![9, 50, 34, u32::MAX]);
        let mut over = Vec::new();
        put_varint(&mut over, u64::from(u32::MAX) + 1);
        assert!(packed_u32(&over, &mut vals).is_err());
    }
}
