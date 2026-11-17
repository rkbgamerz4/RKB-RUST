//! Primitive codecs shared by every protocol state.

use crate::error::{Error, Result};

/// Java counts `String.length()` in UTF-16 code units, not Unicode scalars.
/// Minecraft's string limits use that count.
fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// A cursor over one packet body or frame.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    /// Starts at the beginning of `data`.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    /// How many bytes have been consumed.
    #[must_use]
    pub fn position(&self) -> usize {
        self.position
    }

    /// How many bytes are left.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len() - self.position
    }

    /// Whether every byte has been consumed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Rejects a body that still has bytes after its declared fields.
    pub fn finish(&self) -> Result<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(Error::TrailingBytes(self.remaining()))
        }
    }

    /// Reads one unsigned byte.
    pub fn read_u8(&mut self) -> Result<u8> {
        let byte = *self.data.get(self.position).ok_or(Error::UnexpectedEof {
            needed: 1,
            available: self.remaining(),
        })?;
        self.position += 1;
        Ok(byte)
    }

    /// Reads exactly `length` bytes.
    pub fn read_bytes(&mut self, length: usize) -> Result<&'a [u8]> {
        if length > self.remaining() {
            return Err(Error::UnexpectedEof {
                needed: length,
                available: self.remaining(),
            });
        }
        let start = self.position;
        self.position += length;
        Ok(&self.data[start..self.position])
    }

    /// Reads the rest of the input.
    pub fn read_rest(&mut self) -> &'a [u8] {
        let start = self.position;
        self.position = self.data.len();
        &self.data[start..]
    }

    /// Reads a protocol VarInt (at most five bytes, two's complement for
    /// negative values).
    pub fn read_varint(&mut self) -> Result<i32> {
        let mut value = 0u32;
        for index in 0..5 {
            let byte = self.read_u8()?;
            value |= u32::from(byte & 0x7F) << (7 * index);
            if byte & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(Error::VarIntTooLong)
    }

    /// Reads a protocol VarLong (at most ten bytes).
    pub fn read_varlong(&mut self) -> Result<i64> {
        let mut value = 0u64;
        for index in 0..10 {
            let byte = self.read_u8()?;
            value |= u64::from(byte & 0x7F) << (7 * index);
            if byte & 0x80 == 0 {
                return Ok(value as i64);
            }
        }
        Err(Error::VarLongTooLong)
    }

    /// Reads a boolean (`0` false, `1` true).
    pub fn read_bool(&mut self) -> Result<bool> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(Error::protocol(format!("invalid boolean {other}"))),
        }
    }

    /// Reads a big-endian `u16`.
    pub fn read_u16(&mut self) -> Result<u16> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// Reads a big-endian `i32`.
    pub fn read_i32(&mut self) -> Result<i32> {
        let bytes = self.read_bytes(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Reads a big-endian `i16`.
    pub fn read_i16(&mut self) -> Result<i16> {
        let bytes = self.read_bytes(2)?;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// Reads a big-endian `i64`.
    pub fn read_i64(&mut self) -> Result<i64> {
        let bytes = self.read_bytes(8)?;
        Ok(i64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    /// Reads a big-endian `f32`.
    pub fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.read_i32()? as u32))
    }

    /// Reads a big-endian `f64`.
    pub fn read_f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.read_i64()? as u64))
    }

    /// Reads a UUID as sixteen big-endian bytes.
    pub fn read_uuid(&mut self) -> Result<u128> {
        let bytes = self.read_bytes(16)?;
        let mut value = [0u8; 16];
        value.copy_from_slice(bytes);
        Ok(u128::from_be_bytes(value))
    }

    /// Reads a length-prefixed UTF-8 string whose **UTF-16 code unit** count is
    /// at most `max_chars` (matching Java's `String.length()`). The encoded
    /// byte length is checked against `max_chars * 3`, as vanilla does.
    pub fn read_string(&mut self, max_chars: usize) -> Result<String> {
        let length = self.read_varint()?;
        if length < 0 {
            return Err(Error::NegativeLength(length));
        }
        let length = length as usize;
        let max_bytes = max_chars.saturating_mul(3);
        if length > max_bytes {
            return Err(Error::StringTooLong {
                length,
                max: max_bytes,
            });
        }
        let bytes = self.read_bytes(length)?;
        let value = std::str::from_utf8(bytes).map_err(|_| Error::InvalidUtf8)?;
        let chars = utf16_len(value);
        if chars > max_chars {
            return Err(Error::StringTooLong {
                length: chars,
                max: max_chars,
            });
        }
        Ok(value.to_owned())
    }

    /// Reads a VarInt-length-prefixed byte array of at most `max` bytes.
    pub fn read_byte_array(&mut self, max: usize) -> Result<&'a [u8]> {
        let length = self.read_varint()?;
        if length < 0 {
            return Err(Error::NegativeLength(length));
        }
        let length = length as usize;
        if length > max {
            return Err(Error::PacketTooLarge { size: length, max });
        }
        self.read_bytes(length)
    }

    /// Reads a boolean presence flag and, when set, one value.
    pub fn read_optional<T>(
        &mut self,
        read: impl FnOnce(&mut Reader<'a>) -> Result<T>,
    ) -> Result<Option<T>> {
        if self.read_bool()? {
            Ok(Some(read(self)?))
        } else {
            Ok(None)
        }
    }

    /// Reads one anonymous NBT value and returns its exact bytes, including the
    /// leading tag id.
    ///
    /// Network NBT omits the root name that file NBT carries. RKB does not need
    /// every NBT field in a play packet, but it must know where the value ends,
    /// so this validates the structure and skips the payload.
    pub fn read_anonymous_nbt_raw(&mut self, max_bytes: usize) -> Result<&'a [u8]> {
        let start = self.position;
        let tag = self.read_u8()?;
        if tag == 0 {
            return Err(Error::protocol("anonymous NBT has TAG_End as its root"));
        }
        self.skip_nbt_payload(tag, 0)?;
        let end = self.position;
        if end - start > max_bytes {
            return Err(Error::PacketTooLarge {
                size: end - start,
                max: max_bytes,
            });
        }
        Ok(&self.data[start..end])
    }

    /// Reads an optional anonymous NBT value: tag `0` means absent.
    ///
    /// This is the in-packet `Optional NBT` encoding used by block entities and
    /// some components, where the absence marker is `TAG_End`.
    pub fn read_optional_nbt_raw(&mut self, max_bytes: usize) -> Result<Option<&'a [u8]>> {
        if self.data.get(self.position).copied() == Some(0) {
            self.position += 1;
            Ok(None)
        } else {
            Ok(Some(self.read_anonymous_nbt_raw(max_bytes)?))
        }
    }

    /// Reads a boolean-prefixed optional anonymous NBT value.
    ///
    /// Registry entries use a boolean presence flag followed by the tag, which
    /// is a different encoding from [`Self::read_optional_nbt_raw`].
    pub fn read_prefixed_optional_nbt_raw(&mut self, max_bytes: usize) -> Result<Option<&'a [u8]>> {
        if self.read_bool()? {
            Ok(Some(self.read_anonymous_nbt_raw(max_bytes)?))
        } else {
            Ok(None)
        }
    }

    fn skip_nbt_payload(&mut self, tag: u8, depth: u16) -> Result<()> {
        if depth > 512 {
            return Err(Error::protocol("NBT nesting is deeper than 512 levels"));
        }
        match tag {
            1 => {
                self.read_bytes(1)?;
            }
            2 => {
                self.read_bytes(2)?;
            }
            3 => {
                self.read_bytes(4)?;
            }
            4 => {
                self.read_bytes(8)?;
            }
            5 => {
                self.read_bytes(4)?;
            }
            6 => {
                self.read_bytes(8)?;
            }
            7 => {
                let length = self.read_signed_length("byte array")?;
                self.read_bytes(length)?;
            }
            8 => {
                let length = self.read_u16()? as usize;
                self.read_bytes(length)?;
            }
            9 => {
                let element = self.read_u8()?;
                let count = self.read_signed_length("list")?;
                if element == 0 {
                    if count != 0 {
                        return Err(Error::protocol("NBT list of TAG_End has entries"));
                    }
                } else {
                    for _ in 0..count {
                        self.skip_nbt_payload(element, depth + 1)?;
                    }
                }
            }
            10 => loop {
                let child = self.read_u8()?;
                if child == 0 {
                    break;
                }
                let name_length = self.read_u16()? as usize;
                self.read_bytes(name_length)?;
                self.skip_nbt_payload(child, depth + 1)?;
            },
            11 => {
                let count = self.read_signed_length("int array")?;
                let length = count
                    .checked_mul(4)
                    .ok_or_else(|| Error::protocol("NBT int array length overflows"))?;
                self.read_bytes(length)?;
            }
            12 => {
                let count = self.read_signed_length("long array")?;
                let length = count
                    .checked_mul(8)
                    .ok_or_else(|| Error::protocol("NBT long array length overflows"))?;
                self.read_bytes(length)?;
            }
            other => {
                return Err(Error::protocol(format!(
                    "unknown NBT tag type {other} while skipping"
                )))
            }
        }
        Ok(())
    }

    fn read_signed_length(&mut self, what: &str) -> Result<usize> {
        const MAX_COLLECTION: i32 = 16_777_216;
        let length = self.read_i32()?;
        if length < 0 {
            return Err(Error::protocol(format!(
                "negative NBT {what} length {length}"
            )));
        }
        if length > MAX_COLLECTION {
            return Err(Error::protocol(format!(
                "NBT {what} length {length} exceeds {MAX_COLLECTION}"
            )));
        }
        Ok(length as usize)
    }
}

/// A buffer that writes protocol primitives.
#[derive(Clone, Debug, Default)]
pub struct Writer {
    data: Vec<u8>,
}

impl Writer {
    /// An empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes written so far.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    /// Number of bytes written.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether nothing has been written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Consumes the writer and returns its bytes.
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.data
    }

    /// Writes one unsigned byte.
    pub fn write_u8(&mut self, value: u8) {
        self.data.push(value);
    }

    /// Writes a boolean as `0` or `1`.
    pub fn write_bool(&mut self, value: bool) {
        self.data.push(u8::from(value));
    }

    /// Writes a big-endian `u16`.
    pub fn write_u16(&mut self, value: u16) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// Writes one signed byte.
    pub fn write_i8(&mut self, value: i8) {
        self.write_u8(value as u8);
    }

    /// Writes a big-endian `i16`.
    pub fn write_i16(&mut self, value: i16) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// Writes a big-endian `i32`.
    pub fn write_i32(&mut self, value: i32) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// Writes a big-endian `i64`.
    pub fn write_i64(&mut self, value: i64) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// Writes a big-endian `f32`.
    pub fn write_f32(&mut self, value: f32) {
        self.write_i32(value.to_bits() as i32);
    }

    /// Writes a big-endian `f64`.
    pub fn write_f64(&mut self, value: f64) {
        self.write_i64(value.to_bits() as i64);
    }

    /// Writes raw bytes.
    pub fn write_bytes(&mut self, value: &[u8]) {
        self.data.extend_from_slice(value);
    }

    /// Writes a VarInt.
    pub fn write_varint(&mut self, value: i32) {
        let mut value = value as u32;
        loop {
            let mut byte = (value & 0x7F) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            self.data.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    /// Writes a VarLong.
    pub fn write_varlong(&mut self, value: i64) {
        let mut value = value as u64;
        loop {
            let mut byte = (value & 0x7F) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            self.data.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    /// Writes a UUID as sixteen big-endian bytes.
    pub fn write_uuid(&mut self, value: u128) {
        self.data.extend_from_slice(&value.to_be_bytes());
    }

    /// Writes a length-prefixed UTF-8 string, checking the character limit.
    pub fn write_string(&mut self, value: &str, max_chars: usize) -> Result<()> {
        let chars = utf16_len(value);
        if chars > max_chars || value.len() > max_chars.saturating_mul(3) {
            return Err(Error::StringTooLong {
                length: chars,
                max: max_chars,
            });
        }
        let length = i32::try_from(value.len()).map_err(|_| Error::StringTooLong {
            length: value.len(),
            max: max_chars.saturating_mul(3),
        })?;
        self.write_varint(length);
        self.write_bytes(value.as_bytes());
        Ok(())
    }

    /// Writes a VarInt-length-prefixed byte array, checking the byte limit.
    pub fn write_byte_array(&mut self, value: &[u8], max: usize) -> Result<()> {
        if value.len() > max {
            return Err(Error::PacketTooLarge {
                size: value.len(),
                max,
            });
        }
        let length = i32::try_from(value.len()).map_err(|_| Error::PacketTooLarge {
            size: value.len(),
            max,
        })?;
        self.write_varint(length);
        self.write_bytes(value);
        Ok(())
    }

    /// Writes a boolean presence flag and, when set, one value.
    pub fn write_optional<T>(
        &mut self,
        value: Option<&T>,
        write: impl FnOnce(&mut Writer, &T) -> Result<()>,
    ) -> Result<()> {
        match value {
            Some(value) => {
                self.write_bool(true);
                write(self, value)
            }
            None => {
                self.write_bool(false);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_golden_vectors() {
        let cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (255, &[0xFF, 0x01]),
            (767, &[0xFF, 0x05]),
            (2147483647, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
            (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
            (-2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
        ];
        for (value, bytes) in cases {
            let mut writer = Writer::new();
            writer.write_varint(*value);
            assert_eq!(writer.as_slice(), *bytes, "encode {value}");
            let mut reader = Reader::new(bytes);
            assert_eq!(reader.read_varint().unwrap(), *value, "decode {value}");
            assert!(reader.is_empty());
        }
    }

    #[test]
    fn varlong_golden_vectors() {
        let mut writer = Writer::new();
        writer.write_varlong(-1);
        let mut expected = [0xFFu8; 10];
        expected[9] = 0x01;
        assert_eq!(writer.as_slice(), expected);
        let mut reader = Reader::new(writer.as_slice());
        assert_eq!(reader.read_varlong().unwrap(), -1);
    }

    #[test]
    fn varint_rejects_six_byte_encodings() {
        let bytes = [0x80u8, 0x80, 0x80, 0x80, 0x80, 0x00];
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_varint(), Err(Error::VarIntTooLong));
    }

    #[test]
    fn string_round_trips_and_checks_limits() {
        let mut writer = Writer::new();
        writer.write_string("hello", 16).unwrap();
        assert_eq!(writer.as_slice(), b"\x05hello");

        let mut reader = Reader::new(writer.as_slice());
        assert_eq!(reader.read_string(16).unwrap(), "hello");

        let mut writer = Writer::new();
        assert!(writer.write_string("toolong", 3).is_err());

        let mut reader = Reader::new(b"\x02\xFF\xFE");
        assert_eq!(reader.read_string(16), Err(Error::InvalidUtf8));
    }

    #[test]
    fn string_checks_character_count_not_only_bytes() {
        // Three two-byte characters: six bytes, three chars.
        let mut writer = Writer::new();
        writer.write_string("ééé", 3).unwrap();
        let mut reader = Reader::new(writer.as_slice());
        assert_eq!(reader.read_string(3).unwrap(), "ééé");
        let mut reader = Reader::new(writer.as_slice());
        assert!(reader.read_string(2).is_err());
    }

    #[test]
    fn string_limits_count_utf16_code_units() {
        // "😀a" is five UTF-8 bytes but three UTF-16 code units.
        assert!(utf16_len("😀a") == 3);
        let mut writer = Writer::new();
        assert!(writer.write_string("😀a", 2).is_err());
        assert!(writer.write_string("😀a", 3).is_ok());

        let bytes = [5, 0xF0, 0x9F, 0x98, 0x80, b'a'];
        let mut reader = Reader::new(&bytes);
        assert!(reader.read_string(2).is_err());
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_string(3).unwrap(), "😀a");
    }

    #[test]
    fn uuid_round_trips() {
        let value = 0x0123_4567_89AB_CDEF_FEDC_BA98_7654_3210u128;
        let mut writer = Writer::new();
        writer.write_uuid(value);
        assert_eq!(writer.len(), 16);
        let mut reader = Reader::new(writer.as_slice());
        assert_eq!(reader.read_uuid().unwrap(), value);
    }

    #[test]
    fn optional_round_trips() {
        let mut writer = Writer::new();
        writer
            .write_optional(Some(&"x"), |writer, value| writer.write_string(value, 8))
            .unwrap();
        writer.write_optional::<&str>(None, |_, _| Ok(())).unwrap();
        let mut reader = Reader::new(writer.as_slice());
        assert_eq!(
            reader.read_optional(|r| r.read_string(8)).unwrap(),
            Some("x".to_owned())
        );
        assert_eq!(reader.read_optional(|r| r.read_string(8)).unwrap(), None);
    }

    #[test]
    fn finish_reports_trailing_bytes() {
        let mut reader = Reader::new(&[1, 2, 3]);
        assert_eq!(reader.read_u8().unwrap(), 1);
        assert_eq!(reader.finish(), Err(Error::TrailingBytes(2)));
    }
}
