//! RSA for the online-mode login.
//!
//! The server sends an X.509 `SubjectPublicKeyInfo` DER document containing a
//! 1024-bit RSA key. Minecraft encrypts the 16-byte shared secret and the
//! verify token with `RSA/ECB/PKCS1Padding` (PKCS#1 v1.5). This module parses
//! that DER and performs the public-key operation with a small constant-memory
//! big-integer implementation.
//!
//! A private-key helper exists for protocol tests and a future integrated
//! server; the client never uses it.

use std::cmp::Ordering;
use std::fmt;

use crate::error::{Error, Result};

/// The DER contents of `AlgorithmIdentifier` for `rsaEncryption` with NULL
/// parameters: OID 1.2.840.113549.1.1.1 followed by NULL.
const RSA_ALGORITHM: &[u8] = &[
    0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01, 0x05, 0x00,
];

/// An unsigned big integer, little-endian 32-bit limbs.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Big(Vec<u32>);

impl Big {
    fn zero() -> Self {
        Self(Vec::new())
    }

    fn one() -> Self {
        Self(vec![1])
    }

    fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut limbs = Vec::with_capacity(bytes.len().div_ceil(4));
        for chunk in bytes.rchunks(4) {
            let mut value = 0u32;
            for byte in chunk {
                value = (value << 8) | u32::from(*byte);
            }
            limbs.push(value);
        }
        let mut value = Self(limbs);
        value.trim();
        value
    }

    fn to_be_bytes(&self, length: usize) -> Vec<u8> {
        let mut out = vec![0u8; length];
        for (index, limb) in self.0.iter().enumerate() {
            for byte in 0..4 {
                let position = index * 4 + byte;
                if position < length {
                    out[length - 1 - position] = (limb >> (8 * byte)) as u8;
                }
            }
        }
        out
    }

    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }

    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }

    fn is_odd(&self) -> bool {
        self.0.first().is_some_and(|limb| limb & 1 == 1)
    }

    fn bits(&self) -> usize {
        match self.0.last() {
            None => 0,
            Some(last) => (self.0.len() - 1) * 32 + (32 - last.leading_zeros() as usize),
        }
    }

    fn bit(&self, index: usize) -> bool {
        self.0
            .get(index / 32)
            .is_some_and(|limb| (limb >> (index % 32)) & 1 == 1)
    }

    fn cmp(&self, other: &Self) -> Ordering {
        if self.0.len() != other.0.len() {
            return self.0.len().cmp(&other.0.len());
        }
        for index in (0..self.0.len()).rev() {
            if self.0[index] != other.0[index] {
                return self.0[index].cmp(&other.0[index]);
            }
        }
        Ordering::Equal
    }
}

fn add_mod(a: &Big, b: &Big, modulus: &Big) -> Big {
    let length = modulus.0.len();
    let mut out = vec![0u32; length];
    let mut carry = 0u64;
    for (index, slot) in out.iter_mut().enumerate() {
        let left = a.0.get(index).copied().unwrap_or(0) as u64;
        let right = b.0.get(index).copied().unwrap_or(0) as u64;
        let sum = left + right + carry;
        *slot = sum as u32;
        carry = sum >> 32;
    }
    let overflow = carry != 0;
    let needs_subtract = overflow || compare_same_len(&out, &modulus.0) != Ordering::Less;
    if needs_subtract {
        let mut borrow = 0i64;
        for (index, slot) in out.iter_mut().enumerate() {
            let difference = *slot as i64 - modulus.0[index] as i64 - borrow;
            if difference < 0 {
                *slot = (difference + (1i64 << 32)) as u32;
                borrow = 1;
            } else {
                *slot = difference as u32;
                borrow = 0;
            }
        }
    }
    let mut result = Big(out);
    result.trim();
    result
}

fn compare_same_len(left: &[u32], right: &[u32]) -> Ordering {
    for index in (0..left.len()).rev() {
        if left[index] != right[index] {
            return left[index].cmp(&right[index]);
        }
    }
    Ordering::Equal
}

fn mul_mod(a: &Big, b: &Big, modulus: &Big) -> Big {
    let mut result = Big::zero();
    let mut addend = a.clone();
    let bits = b.bits();
    for index in 0..bits {
        if b.bit(index) {
            result = add_mod(&result, &addend, modulus);
        }
        if index + 1 < bits {
            addend = add_mod(&addend, &addend, modulus);
        }
    }
    result
}

fn mod_pow(base: &Big, exponent: &Big, modulus: &Big) -> Big {
    let mut result = Big::one();
    let bits = exponent.bits();
    for index in (0..bits).rev() {
        result = mul_mod(&result, &result, modulus);
        if exponent.bit(index) {
            result = mul_mod(&result, base, modulus);
        }
    }
    result
}

/// A parsed RSA public key.
#[derive(Clone, Debug)]
pub struct RsaPublicKey {
    modulus: Big,
    exponent: Big,
}

impl RsaPublicKey {
    /// Parses the DER `SubjectPublicKeyInfo` Minecraft servers send.
    pub fn from_der(der: &[u8]) -> Result<Self> {
        let mut outer = Der::new(der);
        let top = outer.expect(0x30)?;
        outer.finish()?;

        let mut sequence = Der::new(top);
        let algorithm = sequence.expect(0x30)?;
        if algorithm != RSA_ALGORITHM {
            return Err(Error::protocol("RSA public key is not rsaEncryption"));
        }
        let bit_string = sequence.expect(0x03)?;
        sequence.finish()?;
        if bit_string.first() != Some(&0) {
            return Err(Error::protocol("RSA key bit string has unused bits"));
        }

        let mut inner = Der::new(&bit_string[1..]);
        let key = inner.expect(0x30)?;
        inner.finish()?;
        let mut key = Der::new(key);
        let modulus = parse_integer(key.expect(0x02)?)?;
        let exponent = parse_integer(key.expect(0x02)?)?;
        key.finish()?;

        Self::from_parts(modulus, exponent)
    }

    fn from_parts(modulus: Big, exponent: Big) -> Result<Self> {
        let bits = modulus.bits();
        if !(512..=4096).contains(&bits) {
            return Err(Error::protocol(format!(
                "RSA modulus is {bits} bits; expected 512..=4096"
            )));
        }
        if !modulus.is_odd() {
            return Err(Error::protocol("RSA modulus is even"));
        }
        if exponent.bits() == 0 || !exponent.is_odd() || exponent == Big::one() {
            return Err(Error::protocol("invalid RSA public exponent"));
        }
        if exponent.cmp(&modulus) != Ordering::Less {
            return Err(Error::protocol("RSA exponent is not below the modulus"));
        }
        Ok(Self { modulus, exponent })
    }

    /// The modulus length in bytes (the ciphertext length).
    #[must_use]
    pub fn ciphertext_len(&self) -> usize {
        self.modulus.bits().div_ceil(8)
    }

    /// Encrypts `message` with PKCS#1 v1.5 padding.
    pub fn encrypt(&self, message: &[u8]) -> Result<Vec<u8>> {
        let length = self.ciphertext_len();
        if message.len() + 11 > length {
            return Err(Error::protocol(format!(
                "message of {} bytes is too long for a {length}-byte RSA block",
                message.len()
            )));
        }
        let mut block = vec![0u8; length];
        block[0] = 0;
        block[1] = 2;
        let padding = length - message.len() - 3;
        let mut filled = 0;
        while filled < padding {
            let mut byte = [0u8; 1];
            getrandom::getrandom(&mut byte).map_err(|error| Error::random(error.to_string()))?;
            if byte[0] != 0 {
                block[2 + filled] = byte[0];
                filled += 1;
            }
        }
        block[2 + padding] = 0;
        block[3 + padding..].copy_from_slice(message);

        let value = Big::from_be_bytes(&block);
        if value.cmp(&self.modulus) != Ordering::Less {
            return Err(Error::protocol("padded RSA value is not below the modulus"));
        }
        let ciphertext = mod_pow(&value, &self.exponent, &self.modulus);
        Ok(ciphertext.to_be_bytes(length))
    }
}

/// A private key for protocol tests and a future server-side implementation.
#[derive(Clone, Debug)]
pub struct RsaPrivateKey {
    modulus: Big,
    exponent: Big,
}

impl RsaPrivateKey {
    /// Builds a key from big-endian modulus and private exponent bytes.
    pub fn new(modulus: &[u8], exponent: &[u8]) -> Result<Self> {
        let modulus = Big::from_be_bytes(modulus);
        let exponent = Big::from_be_bytes(exponent);
        if modulus.bits() < 512 || modulus.bits() > 4096 {
            return Err(Error::protocol("invalid RSA private modulus size"));
        }
        if !modulus.is_odd() || exponent.is_zero() || !exponent.is_odd() {
            return Err(Error::protocol("invalid RSA private key components"));
        }
        if exponent.cmp(&modulus) != Ordering::Less {
            return Err(Error::protocol(
                "RSA private exponent is not below the modulus",
            ));
        }
        Ok(Self { modulus, exponent })
    }

    /// Decrypts a PKCS#1 v1.5 ciphertext.
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let length = self.modulus.bits().div_ceil(8);
        if ciphertext.len() != length {
            return Err(Error::protocol(format!(
                "ciphertext is {} bytes; expected {length}",
                ciphertext.len()
            )));
        }
        let value = Big::from_be_bytes(ciphertext);
        if value.cmp(&self.modulus) != Ordering::Less {
            return Err(Error::protocol("ciphertext is not below the modulus"));
        }
        let plain = mod_pow(&value, &self.exponent, &self.modulus);
        let block = plain.to_be_bytes(length);
        if block.len() < 11 || block[0] != 0 || block[1] != 2 {
            return Err(Error::protocol("invalid PKCS#1 v1.5 padding"));
        }
        let separator = block[2..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|position| position + 2)
            .ok_or_else(|| Error::protocol("missing PKCS#1 v1.5 separator"))?;
        if separator < 10 {
            return Err(Error::protocol(
                "PKCS#1 v1.5 padding is shorter than 8 bytes",
            ));
        }
        Ok(block[separator + 1..].to_vec())
    }
}

fn parse_integer(bytes: &[u8]) -> Result<Big> {
    if bytes.is_empty() {
        return Err(Error::protocol("empty DER integer"));
    }
    if bytes[0] & 0x80 != 0 {
        return Err(Error::protocol("negative DER integer"));
    }
    let start = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len());
    Ok(Big::from_be_bytes(&bytes[start..]))
}

struct Der<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Der<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn expect(&mut self, expected: u8) -> Result<&'a [u8]> {
        let (tag, value) = self.read_tlv()?;
        if tag != expected {
            return Err(Error::protocol(format!(
                "DER tag 0x{tag:02X}, expected 0x{expected:02X}"
            )));
        }
        Ok(value)
    }

    fn read_tlv(&mut self) -> Result<(u8, &'a [u8])> {
        let tag = *self
            .data
            .get(self.position)
            .ok_or_else(|| Error::protocol("truncated DER tag"))?;
        self.position += 1;
        let first = *self
            .data
            .get(self.position)
            .ok_or_else(|| Error::protocol("truncated DER length"))?;
        self.position += 1;
        let length = if first & 0x80 == 0 {
            usize::from(first)
        } else {
            let count = usize::from(first & 0x7F);
            if count == 0 || count > 4 {
                return Err(Error::protocol("unsupported DER length"));
            }
            let mut length = 0usize;
            for _ in 0..count {
                let byte = *self
                    .data
                    .get(self.position)
                    .ok_or_else(|| Error::protocol("truncated DER length"))?;
                self.position += 1;
                length = (length << 8) | usize::from(byte);
            }
            length
        };
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| Error::protocol("DER length overflow"))?;
        if end > self.data.len() {
            return Err(Error::protocol("truncated DER value"));
        }
        let value = &self.data[self.position..end];
        self.position = end;
        Ok((tag, value))
    }

    fn finish(&self) -> Result<()> {
        if self.position == self.data.len() {
            Ok(())
        } else {
            Err(Error::protocol("trailing bytes after DER value"))
        }
    }
}

impl fmt::Display for RsaPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RSA-{} public key", self.modulus.bits())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PUBLIC_DER: &[u8] = include_bytes!("../../tests/fixtures/rsa_test_pub.der");
    const TEST_MODULUS: &str = "df37d97634ac5fba3034ec94adf442f898a686c8d9a55505cbd73e4ff55fb2ad3e8e6d30becfb52b6b995f6c3c7be25dd28841d25cb8d39ef0ca0c63b597d2ffbc817972bd7867bfcabfaf95fd847341c6ede5b74f28cc27e945cce5232b9b3723cd6953739a2473a21d8c47d02a8884b8729f2e3189ba29f8c06e9a801061f5";
    const TEST_PRIVATE_EXPONENT: &str = "30f6e754eea90136ae6c6d990faf8d214b1afc66029829d663a88b9aae9097793947b4dc047f3b9692bbca1acc09c0d623e08fcfbd572dd4a7ad2a9a94b3012d7eef6ba9f9124743f15453034f5fdbdc4e925a056ea81a159ae45327d297783401dd339d39ef167d781b59a5623609b8d9b67a47285d7f5ec8dfa9f4f0ebf801";

    fn hex(text: &str) -> Vec<u8> {
        assert_eq!(text.len() % 2, 0);
        (0..text.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn parses_the_test_key() {
        let key = RsaPublicKey::from_der(TEST_PUBLIC_DER).unwrap();
        assert_eq!(key.ciphertext_len(), 128);
        assert!(key.to_string().contains("RSA-1024"));
    }

    #[test]
    fn pkcs1_encrypts_and_private_key_decrypts() {
        let public = RsaPublicKey::from_der(TEST_PUBLIC_DER).unwrap();
        let private = RsaPrivateKey::new(&hex(TEST_MODULUS), &hex(TEST_PRIVATE_EXPONENT)).unwrap();
        for message in [b"secret".as_slice(), b"token".as_slice(), &[0u8; 16]] {
            let ciphertext = public.encrypt(message).unwrap();
            assert_eq!(ciphertext.len(), 128);
            assert_ne!(ciphertext, message);
            assert_eq!(private.decrypt(&ciphertext).unwrap(), message);
        }
    }

    #[test]
    fn decrypts_an_openssl_pkcs1_ciphertext() {
        // `openssl pkeyutl -encrypt -pubin -inkey rsa_test_pub.der
        //  -pkeyopt rsa_padding_mode:pkcs1` over b"rkb-rsa-test".
        let ciphertext = hex(
            "bfd480c5a13815faa159da5cdd578ead65731c09a8c187c6d1c3e001faadaa92\
             470eaf7a146236dd263042e191e217b37ad54330f60dcb1ef070ce1591e22cbd\
             50914bee7b5730d70f7a55bdbfabf19c286b535c2c878eb3872c487fa7d4bcad\
             fc60261c5b91ff3a92fa6189f4d3f8eadbf86cb04264854d7cb581147637a89e",
        );
        let private = RsaPrivateKey::new(&hex(TEST_MODULUS), &hex(TEST_PRIVATE_EXPONENT)).unwrap();
        assert_eq!(private.decrypt(&ciphertext).unwrap(), b"rkb-rsa-test");
    }

    #[test]
    fn rejects_messages_that_do_not_fit() {
        let public = RsaPublicKey::from_der(TEST_PUBLIC_DER).unwrap();
        assert!(public.encrypt(&[0u8; 118]).is_err());
        assert!(public.encrypt(&[0u8; 117]).is_ok());
    }

    #[test]
    fn rejects_malformed_der() {
        assert!(RsaPublicKey::from_der(&[]).is_err());
        assert!(RsaPublicKey::from_der(&TEST_PUBLIC_DER[..TEST_PUBLIC_DER.len() - 1]).is_err());
        let mut trailing = TEST_PUBLIC_DER.to_vec();
        trailing.push(0);
        assert!(RsaPublicKey::from_der(&trailing).is_err());
        assert!(RsaPublicKey::from_der(b"\x30\x00").is_err());
    }

    #[test]
    fn rejects_ciphertext_of_the_wrong_length() {
        let private = RsaPrivateKey::new(&hex(TEST_MODULUS), &hex(TEST_PRIVATE_EXPONENT)).unwrap();
        assert!(private.decrypt(&[0u8; 127]).is_err());
        assert!(private.decrypt(&[0u8; 129]).is_err());
    }

    #[test]
    fn big_integer_helpers() {
        assert_eq!(Big::from_be_bytes(&[0, 1, 0]).bits(), 9);
        assert_eq!(Big::from_be_bytes(&[0xFF]).to_be_bytes(2), vec![0, 0xFF]);
        assert!(Big::from_be_bytes(&[1]).is_odd());
        assert!(!Big::from_be_bytes(&[2]).is_odd());
        assert_eq!(
            Big::from_be_bytes(&[1, 0]).cmp(&Big::from_be_bytes(&[0xFF])),
            Ordering::Greater
        );
    }
}
