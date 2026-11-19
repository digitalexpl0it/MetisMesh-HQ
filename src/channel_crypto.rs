//! MeshCore GRP_TXT (group channel) encrypt / decrypt.
//!
//! Wire: `[channel_hash:1][mac:2][ciphertext…]`
//! - `channel_hash` = SHA-256(secret)[0] (secret is 16 or 32 bytes as stored)
//! - AES-128-ECB with `secret[0..16]`, zero-padded plaintext to 16-byte blocks
//! - HMAC-SHA256 with secret zero-padded to 32 bytes over ciphertext; MAC = first 2 bytes

use aes::cipher::{BlockDecrypt, KeyInit};
#[cfg(test)]
use aes::cipher::BlockEncrypt;
use aes::Aes128;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const BLOCK: usize = 16;
const MAC_LEN: usize = 2;

/// First byte of SHA-256 over the channel secret (16-byte HQ secrets).
pub fn channel_hash(secret16: &[u8; 16]) -> u8 {
    Sha256::digest(secret16)[0]
}

/// HMAC key: 16-byte PSK zero-padded to 32 (MeshCore `PUB_KEY_SIZE`).
fn hmac_key(secret16: &[u8; 16]) -> [u8; 32] {
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(secret16);
    key
}

fn aes_ecb_decrypt(key: &[u8; 16], ciphertext: &[u8]) -> Option<Vec<u8>> {
    if ciphertext.is_empty() || ciphertext.len() % BLOCK != 0 {
        return None;
    }
    let cipher = Aes128::new_from_slice(key).ok()?;
    let mut out = ciphertext.to_vec();
    for chunk in out.chunks_exact_mut(BLOCK) {
        cipher.decrypt_block(chunk.into());
    }
    Some(out)
}

#[cfg(test)]
fn aes_ecb_encrypt(key: &[u8; 16], plaintext: &[u8]) -> Option<Vec<u8>> {
    if plaintext.is_empty() || plaintext.len() % BLOCK != 0 {
        return None;
    }
    let cipher = Aes128::new_from_slice(key).ok()?;
    let mut out = plaintext.to_vec();
    for chunk in out.chunks_exact_mut(BLOCK) {
        cipher.encrypt_block(chunk.into());
    }
    Some(out)
}

fn truncate_mac(secret16: &[u8; 16], ciphertext: &[u8]) -> Option<[u8; MAC_LEN]> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(&hmac_key(secret16)).ok()?;
    mac.update(ciphertext);
    let full = mac.finalize().into_bytes();
    Some([full[0], full[1]])
}

/// Decrypt a GRP_TXT inner payload. Returns the UTF-8 body (`Name: text`) on success.
pub fn try_decrypt_group_text(secret16: &[u8; 16], payload: &[u8]) -> Option<String> {
    if payload.len() < 1 + MAC_LEN + BLOCK {
        return None;
    }
    if payload[0] != channel_hash(secret16) {
        return None;
    }
    let mac = &payload[1..1 + MAC_LEN];
    let ciphertext = &payload[1 + MAC_LEN..];
    if ciphertext.len() % BLOCK != 0 {
        return None;
    }
    let expect = truncate_mac(secret16, ciphertext)?;
    if mac != expect {
        return None;
    }
    let plain = aes_ecb_decrypt(secret16, ciphertext)?;
    parse_group_plaintext(&plain)
}

/// Plaintext after AES: `[timestamp LE:4][txt_type:1][Name: body\0…]`.
fn parse_group_plaintext(plain: &[u8]) -> Option<String> {
    if plain.len() < 5 {
        return None;
    }
    let txt_type = plain[4];
    // Upper six bits are txt_type family; only plain (0) supported here.
    if txt_type >> 2 != 0 {
        return None;
    }
    let body = &plain[5..];
    let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
    let raw = std::str::from_utf8(&body[..end]).ok()?;
    Some(raw.to_string())
}

/// Build a GRP_TXT payload for tests / round-trip.
#[cfg(test)]
pub fn encrypt_group_text(secret16: &[u8; 16], timestamp: u32, wire_body: &str) -> Option<Vec<u8>> {
    let mut plain = Vec::with_capacity(5 + wire_body.len() + BLOCK);
    plain.extend_from_slice(&timestamp.to_le_bytes());
    plain.push(0); // TXT_TYPE_PLAIN
    plain.extend_from_slice(wire_body.as_bytes());
    plain.push(0);
    while plain.len() % BLOCK != 0 {
        plain.push(0);
    }
    let ciphertext = aes_ecb_encrypt(secret16, &plain)?;
    let mac = truncate_mac(secret16, &ciphertext)?;
    let mut out = Vec::with_capacity(1 + MAC_LEN + ciphertext.len());
    out.push(channel_hash(secret16));
    out.extend_from_slice(&mac);
    out.extend_from_slice(&ciphertext);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_secret() -> [u8; 16] {
        *b"0123456789abcdef"
    }

    #[test]
    fn round_trip_wire_body() {
        let secret = sample_secret();
        let wire = "HQ: hello mesh";
        let payload = encrypt_group_text(&secret, 1_700_000_000, wire).expect("encrypt");
        let got = try_decrypt_group_text(&secret, &payload).expect("decrypt");
        assert_eq!(got, wire);
    }

    #[test]
    fn wrong_secret_fails() {
        let secret = sample_secret();
        let payload = encrypt_group_text(&secret, 42, "A: x").unwrap();
        let mut other = secret;
        other[0] ^= 0xff;
        assert!(try_decrypt_group_text(&other, &payload).is_none());
    }

    #[test]
    fn bad_mac_fails() {
        let secret = sample_secret();
        let mut payload = encrypt_group_text(&secret, 42, "A: x").unwrap();
        payload[1] ^= 0xff;
        assert!(try_decrypt_group_text(&secret, &payload).is_none());
    }

    #[test]
    fn channel_hash_stable() {
        assert_eq!(channel_hash(&sample_secret()), channel_hash(&sample_secret()));
    }
}
