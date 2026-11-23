//! GMO Temp OTP core.
//!
//! Runs as a native desktop window: secrets never leave this machine.
//! Implements TOTP (RFC 6238) on top of HMAC-SHA1/SHA256/SHA512 and a small
//! one-time-code extractor for pasted email or SMS text.

pub mod gui;
pub mod vault;

// ------------------------------------------------------------ hashing

fn sha1(msg: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut data = msg.to_vec();
    let bits = (msg.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bits.to_be_bytes());

    for chunk in data.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(msg: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut data = msg.to_vec();
    let bits = (msg.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bits.to_be_bytes());

    for chunk in data.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v[7] = v[6];
            v[6] = v[5];
            v[5] = v[4];
            v[4] = v[3].wrapping_add(t1);
            v[3] = v[2];
            v[2] = v[1];
            v[1] = v[0];
            v[0] = t1.wrapping_add(t2);
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

const SHA512_K: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

fn sha512(msg: &[u8]) -> [u8; 64] {
    let mut h: [u64; 8] = [
        0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
        0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
    ];
    let mut data = msg.to_vec();
    let bits = (msg.len() as u128) * 8;
    data.push(0x80);
    while data.len() % 128 != 112 {
        data.push(0);
    }
    data.extend_from_slice(&bits.to_be_bytes());

    for chunk in data.chunks(128) {
        let mut w = [0u64; 80];
        for i in 0..16 {
            let mut word = [0u8; 8];
            word.copy_from_slice(&chunk[i * 8..i * 8 + 8]);
            w[i] = u64::from_be_bytes(word);
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..80 {
            let s1 = v[4].rotate_right(14) ^ v[4].rotate_right(18) ^ v[4].rotate_right(41);
            let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA512_K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(28) ^ v[0].rotate_right(34) ^ v[0].rotate_right(39);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v[7] = v[6];
            v[6] = v[5];
            v[5] = v[4];
            v[4] = v[3].wrapping_add(t1);
            v[3] = v[2];
            v[2] = v[1];
            v[1] = v[0];
            v[0] = t1.wrapping_add(t2);
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }

    let mut out = [0u8; 64];
    for (i, word) in h.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Algo {
    Sha1,
    Sha256,
    Sha512,
}

impl Algo {
    fn parse(name: &str) -> Algo {
        match name.trim().to_uppercase().replace('-', "").as_str() {
            "SHA256" => Algo::Sha256,
            "SHA512" => Algo::Sha512,
            _ => Algo::Sha1,
        }
    }
}

fn hash(algo: Algo, data: &[u8]) -> Vec<u8> {
    match algo {
        Algo::Sha1 => sha1(data).to_vec(),
        Algo::Sha256 => sha256(data).to_vec(),
        Algo::Sha512 => sha512(data).to_vec(),
    }
}

fn hmac(algo: Algo, key: &[u8], message: &[u8]) -> Vec<u8> {
    let block = match algo {
        Algo::Sha512 => 128,
        _ => 64,
    };
    let mut key_block = if key.len() > block {
        hash(algo, key)
    } else {
        key.to_vec()
    };
    key_block.resize(block, 0);

    let mut inner = Vec::with_capacity(block + message.len());
    inner.extend(key_block.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(message);

    let inner_hash = hash(algo, &inner);
    let mut outer = Vec::with_capacity(block + inner_hash.len());
    outer.extend(key_block.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&inner_hash);
    hash(algo, &outer)
}

// ------------------------------------------------------------ base32

fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let mut bits: u64 = 0;
    let mut bit_count: u32 = 0;
    let mut out = Vec::with_capacity(input.len() * 5 / 8 + 1);
    for ch in input.chars() {
        let value = match ch.to_ascii_uppercase() {
            'A'..='Z' => ch.to_ascii_uppercase() as u8 - b'A',
            '2'..='7' => ch as u8 - b'2' + 26,
            ' ' | '=' | '-' | '_' => continue,
            _ => return None,
        } as u64;
        bits = (bits << 5) | value;
        bit_count += 5;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push(((bits >> bit_count) & 0xff) as u8);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

// ------------------------------------------------------------ totp

fn totp_at(algo: Algo, secret: &[u8], digits: u32, period: u32, unix: u64) -> String {
    let step = (unix / period.max(1) as u64).to_be_bytes();
    let digest = hmac(algo, secret, &step);
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let slice = &digest[offset..offset + 4];
    let binary = u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]) & 0x7fff_ffff;
    let modulo = 10u32.pow(digits.clamp(4, 10) - 1).saturating_mul(10);
    let code = binary % modulo;
    format!("{:0width$}", code, width = digits as usize)
}

/// Generate a code for a base32 secret at a given unix time (seconds).
pub fn totp(secret_b32: &str, digits: u32, period: u32, algorithm: &str, unix_seconds: f64) -> String {
    let Some(secret) = base32_decode(secret_b32) else {
        return String::new();
    };
    if secret.is_empty() {
        return String::new();
    }
    totp_at(
        Algo::parse(algorithm),
        &secret,
        digits.clamp(4, 10),
        period.max(1),
        unix_seconds.max(0.0) as u64,
    )
}

/// Seconds until the current code rolls over.
pub fn seconds_left(period: u32, unix_seconds: f64) -> u32 {
    let period = period.max(1) as u64;
    let now = unix_seconds.max(0.0) as u64;
    (period - (now % period)) as u32
}

// ------------------------------------------------------------ otpauth

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

struct OtpAuth {
    secret: String,
    digits: u32,
    period: u32,
    algorithm: String,
    issuer: String,
    account: String,
}

fn parse_otpauth_uri(uri: &str) -> Option<OtpAuth> {
    let trimmed = uri.trim();
    let lower = trimmed.to_lowercase();
    if !lower.starts_with("otpauth://") {
        return None;
    }
    let rest = &trimmed["otpauth://".len()..];
    let mut parts = rest.splitn(2, '?');
    let label = parts.next()?.to_string();
    let query = parts.next().unwrap_or("");

    let account = label.split('/').nth(1).unwrap_or(&label).to_string();
    let mut secret = String::new();
    let mut digits = 6u32;
    let mut period = 30u32;
    let mut algorithm = "SHA1".to_string();
    let mut issuer = String::new();

    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        let key = kv.next().unwrap_or("").to_lowercase();
        let value = percent_decode(kv.next().unwrap_or(""));
        match key.as_str() {
            "secret" => secret = value,
            "digits" => digits = value.parse().unwrap_or(6),
            "period" => period = value.parse().unwrap_or(30),
            "algorithm" => algorithm = value.to_uppercase(),
            "issuer" => issuer = value,
            _ => {}
        }
    }

    if secret.is_empty() {
        return None;
    }
    if issuer.is_empty() {
        issuer = percent_decode(account.split(':').next().unwrap_or("").trim());
    }
    let account = percent_decode(account.split(':').last().unwrap_or("").trim());

    Some(OtpAuth {
        secret,
        digits,
        period,
        algorithm,
        issuer,
        account,
    })
}

/// A secret read out of an `otpauth://` URI.
#[derive(Debug, Clone, PartialEq)]
pub struct TotpParams {
    pub secret: String,
    pub digits: u32,
    pub period: u32,
    pub algorithm: String,
    pub issuer: String,
    pub account: String,
}

pub fn parse_otpauth(uri: &str) -> Option<TotpParams> {
    parse_otpauth_uri(uri).map(|parsed| TotpParams {
        secret: parsed.secret,
        digits: parsed.digits,
        period: parsed.period,
        algorithm: parsed.algorithm,
        issuer: parsed.issuer,
        account: parsed.account,
    })
}

// ------------------------------------------------------------ extraction

fn looks_like_code(token: &str) -> bool {
    let len = token.chars().count();
    if !(4..=8).contains(&len) {
        return false;
    }
    let digits = token.chars().filter(|c| c.is_ascii_digit()).count();
    let alnum = token.chars().all(|c| c.is_ascii_alphanumeric());
    alnum && digits >= 2
}

/// One-time codes and verification links found in pasted text.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Extracted {
    pub codes: Vec<String>,
    pub links: Vec<String>,
}

pub fn extract(text: &str) -> Extracted {
    const KEYWORDS: [&str; 8] = [
        "code", "otp", "passcode", "verification", "verify", "pin", "security", "one-time",
    ];
    let lower = text.to_lowercase();
    let mut found: Vec<(i32, String)> = Vec::new();

    for token in text.split(|c: char| !c.is_ascii_alphanumeric()) {
        if !looks_like_code(token) {
            continue;
        }
        let pos = text.find(token).unwrap_or(0);
        let start = pos.saturating_sub(60);
        let end = (pos + token.len() + 60).min(lower.len());
        let window = &lower[start..end.min(lower.len())];
        let mut score = 1;
        for keyword in KEYWORDS {
            if window.contains(keyword) {
                score += 3;
            }
        }
        if text.chars().count() < 160 {
            score += 1;
        }
        found.push((score, token.to_string()));
    }

    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.dedup_by(|a, b| a.1 == b.1);

    let mut links: Vec<String> = Vec::new();
    for token in text.split_whitespace() {
        let cleaned = token
            .trim_matches(|c: char| "\"'<>,()".contains(c))
            .trim_end_matches('.');
        if cleaned.starts_with("http://") || cleaned.starts_with("https://") {
            let l = cleaned.to_lowercase();
            if ["verify", "confirm", "activate", "magic", "reset", "token"]
                .iter()
                .any(|hint| l.contains(hint))
                && cleaned.len() < 600
                && !links.iter().any(|existing| existing == cleaned)
            {
                links.push(cleaned.to_string());
            }
        }
    }

    Extracted {
        codes: found
            .iter()
            .take(5)
            .map(|(_, code)| code.clone())
            .collect(),
        links: links.into_iter().take(3).collect(),
    }
}

pub fn version() -> String {
    "1.0.0".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_vectors() {
        // Secret "12345678901234567890" in base32.
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(totp(secret, 8, 30, "SHA1", 59.0), "94287082");
        assert_eq!(totp(secret, 8, 30, "SHA1", 1111111109.0), "07081804");
        assert_eq!(totp(secret, 8, 30, "SHA1", 1234567890.0), "89005924");

        // 32-byte key for SHA256.
        let secret256 = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZA";
        assert_eq!(totp(secret256, 8, 30, "SHA256", 59.0), "46119246");
    }

    #[test]
    fn base32_roundtrip() {
        assert_eq!(base32_decode("GEZDGNBV").unwrap(), b"12345".to_vec());
    }

    #[test]
    fn extracts_code() {
        let out = extract("Your verification code is 481920. It expires soon.");
        assert!(out.codes.iter().any(|code| code == "481920"));
    }

    #[test]
    fn parses_otpauth_uri() {
        let params = parse_otpauth(
            "otpauth://totp/ACME:alice%40example.com?secret=JBSWY3DPEHPK3PXP&issuer=ACME&digits=8&period=60&algorithm=SHA256",
        )
        .unwrap();
        assert_eq!(params.secret, "JBSWY3DPEHPK3PXP");
        assert_eq!(params.digits, 8);
        assert_eq!(params.period, 60);
        assert_eq!(params.algorithm, "SHA256");
        assert_eq!(params.issuer, "ACME");
        assert_eq!(params.account, "alice@example.com");
        assert!(parse_otpauth("https://example.com").is_none());
    }

    #[test]
    fn sha512_rfc6238_vector() {
        // RFC 6238 Appendix B, 64-byte key for SHA-512.
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNA";
        assert_eq!(totp(secret, 8, 30, "SHA512", 59.0), "90693936");
    }

    #[test]
    fn extracts_verification_links_and_ignores_unrelated_urls() {
        let out = extract(
            "Verify https://example.com/verify?token=abc. Ignore https://example.com/docs. Your code is 445566.",
        );
        assert_eq!(out.links, vec!["https://example.com/verify?token=abc"]);
        assert!(out.codes.iter().any(|code| code == "445566"));
    }
}
