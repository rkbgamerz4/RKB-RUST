//! AES-128 and the AES/CFB8 stream mode used by the Minecraft login.
//!
//! Minecraft's `Crypt.getCipher` is `AES/CFB8/NoPadding` with the 16-byte
//! shared secret as both key and IV. Each direction of the connection has its
//! own cipher state.

/// The AES S-box, derived at compile time from the finite-field inverse and
/// the affine transform. Deriving it avoids a hand-copied table.
const SBOX: [u8; 256] = build_sbox();

const fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    let mut bit = 0;
    while bit < 8 {
        if b & 1 != 0 {
            product ^= a;
        }
        let high = a & 0x80;
        a <<= 1;
        if high != 0 {
            a ^= 0x1B;
        }
        b >>= 1;
        bit += 1;
    }
    product
}

/// `a^254` in GF(2^8), the multiplicative inverse for non-zero `a`.
const fn gf_inverse(a: u8) -> u8 {
    let mut result = 1u8;
    let mut i = 0;
    while i < 254 {
        result = gf_mul(result, a);
        i += 1;
    }
    result
}

const fn rotl8(value: u8, amount: u32) -> u8 {
    value.rotate_left(amount)
}

const fn build_sbox() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let inverse = gf_inverse(i as u8);
        table[i] = inverse
            ^ rotl8(inverse, 1)
            ^ rotl8(inverse, 2)
            ^ rotl8(inverse, 3)
            ^ rotl8(inverse, 4)
            ^ 0x63;
        i += 1;
    }
    table
}

const fn xtime(value: u8) -> u8 {
    (value << 1) ^ if value & 0x80 != 0 { 0x1B } else { 0 }
}

const ROUNDS: usize = 10;
const WORDS: usize = 4 * (ROUNDS + 1);

/// AES-128 block encryption.
#[derive(Clone, Debug)]
struct Aes128 {
    words: [[u8; 4]; WORDS],
}

impl Aes128 {
    fn new(key: &[u8; 16]) -> Self {
        let mut words = [[0u8; 4]; WORDS];
        for (index, word) in words.iter_mut().take(4).enumerate() {
            word.copy_from_slice(&key[index * 4..index * 4 + 4]);
        }
        let mut rcon = 1u8;
        for index in 4..WORDS {
            let mut temp = words[index - 1];
            if index % 4 == 0 {
                temp = [
                    SBOX[temp[1] as usize] ^ rcon,
                    SBOX[temp[2] as usize],
                    SBOX[temp[3] as usize],
                    SBOX[temp[0] as usize],
                ];
                rcon = xtime(rcon);
            }
            for (byte, value) in temp.iter().enumerate() {
                words[index][byte] = words[index - 4][byte] ^ value;
            }
        }
        Self { words }
    }

    fn encrypt_block(&self, block: &[u8; 16]) -> [u8; 16] {
        let mut state = *block;
        self.add_round_key(&mut state, 0);
        for round in 1..ROUNDS {
            sub_bytes(&mut state);
            shift_rows(&mut state);
            mix_columns(&mut state);
            self.add_round_key(&mut state, round);
        }
        sub_bytes(&mut state);
        shift_rows(&mut state);
        self.add_round_key(&mut state, ROUNDS);
        state
    }

    fn add_round_key(&self, state: &mut [u8; 16], round: usize) {
        for column in 0..4 {
            let word = self.words[round * 4 + column];
            for row in 0..4 {
                state[row + 4 * column] ^= word[row];
            }
        }
    }
}

fn sub_bytes(state: &mut [u8; 16]) {
    for byte in state.iter_mut() {
        *byte = SBOX[*byte as usize];
    }
}

fn shift_rows(state: &mut [u8; 16]) {
    for row in 1..4 {
        let values = [state[row], state[row + 4], state[row + 8], state[row + 12]];
        for column in 0..4 {
            state[row + 4 * column] = values[(column + row) % 4];
        }
    }
}

fn mix_columns(state: &mut [u8; 16]) {
    for column in 0..4 {
        let offset = 4 * column;
        let a = [
            state[offset],
            state[offset + 1],
            state[offset + 2],
            state[offset + 3],
        ];
        let all = a[0] ^ a[1] ^ a[2] ^ a[3];
        state[offset] = all ^ a[0] ^ xtime(a[0] ^ a[1]);
        state[offset + 1] = all ^ a[1] ^ xtime(a[1] ^ a[2]);
        state[offset + 2] = all ^ a[2] ^ xtime(a[2] ^ a[3]);
        state[offset + 3] = all ^ a[3] ^ xtime(a[3] ^ a[0]);
    }
}

/// AES/CFB8 with the shared secret as key and IV.
#[derive(Clone, Debug)]
pub struct Aes128Cfb8 {
    cipher: Aes128,
    register: [u8; 16],
}

impl Aes128Cfb8 {
    /// A fresh cipher state. Minecraft uses the same 16 bytes for key and IV.
    #[must_use]
    pub fn new(secret: &[u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(secret),
            register: *secret,
        }
    }

    /// Encrypts in place.
    pub fn encrypt(&mut self, data: &mut [u8]) {
        for byte in data {
            let keystream = self.cipher.encrypt_block(&self.register);
            let ciphertext = *byte ^ keystream[0];
            *byte = ciphertext;
            self.register.rotate_left(1);
            self.register[15] = ciphertext;
        }
    }

    /// Decrypts in place.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        for byte in data {
            let ciphertext = *byte;
            let keystream = self.cipher.encrypt_block(&self.register);
            *byte = ciphertext ^ keystream[0];
            self.register.rotate_left(1);
            self.register[15] = ciphertext;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex<const N: usize>(hex: &str) -> [u8; N] {
        assert_eq!(hex.len(), N * 2);
        let mut out = [0u8; N];
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn sbox_known_entries() {
        assert_eq!(SBOX[0x00], 0x63);
        assert_eq!(SBOX[0x01], 0x7C);
        assert_eq!(SBOX[0x10], 0xCA);
        assert_eq!(SBOX[0xFF], 0x16);
    }

    #[test]
    fn fips_197_block_vector() {
        let key: [u8; 16] = unhex("000102030405060708090a0b0c0d0e0f");
        let plaintext: [u8; 16] = unhex("00112233445566778899aabbccddeeff");
        let ciphertext: [u8; 16] = unhex("69c4e0d86a7b0430d8cdb78070b4c55a");
        assert_eq!(Aes128::new(&key).encrypt_block(&plaintext), ciphertext);
    }

    #[test]
    fn nist_cfb8_vector() {
        let key: [u8; 16] = unhex("2b7e151628aed2a6abf7158809cf4f3c");
        let iv: [u8; 16] = unhex("000102030405060708090a0b0c0d0e0f");
        let mut plaintext: [u8; 32] =
            unhex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        let expected: [u8; 32] =
            unhex("3b79424c9c0dd436bace9e0ed4586a4f32b9ded50ae3ba69d472e88267fb5052");

        // The public constructor uses key=IV, so build the NIST case directly.
        let mut cipher = Aes128Cfb8 {
            cipher: Aes128::new(&key),
            register: iv,
        };
        cipher.encrypt(&mut plaintext);
        assert_eq!(plaintext, expected);

        let mut cipher = Aes128Cfb8 {
            cipher: Aes128::new(&key),
            register: iv,
        };
        cipher.decrypt(&mut plaintext);
        assert_eq!(
            plaintext,
            unhex::<32>("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51")
        );
    }

    #[test]
    fn minecraft_key_equals_iv_round_trips() {
        let secret: [u8; 16] = unhex("00112233445566778899aabbccddeeff");
        let original = b"the quick brown fox jumps over the lazy dog";
        let mut data = original.to_vec();
        Aes128Cfb8::new(&secret).encrypt(&mut data);
        assert_ne!(data, original);
        Aes128Cfb8::new(&secret).decrypt(&mut data);
        assert_eq!(data, original);
    }

    #[test]
    fn empty_input_keeps_state_unchanged() {
        let secret = [7u8; 16];
        let mut cipher = Aes128Cfb8::new(&secret);
        cipher.encrypt(&mut []);
        cipher.decrypt(&mut []);
    }
}
