//! Cryptography required by the Minecraft: Java Edition login.
//!
//! Everything here exists because protocol 767 requires it, not as a general
//! crypto library:
//!
//! - [`aes`]: AES-128/CFB8, the stream cipher both directions use after login.
//! - [`rsa`]: X.509 public-key parsing and PKCS#1 v1.5 encryption for the
//!   shared secret and verify token.
//! - [`sha1`]: the online-mode session hash.
//! - [`md5`]: vanilla's offline-mode UUID derivation.
//!
//! No key material is embedded and nothing here logs secret values.

pub mod aes;
pub mod md5;
pub mod rsa;
pub mod sha1;

pub use aes::Aes128Cfb8;
pub use rsa::{RsaPrivateKey, RsaPublicKey};
pub use sha1::Sha1;

use crate::error::{Error, Result};

/// The shared-secret length Minecraft uses (`AES` block size).
pub const SHARED_SECRET_LEN: usize = 16;

/// Fills `buffer` from the operating system's secure random source.
pub fn random_bytes(buffer: &mut [u8]) -> Result<()> {
    getrandom::getrandom(buffer).map_err(|error| Error::random(error.to_string()))
}

/// A fresh 16-byte AES/CFB8 shared secret.
pub fn generate_secret() -> Result<[u8; SHARED_SECRET_LEN]> {
    let mut secret = [0u8; SHARED_SECRET_LEN];
    random_bytes(&mut secret)?;
    Ok(secret)
}

/// The Minecraft online-mode session hash for the current login.
///
/// Vanilla computes `SHA1(server_id + shared_secret + public_key_der)` and
/// renders the digest with `BigInteger.toString(16)`, which is two's-complement
/// signed hex: a leading `-` when the high bit is set, and no zero padding.
#[must_use]
pub fn server_hash(server_id: &str, secret: &[u8], public_key_der: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(server_id.as_bytes());
    hasher.update(secret);
    hasher.update(public_key_der);
    java_hex_digest(&hasher.finish())
}

/// Renders bytes the way Java's `BigInteger(byte[]).toString(16)` does.
///
/// Positive values have no leading zeros; negative values use two's-complement
/// magnitude with a leading `-`. This is the exact text sent to the session
/// server.
#[must_use]
pub fn java_hex_digest(bytes: &[u8]) -> String {
    let Some((&first, _)) = bytes.split_first() else {
        return "0".to_owned();
    };
    let negative = first & 0x80 != 0;
    let magnitude;
    let source = if negative {
        // Two's-complement negation: invert every byte, then add one.
        let mut inverted = bytes.to_vec();
        for byte in &mut inverted {
            *byte = !*byte;
        }
        let mut carry = 1u16;
        for byte in inverted.iter_mut().rev() {
            let sum = u16::from(*byte) + carry;
            *byte = sum as u8;
            carry = sum >> 8;
        }
        magnitude = inverted;
        magnitude.as_slice()
    } else {
        bytes
    };

    let mut hex = String::with_capacity(source.len() * 2 + 1);
    let mut started = false;
    for byte in source {
        if started {
            hex.push_str(&format!("{byte:02x}"));
        } else if *byte != 0 {
            hex.push_str(&format!("{byte:x}"));
            started = true;
        }
    }
    if !started {
        return "0".to_owned();
    }
    if negative {
        hex.insert(0, '-');
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        assert_eq!(text.len() % 2, 0);
        (0..text.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn secret_is_sixteen_bytes_and_random() {
        let first = generate_secret().unwrap();
        let second = generate_secret().unwrap();
        assert_eq!(first.len(), SHARED_SECRET_LEN);
        assert_ne!(first, second, "two draws should not collide");
    }

    #[test]
    fn java_hex_matches_big_integer_formatting() {
        // BigInteger("00ff", 16).toString(16) == "ff"
        assert_eq!(java_hex_digest(&[0x00, 0xFF]), "ff");
        // All-zero is "0", not empty.
        assert_eq!(java_hex_digest(&[0, 0, 0]), "0");
        assert_eq!(java_hex_digest(&[]), "0");
        // 0x80 as a signed byte is -128.
        assert_eq!(java_hex_digest(&[0x80]), "-80");
        // 0xFFFF is -1.
        assert_eq!(java_hex_digest(&[0xFF, 0xFF]), "-1");
        // 0x8000 is -32768.
        assert_eq!(java_hex_digest(&[0x80, 0x00]), "-8000");
    }

    #[test]
    fn server_hash_matches_the_vanilla_formula() {
        // Empty server id, fixed secret and key. Independent SHA-1 of the same
        // concatenation is checked in the assertion below.
        let secret = [0x11u8; 16];
        let key = hex("3082010a");
        // SHA1(0x11 * 16 || 30 82 01 0a), independently computed; the digest
        // has its high bit set, so it renders as a negative Java BigInteger.
        assert_eq!(
            server_hash("", &secret, &key),
            "-4b0bb9f567cb95ca630d64d0e2055b4c3b0f1504"
        );
        assert_ne!(
            server_hash("sid", &secret, &key),
            server_hash("", &secret, &key)
        );
    }

    #[test]
    fn known_sha1_digest_rendering() {
        // SHA1("jeb_") = 8362a4ffbb3ecfef65a284a04a3ce83fd4b1d73f, which as a
        // Java negative BigInteger renders with a leading '-'.
        let digest = sha1::digest(b"jeb_");
        assert_eq!(digest, hex("8362a4ffbb3ecfef65a284a04a3ce83fd4b1d73f")[..]);
        assert_eq!(
            java_hex_digest(&digest),
            "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1"
        );
    }
}
