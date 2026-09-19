//! Hex text and byte-reader helpers shared by every codec in the crate.
//!
//! Every function here returns `Option` or writes into a formatter. It names no
//! error type, so each call site keeps the error value and message it already
//! had: the duplication these replace was the loop body, never the vocabulary.

use std::fmt;

/// Lower-case hex, the only form this crate writes.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

/// Writes lower-case hex straight into a formatter, for `Display` impls.
pub(crate) fn write_hex(bytes: &[u8], formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    for byte in bytes {
        write!(formatter, "{byte:02x}")?;
    }
    Ok(())
}

/// One hex digit, upper or lower case. Nothing else is a digit, which is what
/// makes this stricter than `u8::from_str_radix`: that accepts a leading `+`.
const fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// Decodes exactly `N` bytes of hex, or nothing.
pub(crate) fn hex_decode_fixed<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut bytes = [0_u8; N];
    for (byte, pair) in bytes.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
        *byte = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Some(bytes)
}

/// Decodes hex of any even length, or nothing.
pub(crate) fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| Some((hex_digit(pair[0])? << 4) | hex_digit(pair[1])?))
        .collect()
}

/// A forward-only cursor over untrusted bytes.
///
/// Every read is bounds-checked and returns `None` at the end, so a codec maps
/// one `None` to its own error rather than repeating the arithmetic.
pub(crate) struct ByteReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> ByteReader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// The next `length` bytes.
    pub(crate) fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(length)?;
        let value = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(value)
    }

    /// The next `N` bytes, copied into an array.
    pub(crate) fn take_array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    /// The next byte.
    pub(crate) fn take_u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    /// How many bytes are still unread.
    pub(crate) const fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Succeeds only when everything was read, so trailing bytes are a failure
    /// rather than silence.
    pub(crate) const fn finish(&self) -> Option<()> {
        if self.is_empty() { Some(()) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_in_lower_case_and_reads_either_case() {
        let bytes = [0x00_u8, 0x0f, 0xa5, 0xff];
        assert_eq!(hex_encode(&bytes), "000fa5ff");
        assert_eq!(hex_decode_fixed::<4>("000fa5ff"), Some(bytes));
        assert_eq!(hex_decode_fixed::<4>("000FA5FF"), Some(bytes));
        assert_eq!(hex_decode("000fa5ff").as_deref(), Some(bytes.as_slice()));
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_decode(""), Some(Vec::new()));
    }

    /// `u8::from_str_radix` accepts `+f`; the digit table does not, and every
    /// hex reader in the crate now shares the table.
    #[test]
    fn a_sign_or_a_wrong_width_is_not_hex() {
        assert_eq!(hex_decode_fixed::<2>("+f0a"), None);
        assert_eq!(hex_decode("+f"), None);
        assert_eq!(hex_decode_fixed::<2>("0f"), None);
        assert_eq!(hex_decode_fixed::<2>("0f0a0b"), None);
        assert_eq!(hex_decode("0f0"), None);
        assert_eq!(hex_decode_fixed::<1>("g0"), None);
        assert_eq!(hex_decode_fixed::<1>("0é"), None);
    }

    #[test]
    fn a_reader_stops_at_the_end_of_its_bytes() {
        let mut reader = ByteReader::new(&[1, 2, 3, 4, 5]);
        assert_eq!(reader.take_u8(), Some(1));
        assert_eq!(reader.take_array::<2>(), Some([2, 3]));
        assert_eq!(reader.remaining(), 2);
        assert_eq!(reader.take(3), None);
        assert_eq!(reader.remaining(), 2, "a refused read consumes nothing");
        assert_eq!(reader.take(2), Some([4, 5].as_slice()));
        assert_eq!(reader.take_u8(), None);
    }

    #[test]
    fn finish_refuses_trailing_bytes() {
        let mut reader = ByteReader::new(&[1, 2]);
        assert_eq!(reader.take_u8(), Some(1));
        assert_eq!(reader.finish(), None);
        assert_eq!(reader.take_u8(), Some(2));
        assert_eq!(reader.finish(), Some(()));
    }
}
