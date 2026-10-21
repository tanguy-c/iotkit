//! `Noise_NNpsk0_25519_ChaChaPoly_SHA256` (noiseprotocol.org), the responder side, as the
//! ESPHome API uses it: the shared key authenticates both ends, fresh X25519 keys make each
//! session's keys, ChaCha20-Poly1305 encrypts the messages.

use chacha20poly1305::{AeadInPlace, ChaCha20Poly1305, KeyInit, Nonce, Tag};
use ed25519_compact::x25519::{PublicKey, SecretKey};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};

const PROTOCOL: &[u8] = b"Noise_NNpsk0_25519_ChaChaPoly_SHA256";
/// What both ends mix in first: the ESPHome API's.
const PROLOGUE: &[u8] = b"NoiseAPIInit\0\0";
/// Each encrypted message's authentication tag.
pub const TAG: usize = 16;
/// The client's handshake message: its X25519 key, and the tag of an empty payload.
pub const HANDSHAKE: usize = 32 + TAG;

/// One direction's key and message counter.
pub struct Cipher {
    aead: ChaCha20Poly1305,
    n: u64,
}

impl Cipher {
    fn new(key: &[u8; 32]) -> Self {
        Self { aead: ChaCha20Poly1305::new(key.into()), n: 0 }
    }

    fn nonce(&mut self) -> Nonce {
        let mut nonce = Nonce::default();
        nonce[4..].copy_from_slice(&self.n.to_le_bytes());
        self.n += 1;
        nonce
    }

    /// Encrypts `message` in place; its tag goes after it.
    pub fn encrypt(&mut self, message: &mut [u8]) -> [u8; TAG] {
        self.encrypt_with(&[], message)
    }

    fn encrypt_with(&mut self, ad: &[u8], message: &mut [u8]) -> [u8; TAG] {
        let nonce = self.nonce();
        // Only fails for messages over 256 GB.
        self.aead.encrypt_in_place_detached(&nonce, ad, message).map(Into::into).unwrap_or([0; TAG])
    }

    /// Decrypts `frame` (the message, then its tag) in place: the message's length, `None` if
    /// it was not encrypted with this key.
    pub fn decrypt(&mut self, frame: &mut [u8]) -> Option<usize> {
        self.decrypt_with(&[], frame)
    }

    fn decrypt_with(&mut self, ad: &[u8], frame: &mut [u8]) -> Option<usize> {
        let len = frame.len().checked_sub(TAG)?;
        let (message, tag) = frame.split_at_mut(len);
        let nonce = self.nonce();
        self.aead.decrypt_in_place_detached(&nonce, ad, message, Tag::from_slice(tag)).ok()?;
        Some(len)
    }
}

/// The handshake's running hash and chaining key.
struct Symmetric {
    ck: [u8; 32],
    h: [u8; 32],
}

impl Symmetric {
    fn new() -> Self {
        // The name is longer than a hash: hashed.
        let h = Sha256::digest(PROTOCOL).into();
        let mut s = Self { ck: h, h };
        s.mix_hash(PROLOGUE);
        s
    }

    fn mix_hash(&mut self, data: &[u8]) {
        self.h = Sha256::new().chain_update(self.h).chain_update(data).finalize().into();
    }

    /// Noise's HKDF: RFC 5869 with the chaining key as salt and no info.
    fn hkdf<const N: usize>(&mut self, ikm: &[u8]) -> [[u8; 32]; N] {
        let mut out = [[0; 32]; N];
        // 32 * N bytes, N ≤ 3: well under HKDF's limit.
        let _ = Hkdf::<Sha256>::new(Some(&self.ck), ikm).expand(&[], out.as_flattened_mut());
        out
    }

    fn mix_key(&mut self, ikm: &[u8]) -> Cipher {
        let [ck, k] = self.hkdf(ikm);
        self.ck = ck;
        Cipher::new(&k)
    }

    fn mix_key_and_hash(&mut self, ikm: &[u8]) {
        let [ck, h, _k] = self.hkdf(ikm);
        self.ck = ck;
        self.mix_hash(&h);
    }
}

/// Answers the client's handshake `message` with the shared key `psk` and a fresh random
/// `ephemeral` key: writes the reply, and returns the ciphers for messages from the client and
/// to it. `None` if the client does not have the key, or sent something else.
pub fn respond(
    psk: &[u8; 32],
    ephemeral: [u8; 32],
    message: &[u8],
    reply: &mut [u8; HANDSHAKE],
) -> Option<(Cipher, Cipher)> {
    let (their_key, tag) = message.split_first_chunk::<32>()?;
    let mut s = Symmetric::new();
    // -> psk, e: with a pre-shared key, the ephemeral keys are mixed in as keys too.
    s.mix_key_and_hash(psk);
    s.mix_hash(their_key);
    let mut tag: [u8; TAG] = tag.try_into().ok()?;
    s.mix_key(their_key).decrypt_with(&s.h, &mut tag)?;
    s.mix_hash(&tag);
    // <- e, ee
    let secret = SecretKey::new(ephemeral);
    let ours = secret.recover_public_key().ok()?;
    s.mix_hash(ours.as_slice());
    s.mix_key(ours.as_slice());
    let shared = PublicKey::new(*their_key).dh(&secret).ok()?;
    let tag = s.mix_key(shared.as_slice()).encrypt_with(&s.h, &mut []);
    reply[..32].copy_from_slice(ours.as_slice());
    reply[32..].copy_from_slice(&tag);
    let [from_client, to_client] = s.hkdf(&[]);
    Some((Cipher::new(&from_client), Cipher::new(&to_client)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session from the Python `noise` package (as aioesphomeapi uses it), with fixed keys:
    /// psk 00..1f, client ephemeral 11.., ours 22...
    fn hex<const N: usize>(s: &str) -> [u8; N] {
        core::array::from_fn(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
    }

    #[test]
    fn matches_the_reference() {
        let psk = core::array::from_fn(|i| i as u8);
        let hello: [u8; 48] =
            hex("7b4e909bbe7ffe44c465a220037d608ee35897d31ef972f07f74892cb0f73f13d1bc1a38b6793685dc7e2ffdf8ea0933");
        let mut reply = [0; HANDSHAKE];
        let (mut rx, mut tx) = respond(&psk, [0x22; 32], &hello, &mut reply).unwrap();
        assert_eq!(
            reply,
            hex::<48>(
                "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f207336b95933b321b1ae86673f689f5c9f"
            )
        );
        let mut frame: [u8; 22] = hex("082c88d6253e6e31953ac80af1f822b21a26c819e802");
        assert_eq!(rx.decrypt(&mut frame), Some(6));
        assert_eq!(&frame[..6], b"\x00\x01\x00\x02hi");
        for expected in ["d9483bf41933f7b67e3e3122b3a337e0a547509f", "471927a13be9261b3ac147b6600a85b167b42d8e"] {
            let mut message = if tx.n == 0 { *b"\x00\x02\x00\x00" } else { *b"\x00\x08\x00\x00" };
            let tag = tx.encrypt(&mut message);
            let mut sent = [0; 20];
            sent[..4].copy_from_slice(&message);
            sent[4..].copy_from_slice(&tag);
            assert_eq!(sent, hex::<20>(expected));
        }
    }

    #[test]
    fn refuses_another_key_and_tampering() {
        let hello: [u8; 48] =
            hex("7b4e909bbe7ffe44c465a220037d608ee35897d31ef972f07f74892cb0f73f13d1bc1a38b6793685dc7e2ffdf8ea0933");
        let mut reply = [0; HANDSHAKE];
        assert!(respond(&[0; 32], [0x22; 32], &hello, &mut reply).is_none(), "another key");
        let psk = core::array::from_fn(|i| i as u8);
        assert!(respond(&psk, [0x22; 32], &hello[..47], &mut reply).is_none(), "short");
        let (mut rx, _) = respond(&psk, [0x22; 32], &hello, &mut reply).unwrap();
        let mut frame: [u8; 22] = hex("082c88d6253e6e31953ac80af1f822b21a26c819e803");
        assert_eq!(rx.decrypt(&mut frame), None, "tampered");
    }
}
