//! Java Edition NBT, the binary format behind `level.dat`, chunks, and
//! entities.
//!
//! The reader is strict on purpose: it rejects truncated input, unknown tag
//! types, duplicate compound keys, invalid modified UTF-8, and data that
//! exceeds explicit limits. Minecraft strings use Java's modified UTF-8 (NUL
//! is `C0 80` and supplementary characters are surrogate pairs), not the
//! standard UTF-8 used by Rust, so this module implements both directions.
//!
//! Limits protect a phone from a malformed or hostile save: nesting depth,
//! individual string and collection sizes, and the total data accounted for.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};

use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};
use flate2::Compression as FlateCompression;

pub const TAG_END: u8 = 0;
pub const TAG_BYTE: u8 = 1;
pub const TAG_SHORT: u8 = 2;
pub const TAG_INT: u8 = 3;
pub const TAG_LONG: u8 = 4;
pub const TAG_FLOAT: u8 = 5;
pub const TAG_DOUBLE: u8 = 6;
pub const TAG_BYTE_ARRAY: u8 = 7;
pub const TAG_STRING: u8 = 8;
pub const TAG_LIST: u8 = 9;
pub const TAG_COMPOUND: u8 = 10;
pub const TAG_INT_ARRAY: u8 = 11;
pub const TAG_LONG_ARRAY: u8 = 12;

/// One NBT value. Compounds keep their keys sorted so writing the same tree
/// twice produces identical bytes.
#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    /// Only appears as the element type of an empty list or as a compound
    /// terminator; never as a parsed value.
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Tag>),
    Compound(BTreeMap<String, Tag>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    /// The numeric tag id written before a payload.
    pub const fn tag_id(&self) -> u8 {
        match self {
            Self::End => TAG_END,
            Self::Byte(_) => TAG_BYTE,
            Self::Short(_) => TAG_SHORT,
            Self::Int(_) => TAG_INT,
            Self::Long(_) => TAG_LONG,
            Self::Float(_) => TAG_FLOAT,
            Self::Double(_) => TAG_DOUBLE,
            Self::ByteArray(_) => TAG_BYTE_ARRAY,
            Self::String(_) => TAG_STRING,
            Self::List(_) => TAG_LIST,
            Self::Compound(_) => TAG_COMPOUND,
            Self::IntArray(_) => TAG_INT_ARRAY,
            Self::LongArray(_) => TAG_LONG_ARRAY,
        }
    }

    /// Reads a field from a compound, or `None` for other tags.
    pub fn get(&self, key: &str) -> Option<&Tag> {
        self.as_compound().and_then(|map| map.get(key))
    }

    /// Mutable field access, for callers composing synthetic saves.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Tag> {
        self.as_compound_mut().and_then(|map| map.get_mut(key))
    }

    pub fn as_compound(&self) -> Option<&BTreeMap<String, Tag>> {
        match self {
            Self::Compound(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_compound_mut(&mut self) -> Option<&mut BTreeMap<String, Tag>> {
        match self {
            Self::Compound(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_byte(&self) -> Option<i8> {
        match self {
            Self::Byte(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i32> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_long(&self) -> Option<i64> {
        match self {
            Self::Long(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_long_array(&self) -> Option<&[i64]> {
        match self {
            Self::LongArray(values) => Some(values),
            _ => None,
        }
    }
}

/// Resource limits applied while reading or writing NBT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Maximum compound/list nesting. Vanilla chunks are far below this.
    pub max_depth: usize,
    /// Maximum encoded length of one string. The wire format uses a `u16`
    /// length, so values above 65535 cannot be represented anyway.
    pub max_string_bytes: usize,
    /// Maximum element count for a list or an array.
    pub max_collection_len: usize,
    /// Approximate budget for decoded strings, arrays, and list elements. Kept
    /// generous enough for a 384-block-tall chunk while still bounding a
    /// malicious file.
    pub max_total_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 512,
            max_string_bytes: u16::MAX as usize,
            max_collection_len: 1 << 24,
            max_total_bytes: 64 << 20,
        }
    }
}

/// Failures while reading or writing NBT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NbtError {
    UnexpectedEof {
        offset: usize,
        needed: usize,
        available: usize,
    },
    InvalidTagType {
        tag: u8,
        offset: usize,
    },
    InvalidModifiedUtf8 {
        offset: usize,
        reason: &'static str,
    },
    StringTooLong {
        len: usize,
    },
    NegativeLength {
        kind: &'static str,
        value: i32,
    },
    TooDeep {
        depth: usize,
    },
    TooLarge {
        what: &'static str,
        len: usize,
    },
    DuplicateKey {
        key: String,
    },
    EmptyListElement {
        len: usize,
    },
    HeterogeneousList,
    RootIsEnd,
    TrailingData {
        offset: usize,
    },
    Io {
        context: &'static str,
        message: String,
    },
}

impl NbtError {
    fn io(context: &'static str, error: std::io::Error) -> Self {
        Self::Io {
            context,
            message: error.to_string(),
        }
    }
}

impl fmt::Display for NbtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof {
                offset,
                needed,
                available,
            } => write!(
                f,
                "unexpected end of NBT at byte {offset}: needed {needed} bytes, {available} available"
            ),
            Self::InvalidTagType { tag, offset } => {
                write!(f, "invalid NBT tag type {tag} at byte {offset}")
            }
            Self::InvalidModifiedUtf8 { offset, reason } => {
                write!(f, "invalid modified UTF-8 at byte {offset}: {reason}")
            }
            Self::StringTooLong { len } => {
                write!(f, "NBT string is {len} bytes; the limit is 65535")
            }
            Self::NegativeLength { kind, value } => {
                write!(f, "negative {kind} length {value}")
            }
            Self::TooDeep { depth } => write!(f, "NBT nesting deeper than the limit ({depth})"),
            Self::TooLarge { what, len } => {
                write!(f, "{what} is too large ({len} bytes or elements)")
            }
            Self::DuplicateKey { key } => write!(f, "duplicate compound key {key:?}"),
            Self::EmptyListElement { len } => {
                write!(f, "list of {len} elements has the End tag type")
            }
            Self::HeterogeneousList => write!(f, "NBT lists must contain one tag type"),
            Self::RootIsEnd => write!(f, "root NBT tag is End"),
            Self::TrailingData { offset } => {
                write!(f, "unexpected trailing NBT data after byte {offset}")
            }
            Self::Io { context, message } => write!(f, "{context}: {message}"),
        }
    }
}

impl std::error::Error for NbtError {}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    limits: Limits,
    used: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], limits: Limits) -> Self {
        Self {
            data,
            pos: 0,
            limits,
            used: 0,
        }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], NbtError> {
        let end = self.pos.checked_add(len).ok_or(NbtError::TooLarge {
            what: "NBT field",
            len,
        })?;
        if end > self.data.len() {
            return Err(NbtError::UnexpectedEof {
                offset: self.pos,
                needed: len,
                available: self.data.len() - self.pos,
            });
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, NbtError> {
        Ok(self.take(1)?[0])
    }

    fn i8(&mut self) -> Result<i8, NbtError> {
        Ok(self.u8()? as i8)
    }

    fn u16(&mut self) -> Result<u16, NbtError> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn i16(&mut self) -> Result<i16, NbtError> {
        Ok(self.u16()? as i16)
    }

    fn i32(&mut self) -> Result<i32, NbtError> {
        let bytes = self.take(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i64(&mut self) -> Result<i64, NbtError> {
        let bytes = self.take(8)?;
        Ok(i64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn f32(&mut self) -> Result<f32, NbtError> {
        Ok(f32::from_bits(self.i32()? as u32))
    }

    fn f64(&mut self) -> Result<f64, NbtError> {
        Ok(f64::from_bits(self.i64()? as u64))
    }

    fn charge(&mut self, bytes: usize) -> Result<(), NbtError> {
        self.used = self.used.saturating_add(bytes);
        if self.used > self.limits.max_total_bytes {
            return Err(NbtError::TooLarge {
                what: "decoded NBT",
                len: self.used,
            });
        }
        Ok(())
    }

    fn read_len(&mut self, kind: &'static str) -> Result<usize, NbtError> {
        let value = self.i32()?;
        if value < 0 {
            return Err(NbtError::NegativeLength { kind, value });
        }
        let len = value as usize;
        if len > self.limits.max_collection_len {
            return Err(NbtError::TooLarge { what: kind, len });
        }
        Ok(len)
    }

    fn read_string(&mut self) -> Result<String, NbtError> {
        let offset = self.pos;
        let len = self.u16()? as usize;
        if len > self.limits.max_string_bytes {
            return Err(NbtError::StringTooLong { len });
        }
        self.charge(len)?;
        let bytes = self.take(len)?;
        decode_modified_utf8(bytes, offset)
    }

    fn read_payload(&mut self, tag: u8, depth: usize) -> Result<Tag, NbtError> {
        if depth > self.limits.max_depth {
            return Err(NbtError::TooDeep { depth });
        }
        match tag {
            TAG_BYTE => Ok(Tag::Byte(self.i8()?)),
            TAG_SHORT => Ok(Tag::Short(self.i16()?)),
            TAG_INT => Ok(Tag::Int(self.i32()?)),
            TAG_LONG => Ok(Tag::Long(self.i64()?)),
            TAG_FLOAT => Ok(Tag::Float(self.f32()?)),
            TAG_DOUBLE => Ok(Tag::Double(self.f64()?)),
            TAG_BYTE_ARRAY => {
                let len = self.read_len("byte array")?;
                self.charge(len)?;
                Ok(Tag::ByteArray(self.take(len)?.to_vec()))
            }
            TAG_STRING => Ok(Tag::String(self.read_string()?)),
            TAG_LIST => self.read_list(depth),
            TAG_COMPOUND => self.read_compound(depth),
            TAG_INT_ARRAY => {
                let len = self.read_len("int array")?;
                let bytes = len.checked_mul(4).ok_or(NbtError::TooLarge {
                    what: "int array",
                    len,
                })?;
                self.charge(bytes)?;
                let data = self.take(bytes)?;
                Ok(Tag::IntArray(
                    data.chunks_exact(4)
                        .map(|chunk| i32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                        .collect(),
                ))
            }
            TAG_LONG_ARRAY => {
                let len = self.read_len("long array")?;
                let bytes = len.checked_mul(8).ok_or(NbtError::TooLarge {
                    what: "long array",
                    len,
                })?;
                self.charge(bytes)?;
                let data = self.take(bytes)?;
                Ok(Tag::LongArray(
                    data.chunks_exact(8)
                        .map(|chunk| {
                            i64::from_be_bytes([
                                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5],
                                chunk[6], chunk[7],
                            ])
                        })
                        .collect(),
                ))
            }
            _ => Err(NbtError::InvalidTagType {
                tag,
                offset: self.pos.saturating_sub(1),
            }),
        }
    }

    fn read_list(&mut self, depth: usize) -> Result<Tag, NbtError> {
        let element = self.u8()?;
        let len = self.read_len("list")?;
        if len > 0 && element == TAG_END {
            return Err(NbtError::EmptyListElement { len });
        }
        let charge = len
            .checked_mul(32)
            .ok_or(NbtError::TooLarge { what: "list", len })?;
        self.charge(charge)?;
        let mut items = Vec::with_capacity(len.min(4096));
        for _ in 0..len {
            items.push(self.read_payload(element, depth + 1)?);
        }
        Ok(Tag::List(items))
    }

    fn read_compound(&mut self, depth: usize) -> Result<Tag, NbtError> {
        let mut map = BTreeMap::new();
        loop {
            let tag = self.u8()?;
            if tag == TAG_END {
                break;
            }
            let name = self.read_string()?;
            let value = self.read_payload(tag, depth + 1)?;
            if map.insert(name.clone(), value).is_some() {
                return Err(NbtError::DuplicateKey { key: name });
            }
        }
        Ok(Tag::Compound(map))
    }
}

struct Writer {
    out: Vec<u8>,
    limits: Limits,
}

impl Writer {
    fn write_string(&mut self, value: &str) -> Result<(), NbtError> {
        let bytes = encode_modified_utf8(value);
        if bytes.len() > u16::MAX as usize || bytes.len() > self.limits.max_string_bytes {
            return Err(NbtError::StringTooLong { len: bytes.len() });
        }
        self.out
            .extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        self.out.extend_from_slice(&bytes);
        Ok(())
    }

    fn write_payload(&mut self, tag: &Tag, depth: usize) -> Result<(), NbtError> {
        if depth > self.limits.max_depth {
            return Err(NbtError::TooDeep { depth });
        }
        match tag {
            Tag::End => Err(NbtError::RootIsEnd),
            Tag::Byte(value) => {
                self.out.push(*value as u8);
                Ok(())
            }
            Tag::Short(value) => {
                self.out.extend_from_slice(&value.to_be_bytes());
                Ok(())
            }
            Tag::Int(value) => {
                self.out.extend_from_slice(&value.to_be_bytes());
                Ok(())
            }
            Tag::Long(value) => {
                self.out.extend_from_slice(&value.to_be_bytes());
                Ok(())
            }
            Tag::Float(value) => {
                self.out.extend_from_slice(&value.to_bits().to_be_bytes());
                Ok(())
            }
            Tag::Double(value) => {
                self.out.extend_from_slice(&value.to_bits().to_be_bytes());
                Ok(())
            }
            Tag::ByteArray(values) => {
                self.check_len("byte array", values.len())?;
                self.out
                    .extend_from_slice(&(values.len() as i32).to_be_bytes());
                self.out.extend_from_slice(values);
                Ok(())
            }
            Tag::String(value) => self.write_string(value),
            Tag::List(items) => {
                self.check_len("list", items.len())?;
                let element = items.first().map_or(TAG_END, Tag::tag_id);
                if items.iter().any(|item| item.tag_id() != element) {
                    return Err(NbtError::HeterogeneousList);
                }
                self.out.push(element);
                self.out
                    .extend_from_slice(&(items.len() as i32).to_be_bytes());
                for item in items {
                    self.write_payload(item, depth + 1)?;
                }
                Ok(())
            }
            Tag::Compound(map) => {
                for (key, value) in map {
                    self.out.push(value.tag_id());
                    self.write_string(key)?;
                    self.write_payload(value, depth + 1)?;
                }
                self.out.push(TAG_END);
                Ok(())
            }
            Tag::IntArray(values) => {
                self.check_len("int array", values.len())?;
                self.out
                    .extend_from_slice(&(values.len() as i32).to_be_bytes());
                for value in values {
                    self.out.extend_from_slice(&value.to_be_bytes());
                }
                Ok(())
            }
            Tag::LongArray(values) => {
                self.check_len("long array", values.len())?;
                self.out
                    .extend_from_slice(&(values.len() as i32).to_be_bytes());
                for value in values {
                    self.out.extend_from_slice(&value.to_be_bytes());
                }
                Ok(())
            }
        }
    }

    fn check_len(&self, kind: &'static str, len: usize) -> Result<(), NbtError> {
        if len > i32::MAX as usize || len > self.limits.max_collection_len {
            return Err(NbtError::TooLarge { what: kind, len });
        }
        Ok(())
    }
}

/// Java's `DataInput.readUTF`/`DataOutput.writeUTF` encoding.
fn decode_modified_utf8(bytes: &[u8], base: usize) -> Result<String, NbtError> {
    let mut units = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let first = bytes[index];
        if first < 0x80 {
            units.push(u16::from(first));
            index += 1;
        } else if first & 0xE0 == 0xC0 {
            let Some(&second) = bytes.get(index + 1) else {
                return Err(NbtError::InvalidModifiedUtf8 {
                    offset: base + index,
                    reason: "truncated two-byte sequence",
                });
            };
            if second & 0xC0 != 0x80 {
                return Err(NbtError::InvalidModifiedUtf8 {
                    offset: base + index,
                    reason: "invalid continuation byte",
                });
            }
            let value = (u16::from(first & 0x1F) << 6) | u16::from(second & 0x3F);
            if value < 0x80 && !(first == 0xC0 && second == 0x80) {
                return Err(NbtError::InvalidModifiedUtf8 {
                    offset: base + index,
                    reason: "overlong two-byte sequence",
                });
            }
            units.push(value);
            index += 2;
        } else if first & 0xF0 == 0xE0 {
            let (Some(&second), Some(&third)) = (bytes.get(index + 1), bytes.get(index + 2)) else {
                return Err(NbtError::InvalidModifiedUtf8 {
                    offset: base + index,
                    reason: "truncated three-byte sequence",
                });
            };
            if second & 0xC0 != 0x80 || third & 0xC0 != 0x80 {
                return Err(NbtError::InvalidModifiedUtf8 {
                    offset: base + index,
                    reason: "invalid continuation byte",
                });
            }
            let value = (u16::from(first & 0x0F) << 12)
                | (u16::from(second & 0x3F) << 6)
                | u16::from(third & 0x3F);
            if value < 0x800 {
                return Err(NbtError::InvalidModifiedUtf8 {
                    offset: base + index,
                    reason: "overlong three-byte sequence",
                });
            }
            units.push(value);
            index += 3;
        } else {
            return Err(NbtError::InvalidModifiedUtf8 {
                offset: base + index,
                reason: "invalid leading byte",
            });
        }
    }
    String::from_utf16(&units).map_err(|_| NbtError::InvalidModifiedUtf8 {
        offset: base,
        reason: "unpaired UTF-16 surrogate",
    })
}

fn encode_modified_utf8(value: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    for character in value.chars() {
        let code = character as u32;
        if code <= 0xFFFF {
            encode_unit(code as u16, &mut out);
        } else {
            let offset = code - 0x1_0000;
            encode_unit(0xD800 + (offset >> 10) as u16, &mut out);
            encode_unit(0xDC00 + (offset & 0x3FF) as u16, &mut out);
        }
    }
    out
}

fn encode_unit(unit: u16, out: &mut Vec<u8>) {
    if unit == 0 {
        out.extend_from_slice(&[0xC0, 0x80]);
    } else if unit < 0x80 {
        out.push(unit as u8);
    } else if unit < 0x800 {
        out.push(0xC0 | (unit >> 6) as u8);
        out.push(0x80 | (unit & 0x3F) as u8);
    } else {
        out.push(0xE0 | (unit >> 12) as u8);
        out.push(0x80 | ((unit >> 6) & 0x3F) as u8);
        out.push(0x80 | (unit & 0x3F) as u8);
    }
}

fn decompress(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, NbtError> {
    let mut out = Vec::new();
    reader
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|error| NbtError::io("decompress NBT", error))?;
    if out.len() > limit {
        return Err(NbtError::TooLarge {
            what: "decompressed NBT",
            len: out.len(),
        });
    }
    Ok(out)
}

fn compress(tag: &Tag, limits: Limits, zlib: bool) -> Result<Vec<u8>, NbtError> {
    let raw = Nbt::write_with_limits(tag, limits)?;
    let mut encoded = Vec::new();
    if zlib {
        let mut encoder = ZlibEncoder::new(&mut encoded, FlateCompression::default());
        encoder
            .write_all(&raw)
            .map_err(|error| NbtError::io("compress NBT with zlib", error))?;
        encoder
            .finish()
            .map_err(|error| NbtError::io("finish zlib NBT", error))?;
    } else {
        let mut encoder = GzEncoder::new(&mut encoded, FlateCompression::default());
        encoder
            .write_all(&raw)
            .map_err(|error| NbtError::io("compress NBT with gzip", error))?;
        encoder
            .finish()
            .map_err(|error| NbtError::io("finish gzip NBT", error))?;
    }
    Ok(encoded)
}

/// NBT entry points. All functions treat the outermost tag as a named tag with
/// an empty name, which is the form used by `level.dat`, chunks, and entities.
#[derive(Clone, Copy, Debug, Default)]
pub struct Nbt;

impl Nbt {
    /// Reads an uncompressed named tag with default limits.
    pub fn read(bytes: &[u8]) -> Result<Tag, NbtError> {
        Self::read_with_limits(bytes, Limits::default())
    }

    /// Reads an uncompressed named tag with explicit limits.
    pub fn read_with_limits(bytes: &[u8], limits: Limits) -> Result<Tag, NbtError> {
        let mut reader = Reader::new(bytes, limits);
        let tag = reader.u8()?;
        if tag == TAG_END {
            return Err(NbtError::RootIsEnd);
        }
        let _name = reader.read_string()?;
        let root = reader.read_payload(tag, 0)?;
        if reader.pos != bytes.len() {
            return Err(NbtError::TrailingData { offset: reader.pos });
        }
        Ok(root)
    }

    /// Reads an uncompressed **anonymous** tag with default limits.
    ///
    /// Network NBT (`1.20.2+` packet components, registry entries, and
    /// disconnect reasons) omits the root name that files carry. The payload is
    /// otherwise identical, so this shares the same strict reader.
    pub fn read_anonymous(bytes: &[u8]) -> Result<Tag, NbtError> {
        Self::read_anonymous_with_limits(bytes, Limits::default())
    }

    /// Reads an uncompressed anonymous tag with explicit limits.
    pub fn read_anonymous_with_limits(bytes: &[u8], limits: Limits) -> Result<Tag, NbtError> {
        let mut reader = Reader::new(bytes, limits);
        let tag = reader.u8()?;
        if tag == TAG_END {
            return Err(NbtError::RootIsEnd);
        }
        let root = reader.read_payload(tag, 0)?;
        if reader.pos != bytes.len() {
            return Err(NbtError::TrailingData { offset: reader.pos });
        }
        Ok(root)
    }

    /// Reads a gzip-compressed named tag with default limits.
    pub fn read_gzip(bytes: &[u8]) -> Result<Tag, NbtError> {
        Self::read_gzip_with_limits(bytes, Limits::default())
    }

    /// Reads a gzip-compressed named tag with explicit limits.
    pub fn read_gzip_with_limits(bytes: &[u8], limits: Limits) -> Result<Tag, NbtError> {
        let raw = decompress(GzDecoder::new(bytes), limits.max_total_bytes)?;
        Self::read_with_limits(&raw, limits)
    }

    /// Reads a zlib-compressed named tag with default limits.
    pub fn read_zlib(bytes: &[u8]) -> Result<Tag, NbtError> {
        Self::read_zlib_with_limits(bytes, Limits::default())
    }

    /// Reads a zlib-compressed named tag with explicit limits.
    pub fn read_zlib_with_limits(bytes: &[u8], limits: Limits) -> Result<Tag, NbtError> {
        let raw = decompress(ZlibDecoder::new(bytes), limits.max_total_bytes)?;
        Self::read_with_limits(&raw, limits)
    }

    /// Detects gzip or zlib from the first bytes and falls back to raw NBT.
    pub fn read_auto(bytes: &[u8]) -> Result<Tag, NbtError> {
        Self::read_auto_with_limits(bytes, Limits::default())
    }

    /// Detects the compression of a named tag with explicit limits.
    pub fn read_auto_with_limits(bytes: &[u8], limits: Limits) -> Result<Tag, NbtError> {
        if bytes.starts_with(&[0x1F, 0x8B]) {
            Self::read_gzip_with_limits(bytes, limits)
        } else if is_zlib(bytes) {
            Self::read_zlib_with_limits(bytes, limits)
        } else {
            Self::read_with_limits(bytes, limits)
        }
    }

    /// Writes an uncompressed named tag with default limits.
    pub fn write(tag: &Tag) -> Result<Vec<u8>, NbtError> {
        Self::write_with_limits(tag, Limits::default())
    }

    /// Writes an uncompressed named tag with explicit limits.
    pub fn write_with_limits(tag: &Tag, limits: Limits) -> Result<Vec<u8>, NbtError> {
        if tag.tag_id() == TAG_END {
            return Err(NbtError::RootIsEnd);
        }
        let mut writer = Writer {
            out: Vec::new(),
            limits,
        };
        writer.out.push(tag.tag_id());
        writer.write_string("")?;
        writer.write_payload(tag, 0)?;
        Ok(writer.out)
    }

    /// Writes a gzip-compressed named tag with default limits.
    pub fn write_gzip(tag: &Tag) -> Result<Vec<u8>, NbtError> {
        compress(tag, Limits::default(), false)
    }

    /// Writes a zlib-compressed named tag with default limits.
    pub fn write_zlib(tag: &Tag) -> Result<Vec<u8>, NbtError> {
        compress(tag, Limits::default(), true)
    }
}

fn is_zlib(bytes: &[u8]) -> bool {
    if bytes.len() < 2 || bytes[0] & 0x0F != 8 {
        return false;
    }
    let header = (u16::from(bytes[0]) << 8) | u16::from(bytes[1]);
    header % 31 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tag {
        let mut properties = BTreeMap::new();
        properties.insert("axis".to_string(), Tag::String("y".to_string()));
        let mut block = BTreeMap::new();
        block.insert(
            "Name".to_string(),
            Tag::String("minecraft:oak_log".to_string()),
        );
        block.insert("Properties".to_string(), Tag::Compound(properties));
        let mut root = BTreeMap::new();
        root.insert("DataVersion".to_string(), Tag::Int(3955));
        root.insert("Name".to_string(), Tag::String("synthetic".to_string()));
        root.insert("Byte".to_string(), Tag::Byte(-1));
        root.insert("Short".to_string(), Tag::Short(300));
        root.insert("Int".to_string(), Tag::Int(-70_000));
        root.insert("Long".to_string(), Tag::Long(9_000_000_000));
        root.insert("Float".to_string(), Tag::Float(1.5));
        root.insert("Double".to_string(), Tag::Double(-2.25));
        root.insert("Bytes".to_string(), Tag::ByteArray(vec![0, 127, 255]));
        root.insert(
            "Ints".to_string(),
            Tag::IntArray(vec![i32::MIN, 0, i32::MAX]),
        );
        root.insert(
            "Longs".to_string(),
            Tag::LongArray(vec![i64::MIN, 0, i64::MAX]),
        );
        root.insert("Blocks".to_string(), Tag::List(vec![Tag::Compound(block)]));
        Tag::Compound(root)
    }

    #[test]
    fn round_trip_all_tag_types() {
        let tag = sample();
        let bytes = Nbt::write(&tag).expect("write");
        assert_eq!(Nbt::read(&bytes).expect("read"), tag);
    }

    #[test]
    fn anonymous_network_nbt_has_no_root_name() {
        let tag = sample();
        let named = Nbt::write(&tag).expect("write");
        // tag id, two-byte empty name, payload
        assert_eq!(&named[1..3], &[0, 0]);
        let mut anonymous = Vec::with_capacity(named.len() - 2);
        anonymous.push(named[0]);
        anonymous.extend_from_slice(&named[3..]);
        assert_eq!(Nbt::read_anonymous(&anonymous).expect("read"), tag);

        // A named tag is not a valid anonymous tag because its name bytes are
        // read as payload.
        assert!(Nbt::read_anonymous(&named).is_err());
        assert!(Nbt::read_anonymous(&[]).is_err());
        assert!(Nbt::read_anonymous(&[TAG_END]).is_err());
    }

    #[test]
    fn round_trip_gzip_and_zlib() {
        let tag = sample();
        let gzip = Nbt::write_gzip(&tag).expect("gzip write");
        assert!(gzip.starts_with(&[0x1F, 0x8B]));
        assert_eq!(Nbt::read_gzip(&gzip).expect("gzip read"), tag);
        assert_eq!(Nbt::read_auto(&gzip).expect("auto gzip"), tag);

        let zlib = Nbt::write_zlib(&tag).expect("zlib write");
        assert!(is_zlib(&zlib));
        assert_eq!(Nbt::read_zlib(&zlib).expect("zlib read"), tag);
        assert_eq!(Nbt::read_auto(&zlib).expect("auto zlib"), tag);

        let raw = Nbt::write(&tag).expect("raw write");
        assert_eq!(Nbt::read_auto(&raw).expect("auto raw"), tag);
    }

    #[test]
    fn modified_utf8_handles_nul_and_supplementary_characters() {
        let text = "a\u{0}b\u{7FF}\u{800}\u{FFFF}\u{1F600}\u{10FFFF}é";
        let mut root = BTreeMap::new();
        root.insert("Text".to_string(), Tag::String(text.to_string()));
        let tag = Tag::Compound(root);
        let bytes = Nbt::write(&tag).expect("write");
        // NUL is encoded as C0 80, never as a literal zero byte.
        assert!(bytes.windows(2).any(|pair| pair == [0xC0, 0x80]));
        assert_eq!(Nbt::read(&bytes).expect("read"), tag);
    }

    #[test]
    fn rejects_malformed_modified_utf8() {
        let cases: [&[u8]; 4] = [
            &[0xC3],
            &[0xC1, 0x81],
            &[0xE0, 0x80, 0x80],
            &[0xED, 0xA0, 0x80],
        ];
        for bytes in cases {
            assert!(
                decode_modified_utf8(bytes, 0).is_err(),
                "accepted invalid bytes {bytes:02X?}"
            );
        }
    }

    #[test]
    fn rejects_truncated_data() {
        let bytes = Nbt::write(&sample()).expect("write");
        for len in 0..bytes.len() {
            assert!(
                Nbt::read(&bytes[..len]).is_err(),
                "accepted truncated input of {len} bytes"
            );
        }
    }

    #[test]
    fn rejects_trailing_and_duplicate_data() {
        let mut bytes = Nbt::write(&sample()).expect("write");
        bytes.push(0);
        assert!(matches!(
            Nbt::read(&bytes),
            Err(NbtError::TrailingData { .. })
        ));

        // A compound with the same key twice: 10 00 00 03 00 01 61 00 00 00 01 ... 00
        let mut duplicate = vec![TAG_COMPOUND, 0, 0];
        duplicate.extend_from_slice(&[TAG_INT, 0, 1, b'a', 0, 0, 0, 1]);
        duplicate.extend_from_slice(&[TAG_INT, 0, 1, b'a', 0, 0, 0, 2, TAG_END]);
        assert!(matches!(
            Nbt::read(&duplicate),
            Err(NbtError::DuplicateKey { .. })
        ));
    }

    #[test]
    fn enforces_limits() {
        let mut root = BTreeMap::new();
        root.insert("Value".to_string(), Tag::String("abcd".to_string()));
        let tag = Tag::Compound(root);
        let bytes = Nbt::write(&tag).expect("write");
        let limits = Limits {
            max_total_bytes: 3,
            ..Limits::default()
        };
        assert!(Nbt::read_with_limits(&bytes, limits).is_err());

        let deep = Tag::List(vec![Tag::List(vec![Tag::List(vec![Tag::Int(1)])])]);
        let deep_bytes = Nbt::write(&deep).expect("write deep");
        let shallow = Limits {
            max_depth: 1,
            ..Limits::default()
        };
        assert!(matches!(
            Nbt::read_with_limits(&deep_bytes, shallow),
            Err(NbtError::TooDeep { .. })
        ));
    }

    #[test]
    fn writer_rejects_invalid_trees() {
        let mixed = Tag::List(vec![Tag::Int(1), Tag::Long(2)]);
        assert_eq!(Nbt::write(&mixed), Err(NbtError::HeterogeneousList));

        let too_long = Tag::String("x".repeat(u16::MAX as usize + 1));
        assert!(matches!(
            Nbt::write(&too_long),
            Err(NbtError::StringTooLong { .. })
        ));

        assert_eq!(Nbt::write(&Tag::End), Err(NbtError::RootIsEnd));
    }

    #[test]
    fn empty_list_round_trips_with_end_type() {
        let tag = Tag::List(Vec::new());
        let bytes = Nbt::write(&tag).expect("write");
        assert_eq!(Nbt::read(&bytes).expect("read"), tag);
    }
}
