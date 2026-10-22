//! SHA-256 (FIPS 180-4), written out here so no third-party code is needed.
//! Every copy the program makes gets one, so that anyone can later check,
//! with tools built into every system (`certutil -hashfile <file> SHA256`
//! on Windows, `shasum -a 256 <file>` on a Mac), that a file is still byte
//! for byte what came off the recovered SSD.
//!
//! On processors with the SHA instructions (most from the last few years)
//! those are used; elsewhere a plain version. Both are checked against the
//! standard's examples and against each other (tests below).

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
    0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
    0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
    0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

const H0: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];

#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Sha256 {
        Sha256 { state: H0, buf: [0; 64], buf_len: 0, total: 0 }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len < 64 {
                return;
            }
            let block = self.buf;
            compress(&mut self.state, &block);
            self.buf_len = 0;
        }
        let whole = data.len() / 64 * 64;
        if whole > 0 {
            compress(&mut self.state, &data[..whole]);
        }
        let rest = &data[whole..];
        self.buf[..rest.len()].copy_from_slice(rest);
        self.buf_len = rest.len();
    }

    /// Feed `n` zero bytes (the parts of a copy that are zeros).
    pub fn update_zeros(&mut self, mut n: u64) {
        const Z: [u8; 65536] = [0; 65536];
        while n > 0 {
            let k = n.min(Z.len() as u64) as usize;
            self.update(&Z[..k]);
            n -= k as u64;
        }
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.total.wrapping_mul(8);
        let mut pad = [0u8; 72];
        pad[0] = 0x80;
        let pad_len = if self.buf_len < 56 { 56 - self.buf_len } else { 120 - self.buf_len };
        pad[pad_len..pad_len + 8].copy_from_slice(&bits.to_be_bytes());
        let total = self.total;
        self.update(&pad[..pad_len + 8]);
        self.total = total;
        debug_assert_eq!(self.buf_len, 0);
        let mut out = [0u8; 32];
        for (i, w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    /// The digest as 64 lower-case hex digits, as every checksum tool shows it.
    pub fn finish_hex(self) -> String {
        hex(&self.finish())
    }

    /// How far the digest has got, as text, so that it can be carried on
    /// later from exactly here (image.rs notes it at every safe point): the
    /// eight state words and the number of bytes so far. Only between whole
    /// 64-byte blocks (None otherwise).
    pub fn export(&self) -> Option<String> {
        if self.buf_len != 0 {
            return None;
        }
        let mut s: String = self.state.iter().map(|w| format!("{w:08x}")).collect();
        s.push_str(&format!("{:016x}", self.total));
        Some(s)
    }

    /// A digest carried on from what `export` gave.
    pub fn import(text: &str) -> Option<Sha256> {
        let t = text.trim();
        if t.len() != 80 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let mut state = [0u32; 8];
        for (i, w) in state.iter_mut().enumerate() {
            *w = u32::from_str_radix(&t[i * 8..i * 8 + 8], 16).ok()?;
        }
        let total = u64::from_str_radix(&t[64..80], 16).ok()?;
        if total % 64 != 0 {
            return None;
        }
        Some(Sha256 { state, buf: [0; 64], buf_len: 0, total })
    }

    /// Bytes fed in so far.
    pub fn total(&self) -> u64 {
        self.total
    }
}

pub fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of a whole file, as hex.
pub fn file_hex(path: &std::path::Path, stop: &dyn Fn() -> bool) -> std::io::Result<Option<String>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if stop() {
            return Ok(None);
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(Some(h.finish_hex()))
}

/// The SHA-256 of a whole file as the drive itself holds it - not what
/// Windows still keeps of it in memory, which for a file just written is
/// where an ordinary read would get its last gigabytes from. On Windows the
/// file is read unbuffered (straight from the drive); elsewhere as usual.
/// `amount` gets (bytes read, file size) as it goes. Returns (size, hex), or
/// None when stopped.
pub fn file_hex_from_drive(
    path: &std::path::Path,
    amount: &mut dyn FnMut(u64, u64),
    stop: &dyn Fn() -> bool,
) -> std::io::Result<Option<(u64, String)>> {
    const CHUNK: usize = 8 << 20;
    // Unbuffered reads need a buffer that starts on a sector boundary.
    const ALIGN: usize = 4096;
    let len = std::fs::metadata(path)?.len();
    let mut raw = vec![0u8; CHUNK + ALIGN];
    let at = raw.as_ptr().align_offset(ALIGN);
    let buf = &mut raw[at..at + CHUNK];
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_NO_BUFFERING | FILE_FLAG_SEQUENTIAL_SCAN
        let unbuffered = std::fs::OpenOptions::new().read(true).custom_flags(0x2000_0000 | 0x0800_0000).open(path);
        match unbuffered.and_then(|mut f| hash_reader(&mut f, buf, len, amount, stop)) {
            // ERROR_INVALID_PARAMETER: a drive with sectors bigger than 4 KB.
            Err(e) if e.raw_os_error() == Some(87) => {}
            other => return other,
        }
    }
    let mut f = std::fs::File::open(path)?;
    hash_reader(&mut f, buf, len, amount, stop)
}

fn hash_reader(
    f: &mut std::fs::File,
    buf: &mut [u8],
    len: u64,
    amount: &mut dyn FnMut(u64, u64),
    stop: &dyn Fn() -> bool,
) -> std::io::Result<Option<(u64, String)>> {
    use std::io::Read;
    let mut h = Sha256::new();
    let mut done = 0u64;
    let mut last = std::time::Instant::now();
    amount(0, len);
    // Reads of a whole buffer at a time, until the size known at the start
    // (an unbuffered read must not start past the end).
    while done < len {
        if stop() {
            return Ok(None);
        }
        let n = f.read(buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        done += n as u64;
        if last.elapsed().as_millis() > 300 {
            amount(done, len);
            last = std::time::Instant::now();
        }
    }
    amount(done, len);
    Ok(Some((done, h.finish_hex())))
}

/// Run the compression function over whole 64-byte blocks.
fn compress(state: &mut [u32; 8], blocks: &[u8]) {
    debug_assert_eq!(blocks.len() % 64, 0);
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("sha")
            && std::arch::is_x86_feature_detected!("sse4.1")
            && std::arch::is_x86_feature_detected!("ssse3")
        {
            // Safe: the processor has the instructions (just checked).
            unsafe { shani::compress(state, blocks) };
            return;
        }
    }
    compress_soft(state, blocks);
}

fn compress_soft(state: &mut [u32; 8], blocks: &[u8]) {
    for block in blocks.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
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
        for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

/// The same with the processor's SHA instructions (Intel's published
/// sequence for SHA-256 with SHA-NI).
#[cfg(target_arch = "x86_64")]
mod shani {
    use std::arch::x86_64::*;

    #[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
    pub unsafe fn compress(state: &mut [u32; 8], blocks: &[u8]) {
        let mask = _mm_set_epi64x(0x0c0d_0e0f_0809_0a0bu64 as i64, 0x0405_0607_0001_0203u64 as i64);
        // state as ABEF / CDGH
        let mut tmp = _mm_loadu_si128(state.as_ptr() as *const __m128i);
        let mut state1 = _mm_loadu_si128(state.as_ptr().add(4) as *const __m128i);
        tmp = _mm_shuffle_epi32(tmp, 0xB1); // CDAB
        state1 = _mm_shuffle_epi32(state1, 0x1B); // EFGH
        let mut state0 = _mm_alignr_epi8(tmp, state1, 8); // ABEF
        state1 = _mm_blend_epi16(state1, tmp, 0xF0); // CDGH

        let k = |i: usize| _mm_loadu_si128(super::K.as_ptr().add(i) as *const __m128i);

        for block in blocks.chunks_exact(64) {
            let abef_save = state0;
            let cdgh_save = state1;
            let p = block.as_ptr() as *const __m128i;
            let mut m0 = _mm_shuffle_epi8(_mm_loadu_si128(p), mask);
            let mut m1 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(1)), mask);
            let mut m2 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(2)), mask);
            let m3 = _mm_shuffle_epi8(_mm_loadu_si128(p.add(3)), mask);

            // Rounds 0-3
            let mut msg = _mm_add_epi32(m0, k(0));
            state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
            msg = _mm_shuffle_epi32(msg, 0x0E);
            state0 = _mm_sha256rnds2_epu32(state0, state1, msg);
            // Rounds 4-7
            msg = _mm_add_epi32(m1, k(4));
            state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
            msg = _mm_shuffle_epi32(msg, 0x0E);
            state0 = _mm_sha256rnds2_epu32(state0, state1, msg);
            m0 = _mm_sha256msg1_epu32(m0, m1);
            // Rounds 8-11
            msg = _mm_add_epi32(m2, k(8));
            state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
            msg = _mm_shuffle_epi32(msg, 0x0E);
            state0 = _mm_sha256rnds2_epu32(state0, state1, msg);
            m1 = _mm_sha256msg1_epu32(m1, m2);
            // Rounds 12-15
            msg = _mm_add_epi32(m3, k(12));
            state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
            let mut t = _mm_alignr_epi8(m3, m2, 4);
            m0 = _mm_add_epi32(m0, t);
            m0 = _mm_sha256msg2_epu32(m0, m3);
            msg = _mm_shuffle_epi32(msg, 0x0E);
            state0 = _mm_sha256rnds2_epu32(state0, state1, msg);
            m2 = _mm_sha256msg1_epu32(m2, m3);

            // Rounds 16-51, four at a time, rotating the message registers.
            let mut ms = [m0, m1, m2, m3];
            for r in 4..13 {
                let (a, b, c, d) = (r % 4, (r + 1) % 4, (r + 2) % 4, (r + 3) % 4);
                // ms[a] holds W[4r..4r+3]
                msg = _mm_add_epi32(ms[a], k(4 * r));
                state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
                t = _mm_alignr_epi8(ms[a], ms[d], 4);
                ms[b] = _mm_add_epi32(ms[b], t);
                ms[b] = _mm_sha256msg2_epu32(ms[b], ms[a]);
                msg = _mm_shuffle_epi32(msg, 0x0E);
                state0 = _mm_sha256rnds2_epu32(state0, state1, msg);
                ms[d] = _mm_sha256msg1_epu32(ms[d], ms[a]);
                let _ = c;
            }
            // Rounds 52-55
            msg = _mm_add_epi32(ms[1], k(52));
            state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
            t = _mm_alignr_epi8(ms[1], ms[0], 4);
            ms[2] = _mm_add_epi32(ms[2], t);
            ms[2] = _mm_sha256msg2_epu32(ms[2], ms[1]);
            msg = _mm_shuffle_epi32(msg, 0x0E);
            state0 = _mm_sha256rnds2_epu32(state0, state1, msg);
            // Rounds 56-59
            msg = _mm_add_epi32(ms[2], k(56));
            state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
            t = _mm_alignr_epi8(ms[2], ms[1], 4);
            ms[3] = _mm_add_epi32(ms[3], t);
            ms[3] = _mm_sha256msg2_epu32(ms[3], ms[2]);
            msg = _mm_shuffle_epi32(msg, 0x0E);
            state0 = _mm_sha256rnds2_epu32(state0, state1, msg);
            // Rounds 60-63
            msg = _mm_add_epi32(ms[3], k(60));
            state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
            msg = _mm_shuffle_epi32(msg, 0x0E);
            state0 = _mm_sha256rnds2_epu32(state0, state1, msg);

            state0 = _mm_add_epi32(state0, abef_save);
            state1 = _mm_add_epi32(state1, cdgh_save);
        }

        tmp = _mm_shuffle_epi32(state0, 0x1B); // FEBA
        state1 = _mm_shuffle_epi32(state1, 0xB1); // DCHG
        state0 = _mm_blend_epi16(tmp, state1, 0xF0); // DCBA
        state1 = _mm_alignr_epi8(state1, tmp, 8); // ABEF -> HGFE
        _mm_storeu_si128(state.as_mut_ptr() as *mut __m128i, state0);
        _mm_storeu_si128(state.as_mut_ptr().add(4) as *mut __m128i, state1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(data: &[u8]) -> String {
        let mut s = Sha256::new();
        s.update(data);
        s.finish_hex()
    }

    #[test]
    fn the_standards_examples() {
        assert_eq!(h(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(h(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            h(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let mut s = Sha256::new();
        let a = [b'a'; 1000];
        for _ in 0..1000 {
            s.update(&a);
        }
        assert_eq!(s.finish_hex(), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn pieces_and_zeros_give_the_same_digest() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let whole = h(&data);
        for cut in [1usize, 63, 64, 65, 1000, 4096, 299_999] {
            let mut s = Sha256::new();
            s.update(&data[..cut]);
            s.update(&data[cut..]);
            assert_eq!(s.finish_hex(), whole, "cut {cut}");
        }
        let mut z = data.clone();
        z.extend(std::iter::repeat_n(0u8, 200_001));
        let mut s = Sha256::new();
        s.update(&data);
        s.update_zeros(200_001);
        assert_eq!(s.finish_hex(), h(&z));
    }

    #[test]
    fn a_file_read_from_the_drive_gives_the_same_digest() {
        let dir = std::env::temp_dir().join(format!("rp-sha-drive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Sizes around the 8 MiB read and the 4 KB sector, and none at all.
        for len in [0usize, 1, 4095, 4096, 4097, (8 << 20) - 1, 8 << 20, (8 << 20) + 12_345, (17 << 20) + 511] {
            let p = dir.join(format!("f{len}"));
            let data: Vec<u8> = (0..len as u32).map(|i| (i.wrapping_mul(2_246_822_519) >> 9) as u8).collect();
            std::fs::write(&p, &data).unwrap();
            let mut seen = (0u64, 0u64);
            let got = file_hex_from_drive(&p, &mut |d, t| seen = (d, t), &|| false).unwrap().unwrap();
            assert_eq!(got, (len as u64, h(&data)), "{len}");
            assert_eq!(seen, (len as u64, len as u64), "{len}");
            assert_eq!(file_hex(&p, &|| false).unwrap().unwrap(), got.1);
        }
        assert!(file_hex_from_drive(&dir.join("f4097"), &mut |_, _| {}, &|| true).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_digest_carried_on_from_its_saved_state_is_the_same() {
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 7) as u8).collect();
        let whole = h(&data);
        for cut in [0usize, 64, 4096, 65_536, 999_936] {
            let mut s = Sha256::new();
            s.update(&data[..cut]);
            let saved = s.export().expect("a whole number of blocks");
            let mut t = Sha256::import(&saved).unwrap();
            assert_eq!(t.total(), cut as u64);
            t.update(&data[cut..]);
            assert_eq!(t.finish_hex(), whole, "cut {cut}");
        }
        let mut s = Sha256::new();
        s.update(b"abc");
        assert!(s.export().is_none(), "not between whole blocks");
        assert!(Sha256::import("nonsense").is_none());
        assert!(Sha256::import(&"0".repeat(79)).is_none());
        // A byte count that is not a whole number of blocks is refused.
        assert!(Sha256::import(&format!("{}{:016x}", "0".repeat(64), 65)).is_none());
    }

    #[test]
    fn processor_instructions_agree_with_the_plain_version() {
        let data: Vec<u8> = (0..64 * 1000u32).map(|i| (i.wrapping_mul(40503) >> 7) as u8).collect();
        let mut a = H0;
        let mut b = H0;
        compress(&mut a, &data);
        compress_soft(&mut b, &data);
        assert_eq!(a, b);
    }
}
