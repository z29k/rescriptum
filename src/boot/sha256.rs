//! SHA-256, hand-written, because a digest is not worth a dependency.
//!
//! Two jobs and no third: record what an image was when it was ingested, and re-check
//! it later. FIPS 180-4, the straightforward implementation — a 1.5 GB image is hashed
//! once at `media add`, never per request, so the constant factor here buys nothing
//! worth the code to earn it.

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const INITIAL: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

pub struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    buffered: usize,
    /// Message length in **bits**, which is what the padding encodes.
    bits: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Sha256::new()
    }
}

impl Sha256 {
    pub fn new() -> Sha256 {
        Sha256 {
            state: INITIAL,
            block: [0; 64],
            buffered: 0,
            bits: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.bits = self.bits.wrapping_add((data.len() as u64) * 8);

        if self.buffered > 0 {
            let take = (64 - self.buffered).min(data.len());
            self.block[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            // Still short of a block: return, or the tail below would reset `buffered`
            // to zero and drop what is already held.
            if self.buffered < 64 {
                return;
            }
            let block = self.block;
            self.compress(&block);
            self.buffered = 0;
        }

        let mut chunks = data.chunks_exact(64);
        for chunk in &mut chunks {
            let mut block = [0u8; 64];
            block.copy_from_slice(chunk);
            self.compress(&block);
        }

        let rest = chunks.remainder();
        self.block[..rest.len()].copy_from_slice(rest);
        self.buffered = rest.len();
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.bits;
        // 0x80, then zeros, then the length: the padding must land the message on a
        // 64-byte boundary with eight bytes to spare.
        self.pad(0x80);
        while self.buffered != 56 {
            self.pad(0);
        }
        for byte in bits.to_be_bytes() {
            self.pad(byte);
        }
        debug_assert_eq!(self.buffered, 0);

        let mut out = [0u8; 32];
        for (chunk, word) in out.chunks_exact_mut(4).zip(self.state.iter()) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// One padding byte, flushing the block when it fills. Deliberately not `update`:
    /// padding must not count toward the length.
    fn pad(&mut self, byte: u8) {
        self.block[self.buffered] = byte;
        self.buffered += 1;
        if self.buffered == 64 {
            let block = self.block;
            self.compress(&block);
            self.buffered = 0;
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        for (slot, value) in self
            .state
            .iter_mut()
            .zip([a, b, c, d, e, f, g, h].into_iter())
        {
            *slot = slot.wrapping_add(value);
        }
    }
}

/// The digest of a slice, lowercase hex — the form every checksum file in the world uses.
pub fn hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    to_hex(&hasher.finish())
}

pub fn to_hex(digest: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

/// Whether a string is a plausible SHA-256, so `--sha256 deadbeef` is refused at the
/// boundary rather than never matching anything.
pub fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three vectors everybody's SHA-256 is checked against, plus the two that
    /// actually catch padding bugs: a message that lands exactly on a block boundary,
    /// and one that leaves too little room for the length and forces a second block.
    #[test]
    fn known_vectors() {
        assert_eq!(
            hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // 64 bytes: exactly one block, so the padding is entirely a second one.
        assert_eq!(
            hex(&[b'a'; 64]),
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"
        );
        // 56 bytes: the length field has nowhere to go without a second block.
        assert_eq!(
            hex(&[b'a'; 56]),
            "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"
        );
    }

    #[test]
    fn a_streamed_message_hashes_the_same_as_one_slice() {
        // The whole reason this is incremental: a 1.5 GB image is fed 64 KiB at a time,
        // and a buffering bug there would surface as a digest that never matches.
        let data: Vec<u8> = (0..1000u32).flat_map(|n| n.to_le_bytes()).collect();
        let once = hex(&data);

        for chunk in [1usize, 7, 63, 64, 65, 127, 128, 1000] {
            let mut hasher = Sha256::new();
            for part in data.chunks(chunk) {
                hasher.update(part);
            }
            assert_eq!(to_hex(&hasher.finish()), once, "chunked by {chunk}");
        }
    }

    #[test]
    fn a_digest_is_recognised_by_shape() {
        assert!(is_digest(&hex(b"anything")));
        assert!(!is_digest("deadbeef"));
        assert!(!is_digest(&"z".repeat(64)));
        assert!(!is_digest(""));
    }
}
