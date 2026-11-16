//! Packet framing and zlib compression.
//!
//! A frame is `VarInt length` followed by the packet body (`VarInt id` plus
//! fields). Once compression is negotiated, the body is preceded by a VarInt
//! uncompressed length: `0` means the rest is uncompressed, otherwise the rest
//! is a zlib stream of exactly that many bytes.

use std::io::{Read, Write};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

use crate::codec::{Reader, Writer};
use crate::error::{Error, Result};

/// Frames are at most three VarInt bytes long (21 bits).
pub const MAX_FRAME: usize = (1 << 21) - 1;

/// Maximum declared uncompressed size, matching the 1.21.1 decoder.
pub const MAX_UNCOMPRESSED: usize = 2 * 1024 * 1024;

/// Encodes and decodes frames for one connection.
#[derive(Clone, Debug)]
pub struct FrameCodec {
    threshold: Option<usize>,
    max_frame: usize,
    max_uncompressed: usize,
}

impl Default for FrameCodec {
    fn default() -> Self {
        Self::uncompressed()
    }
}

impl FrameCodec {
    /// A codec before compression is negotiated.
    #[must_use]
    pub fn uncompressed() -> Self {
        Self {
            threshold: None,
            max_frame: MAX_FRAME,
            max_uncompressed: MAX_UNCOMPRESSED,
        }
    }

    /// A codec that compresses bodies of at least `threshold` bytes.
    #[must_use]
    pub fn with_threshold(threshold: usize) -> Self {
        Self {
            threshold: Some(threshold),
            max_frame: MAX_FRAME,
            max_uncompressed: MAX_UNCOMPRESSED,
        }
    }

    /// The current compression threshold, if compression is enabled.
    #[must_use]
    pub fn threshold(&self) -> Option<usize> {
        self.threshold
    }

    /// Enables or disables compression. `None` is the pre-login state; a
    /// negotiated threshold of `0` compresses every packet.
    pub fn set_threshold(&mut self, threshold: Option<usize>) {
        self.threshold = threshold;
    }

    /// Wraps one packet body in a frame.
    pub fn encode(&self, body: &[u8]) -> Result<Vec<u8>> {
        let contents = self.contents(body)?;
        let length = i32::try_from(contents.len()).map_err(|_| Error::PacketTooLarge {
            size: contents.len(),
            max: self.max_frame,
        })?;
        let mut frame = Writer::new();
        frame.write_varint(length);
        frame.write_bytes(&contents);
        Ok(frame.into_inner())
    }

    /// The bytes after the frame's length prefix.
    fn contents(&self, body: &[u8]) -> Result<Vec<u8>> {
        let contents = match self.threshold {
            None => body.to_vec(),
            Some(threshold) if body.len() < threshold => {
                let mut contents = Vec::with_capacity(body.len() + 1);
                contents.push(0);
                contents.extend_from_slice(body);
                contents
            }
            Some(_) => {
                if body.len() > self.max_uncompressed {
                    return Err(Error::PacketTooLarge {
                        size: body.len(),
                        max: self.max_uncompressed,
                    });
                }
                let compressed = deflate(body)?;
                let mut contents = Writer::new();
                let length = i32::try_from(body.len()).map_err(|_| Error::PacketTooLarge {
                    size: body.len(),
                    max: self.max_uncompressed,
                })?;
                contents.write_varint(length);
                contents.write_bytes(&compressed);
                contents.into_inner()
            }
        };
        if contents.len() > self.max_frame {
            return Err(Error::PacketTooLarge {
                size: contents.len(),
                max: self.max_frame,
            });
        }
        Ok(contents)
    }

    /// Decodes one complete frame, including its length prefix.
    pub fn decode(&self, frame: &[u8]) -> Result<Vec<u8>> {
        let mut reader = Reader::new(frame);
        let length = reader.read_varint()?;
        if length < 0 {
            return Err(Error::NegativeLength(length));
        }
        let length = length as usize;
        if length > self.max_frame {
            return Err(Error::PacketTooLarge {
                size: length,
                max: self.max_frame,
            });
        }
        if length != reader.remaining() {
            return Err(Error::protocol(format!(
                "frame declares {length} bytes but carries {}",
                reader.remaining()
            )));
        }
        self.decode_contents(reader.read_rest())
    }

    /// Reads one frame from a stream.
    pub fn read_frame(&self, stream: &mut impl Read) -> Result<Vec<u8>> {
        let length = read_varint(stream)?;
        if length < 0 {
            return Err(Error::NegativeLength(length));
        }
        let length = length as usize;
        if length > self.max_frame {
            return Err(Error::PacketTooLarge {
                size: length,
                max: self.max_frame,
            });
        }
        let mut contents = vec![0u8; length];
        stream.read_exact(&mut contents)?;
        self.decode_contents(&contents)
    }

    /// Writes one framed packet body to a stream.
    pub fn write_frame(&self, stream: &mut impl Write, body: &[u8]) -> Result<()> {
        let frame = self.encode(body)?;
        stream.write_all(&frame)?;
        Ok(())
    }

    fn decode_contents(&self, contents: &[u8]) -> Result<Vec<u8>> {
        if contents.len() > self.max_frame {
            return Err(Error::PacketTooLarge {
                size: contents.len(),
                max: self.max_frame,
            });
        }
        match self.threshold {
            None => Ok(contents.to_vec()),
            Some(threshold) => {
                let mut reader = Reader::new(contents);
                let declared = reader.read_varint()?;
                if declared < 0 {
                    return Err(Error::NegativeLength(declared));
                }
                let declared = declared as usize;
                if declared == 0 {
                    return Ok(reader.read_rest().to_vec());
                }
                if declared < threshold {
                    return Err(Error::compression(format!(
                        "declared size {declared} is below the threshold {threshold}"
                    )));
                }
                if declared > self.max_uncompressed {
                    return Err(Error::PacketTooLarge {
                        size: declared,
                        max: self.max_uncompressed,
                    });
                }
                inflate(reader.read_rest(), declared)
            }
        }
    }
}

fn deflate(body: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(body)
        .map_err(|error| Error::compression(error.to_string()))?;
    encoder
        .finish()
        .map_err(|error| Error::compression(error.to_string()))
}

fn inflate(compressed: &[u8], declared: usize) -> Result<Vec<u8>> {
    let decoder = ZlibDecoder::new(compressed);
    let limit = u64::try_from(declared)
        .map_err(|_| Error::PacketTooLarge {
            size: declared,
            max: MAX_UNCOMPRESSED,
        })?
        .saturating_add(1);
    let mut body = Vec::with_capacity(declared.min(64 * 1024));
    decoder
        .take(limit)
        .read_to_end(&mut body)
        .map_err(|error| Error::compression(error.to_string()))?;
    if body.len() != declared {
        return Err(Error::compression(format!(
            "declared {declared} bytes but inflated {}",
            body.len()
        )));
    }
    Ok(body)
}

fn read_varint(stream: &mut impl Read) -> Result<i32> {
    let mut value = 0u32;
    for index in 0..5 {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte)?;
        value |= u32::from(byte[0] & 0x7F) << (7 * index);
        if byte[0] & 0x80 == 0 {
            return Ok(value as i32);
        }
    }
    Err(Error::VarIntTooLong)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn uncompressed_round_trip() {
        let codec = FrameCodec::uncompressed();
        let body = b"\x00hello";
        let frame = codec.encode(body).unwrap();
        assert_eq!(frame[0], body.len() as u8);
        assert_eq!(codec.decode(&frame).unwrap(), body);
    }

    #[test]
    fn small_packets_stay_uncompressed() {
        let codec = FrameCodec::with_threshold(64);
        let body = b"\x00short";
        let frame = codec.encode(body).unwrap();
        assert_eq!(frame[1], 0, "compression marker");
        assert_eq!(&frame[2..], body);
        assert_eq!(codec.decode(&frame).unwrap(), body);
    }

    #[test]
    fn large_packets_compress_and_round_trip() {
        let codec = FrameCodec::with_threshold(16);
        let body = vec![0x5Au8; 1000];
        let frame = codec.encode(&body).unwrap();
        assert_ne!(frame[1], 0, "expected a compressed body");
        assert_eq!(codec.decode(&frame).unwrap(), body);
    }

    #[test]
    fn threshold_zero_compresses_everything() {
        let codec = FrameCodec::with_threshold(0);
        let body = b"\x00";
        let frame = codec.encode(body).unwrap();
        assert_ne!(frame[1], 0);
        assert_eq!(codec.decode(&frame).unwrap(), body);
    }

    #[test]
    fn declared_size_below_threshold_is_rejected() {
        let codec = FrameCodec::with_threshold(100);
        let mut contents = Writer::new();
        contents.write_varint(50);
        contents.write_bytes(&[0; 8]);
        let contents = contents.into_inner();
        let mut frame = Writer::new();
        frame.write_varint(contents.len() as i32);
        frame.write_bytes(&contents);
        assert!(matches!(
            codec.decode(&frame.into_inner()),
            Err(Error::Compression { .. })
        ));
    }

    #[test]
    fn stream_read_and_write_round_trip() {
        let codec = FrameCodec::with_threshold(8);
        let body = vec![7u8; 100];
        let mut stream = Vec::new();
        codec.write_frame(&mut stream, &body).unwrap();
        let mut cursor = Cursor::new(stream);
        assert_eq!(codec.read_frame(&mut cursor).unwrap(), body);
    }

    #[test]
    fn oversized_body_is_rejected() {
        let codec = FrameCodec::uncompressed();
        let body = vec![0u8; MAX_FRAME + 1];
        assert!(matches!(
            codec.encode(&body),
            Err(Error::PacketTooLarge { .. })
        ));
    }

    #[test]
    fn truncated_frame_is_rejected() {
        let codec = FrameCodec::uncompressed();
        assert!(matches!(
            codec.decode(b"\x05\x00"),
            Err(Error::Protocol { .. })
        ));
    }
}
