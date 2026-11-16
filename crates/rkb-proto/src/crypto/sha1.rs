//! SHA-1 for the Minecraft server hash.
//!
//! SHA-1 is used here only because the Minecraft protocol specifies it for the
//! online-mode session hash. It is not used for passwords or token storage.

/// Incremental SHA-1.
#[derive(Clone, Debug)]
pub struct Sha1 {
    state: [u32; 5],
    length: u64,
    buffer: [u8; 64],
    used: usize,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    /// A fresh hasher.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: [
                0x6745_2301,
                0xEFCD_AB89,
                0x98BA_DCFE,
                0x1032_5476,
                0xC3D2_E1F0,
            ],
            length: 0,
            buffer: [0; 64],
            used: 0,
        }
    }

    /// Adds bytes to the hash.
    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        if self.used > 0 {
            let take = (64 - self.used).min(data.len());
            self.buffer[self.used..self.used + take].copy_from_slice(&data[..take]);
            self.used += take;
            data = &data[take..];
            if self.used == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.used = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.used = data.len();
        }
    }

    /// Finishes and returns the 20-byte digest.
    #[must_use]
    pub fn finish(mut self) -> [u8; 20] {
        let bit_length = self.length.wrapping_mul(8);
        self.buffer[self.used] = 0x80;
        self.used += 1;
        if self.used > 56 {
            while self.used < 64 {
                self.buffer[self.used] = 0;
                self.used += 1;
            }
            let block = self.buffer;
            self.compress(&block);
            self.used = 0;
        }
        while self.used < 56 {
            self.buffer[self.used] = 0;
            self.used += 1;
        }
        self.buffer[56..64].copy_from_slice(&bit_length.to_be_bytes());
        let block = self.buffer;
        self.compress(&block);

        let mut digest = [0u8; 20];
        for (index, value) in self.state.iter().enumerate() {
            digest[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
        }
        digest
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[index * 4],
                block[index * 4 + 1],
                block[index * 4 + 2],
                block[index * 4 + 3],
            ]);
        }
        for index in 16..80 {
            w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (index, word) in w.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

/// Hashes one buffer.
#[must_use]
pub fn digest(data: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(digest: &[u8]) -> String {
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn known_vectors() {
        assert_eq!(
            hex(&digest(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            hex(&digest(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&digest(b"The quick brown fox jumps over the lazy dog")),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
    }

    #[test]
    fn padding_boundaries_match_known_values() {
        // Lengths around the 55/56-byte padding boundary and block boundaries.
        let expected = [
            (55, "c1c8bbdc22796e28c0e15163d20899b65621d65a"),
            (56, "c2db330f6083854c99d4b5bfb6e8f29f201be699"),
            (57, "f08f24908d682555111be7ff6f004e78283d989a"),
            (63, "03f09f5b158a7a8cdad920bddc29b81c18a551f5"),
            (64, "0098ba824b5c16427bd7a1122a5a442a25ec644d"),
            (65, "11655326c708d70319be2610e8a57d9a5b959d3b"),
            (119, "ee971065aaa017e0632a8ca6c77bb3bf8b1dfc56"),
            (120, "f34c1488385346a55709ba056ddd08280dd4c6d6"),
        ];
        for (length, digest) in expected {
            assert_eq!(hex(&self::digest(&vec![b'a'; length])), digest, "{length}");
        }
    }

    #[test]
    fn incremental_updates_match_one_shot() {
        let text = b"incremental SHA-1 should match the one-shot hash";
        let mut hasher = Sha1::new();
        for chunk in text.chunks(3) {
            hasher.update(chunk);
        }
        assert_eq!(hasher.finish(), digest(text));
    }

    #[test]
    fn long_input_matches_known_value() {
        let data = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&digest(&data)),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }
}
