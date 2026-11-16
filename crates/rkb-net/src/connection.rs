//! A framed Minecraft connection, including encryption and compression.
//!
//! After the login key exchange, the **entire frame** — the VarInt length and
//! its contents — is encrypted with AES/CFB8. Compression sits inside the
//! frame, so the order is:
//!
//! - write: packet body → zlib frame contents → length prefix → encrypt whole
//!   frame → socket;
//! - read: socket → decrypt whole frame → length prefix → inflate contents →
//!   packet body.
//!
//! Each direction has its own cipher state, even though both start from the
//! same 16-byte secret (key and IV).

use std::io::{Read, Write};

use rkb_proto::crypto::Aes128Cfb8;
use rkb_proto::error::{Error, Result};
use rkb_proto::frame::{FrameCodec, MAX_FRAME};

/// A connection over any byte stream.
#[derive(Debug)]
pub struct Connection<S> {
    stream: S,
    codec: FrameCodec,
    decrypt: Option<Aes128Cfb8>,
    encrypt: Option<Aes128Cfb8>,
}

impl<S> Connection<S> {
    /// Wraps a stream in the pre-login, uncompressed state.
    #[must_use]
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            codec: FrameCodec::uncompressed(),
            decrypt: None,
            encrypt: None,
        }
    }

    /// The active compression threshold, if compression is enabled.
    #[must_use]
    pub fn threshold(&self) -> Option<usize> {
        self.codec.threshold()
    }

    /// Whether the key exchange has completed.
    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.encrypt.is_some()
    }

    /// Enables AES/CFB8 in both directions from one shared secret.
    ///
    /// This must be called immediately after the `EncryptionResponse` is
    /// written: the next server packet is already encrypted.
    pub fn enable_encryption(&mut self, secret: &[u8; 16]) {
        self.encrypt = Some(Aes128Cfb8::new(secret));
        self.decrypt = Some(Aes128Cfb8::new(secret));
    }

    /// Enables or disables compression for subsequent frames.
    pub fn set_compression(&mut self, threshold: Option<usize>) {
        self.codec.set_threshold(threshold);
    }

    /// The underlying stream.
    #[must_use]
    pub fn get_ref(&self) -> &S {
        &self.stream
    }

    /// The underlying stream, mutably.
    pub fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    /// Unwraps the stream.
    #[must_use]
    pub fn into_inner(self) -> S {
        self.stream
    }
}

impl<S: Read + Write> Connection<S> {
    /// Reads one packet body (VarInt id plus fields).
    pub fn read_packet(&mut self) -> Result<Vec<u8>> {
        let length = self.read_frame_length()?;
        let mut contents = vec![0u8; length];
        self.stream.read_exact(&mut contents)?;
        if let Some(cipher) = self.decrypt.as_mut() {
            cipher.decrypt(&mut contents);
        }
        self.codec.decode_contents(&contents)
    }

    /// Writes one packet body, applying compression and encryption.
    pub fn write_packet(&mut self, body: &[u8]) -> Result<()> {
        let mut frame = self.codec.encode(body)?;
        if let Some(cipher) = self.encrypt.as_mut() {
            cipher.encrypt(&mut frame);
        }
        self.stream.write_all(&frame)?;
        Ok(())
    }

    /// Flushes buffered output.
    pub fn flush(&mut self) -> Result<()> {
        self.stream.flush()?;
        Ok(())
    }

    fn read_frame_length(&mut self) -> Result<usize> {
        let mut value = 0u32;
        for index in 0..5 {
            let byte = self.read_byte()?;
            value |= u32::from(byte & 0x7F) << (7 * index);
            if byte & 0x80 == 0 {
                let length = value as i32;
                if length < 0 {
                    return Err(Error::NegativeLength(length));
                }
                let length = length as usize;
                if length > MAX_FRAME {
                    return Err(Error::PacketTooLarge {
                        size: length,
                        max: MAX_FRAME,
                    });
                }
                return Ok(length);
            }
        }
        Err(Error::VarIntTooLong)
    }

    fn read_byte(&mut self) -> Result<u8> {
        let mut byte = [0u8; 1];
        self.stream.read_exact(&mut byte)?;
        if let Some(cipher) = self.decrypt.as_mut() {
            cipher.decrypt(&mut byte);
        }
        Ok(byte[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    #[test]
    fn encrypted_and_compressed_frames_round_trip() {
        let (client, server) = pair();
        let secret = [0x5Au8; 16];
        let mut writer = Connection::new(client);
        let mut reader = Connection::new(server);
        writer.enable_encryption(&secret);
        reader.enable_encryption(&secret);
        writer.set_compression(Some(8));
        reader.set_compression(Some(8));

        let body = vec![0x42u8; 200];
        let worker = thread::spawn(move || {
            writer.write_packet(&body).unwrap();
            writer.write_packet(b"\x00small").unwrap();
            writer.flush().unwrap();
        });
        assert_eq!(reader.read_packet().unwrap(), vec![0x42u8; 200]);
        assert_eq!(reader.read_packet().unwrap(), b"\x00small");
        worker.join().unwrap();
    }

    #[test]
    fn encrypted_frames_are_not_plaintext_on_the_wire() {
        let (client, mut server) = pair();
        server
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut writer = Connection::new(client);
        let secret = [0x11u8; 16];
        writer.enable_encryption(&secret);
        writer.write_packet(b"\x00hello").unwrap();
        writer.flush().unwrap();

        // The plaintext frame would be length 6 + id 0 + "hello" = 7 bytes.
        let mut raw = [0u8; 7];
        server.read_exact(&mut raw).unwrap();
        assert_ne!(&raw, b"\x06\x00hello");
    }

    #[test]
    fn disabling_compression_uses_plain_frames() {
        let (client, server) = pair();
        let mut writer = Connection::new(client);
        let mut reader = Connection::new(server);
        assert_eq!(writer.threshold(), None);
        writer.write_packet(b"\x01body").unwrap();
        writer.flush().unwrap();
        assert_eq!(reader.read_packet().unwrap(), b"\x01body");
    }
}
