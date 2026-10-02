//! Minimal RFC 6238 TOTP over HMAC-SHA1 (30 s step, 6 digits).
//!
//! Uses only std + a tiny HMAC-SHA1 implementation so the plugin crate
//! stays dependency-free for the reference pack.

use std::fmt::Write as _;

const SHA1_BLOCK: usize = 64;

fn sha1(message: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let ml = (message.len() as u64) * 8;
    let mut msg = message.to_vec();
    msg.push(0x80);
    while msg.len() % SHA1_BLOCK != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());
    for chunk in msg.chunks(SHA1_BLOCK) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().expect("4 bytes"));
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
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
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    let mut key_block = [0u8; SHA1_BLOCK];
    if key.len() > SHA1_BLOCK {
        key_block[..20].copy_from_slice(&sha1(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let inner: Vec<u8> = key_block.iter().map(|b| b ^ 0x36).chain(message.iter().copied()).collect();
    let outer: Vec<u8> = key_block.iter().map(|b| b ^ 0x5c).chain(sha1(&inner)).collect();
    sha1(&outer)
}

/// Current TOTP code for `secret` at unix `time_secs` (30 s step).
#[must_use]
pub fn totp_now(secret: &[u8], time_secs: u64) -> String {
    totp_at(secret, time_secs / 30)
}

/// TOTP at an explicit counter value.
#[must_use]
pub fn totp_at(secret: &[u8], counter: u64) -> String {
    let counter_bytes = counter.to_be_bytes();
    let mac = hmac_sha1(secret, &counter_bytes);
    let offset = (mac[19] & 0x0f) as usize;
    let bin = u32::from_be_bytes(mac[offset..offset + 4].try_into().expect("4 bytes"));
    let code = (bin & 0x7FFF_FFFF) % 1_000_000;
    let mut out = String::new();
    let _ = write!(out, "{code:06}");
    let _ = std::hint::black_box(&mac);
    out
}

/// Constant-time comparison of the provided code against the expected one,
/// accepting ±`window` steps of clock drift.
#[must_use]
pub fn verify(secret: &[u8], code: &str, now_secs: u64, window: u64) -> bool {
    let counter = now_secs / 30;
    for delta in 0..=window {
        for c in [counter + delta, counter.saturating_sub(delta)] {
            if constant_time_eq(&totp_at(secret, c), code) {
                return true;
            }
        }
    }
    false
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
