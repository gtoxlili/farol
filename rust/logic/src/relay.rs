//! The board's half of Paseo's encrypted relay connection (Paseo 0.14,
//! "protected" mode), as pure functions over frames.
//!
//! After the relay's WebSocket opens, the board sends `e2ee_hello` carrying
//! the first message of Noise_NK_25519_ChaChaPoly_SHA256 against the daemon's
//! static key (from the pairing QR); the daemon answers `e2ee_ready` with the
//! second. From then on every frame is one record: the frame cut into chunks,
//! each sealed with ChaCha20-Poly1305 and carrying its kind, FIRST/FINAL flags
//! and the frame length, then base64 in a WebSocket text message. The relay
//! sees only those records.
//!
//! The board offers no binary ciphertext, so every record arrives as text.
//! Source of truth: Paseo `packages/relay/src/{secure-channel.ts,noise/,
//! protected-records.ts}`. `protocol/fixtures/relay.json` holds vectors made
//! by Paseo's own code (`tools/relay_vectors.mjs`); the tests below check
//! this module against them.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use chacha20poly1305::aead::{AeadInOut, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce, Tag};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};
use x25519_dalek::{X25519_BASEPOINT_BYTES, x25519};

/// 32 bytes, so it is used as the initial hash as is (Noise §5.2).
const PROTOCOL_NAME: &[u8; 32] = b"Noise_NK_25519_ChaChaPoly_SHA256";
const PROLOGUE: &[u8] = b"paseo-relay-e2ee/1";
/// Handshake parameters: version 1, flags 0 (no binary ciphertext).
const PARAMS: [u8; 2] = [0x01, 0x00];
const PARAMS_KNOWN_FLAGS: u8 = 0x01;
const ENVELOPE_VERSION: u64 = 1;
const KEY_BYTES: usize = 32;
const TAG_BYTES: usize = 16;
/// kind(1) flags(1) totalLength(u32 BE) at the start of every chunk.
const HEADER_BYTES: usize = 6;
const LENGTH_PREFIX_BYTES: usize = 2;
const MAX_MESSAGE_BYTES: usize = 65_535;
const MAX_CHUNK_DATA: usize = MAX_MESSAGE_BYTES - TAG_BYTES - HEADER_BYTES;
const KIND_TEXT: u8 = 0x00;
const KIND_BINARY: u8 = 0x01;
const FLAG_FIRST: u8 = 0x01;
const FLAG_FINAL: u8 = 0x02;
/// Largest frame the board takes; the link refuses bigger messages before this.
const MAX_FRAME_BYTES: usize = 192 * 1024;

/// Why the encrypted channel cannot go on. The session closes the socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The daemon key from the pairing link is not a usable X25519 key.
    Key,
    /// The daemon's `e2ee_ready` is malformed, unencrypted (an older daemon),
    /// or fails authentication (the daemon's key changed: pair again).
    Handshake,
    /// A record failed authentication or framing.
    Record,
}

// ------------------------------------------------------------------ Noise

struct Cipher {
    key: [u8; KEY_BYTES],
    n: u64,
}

impl Cipher {
    fn nonce(&self) -> Nonce {
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&self.n.to_le_bytes());
        Nonce::from(nonce)
    }

    /// Encrypts `data` in place and appends the tag.
    fn seal(&mut self, ad: &[u8], data: &mut Vec<u8>) {
        let aead = ChaCha20Poly1305::new(&self.key.into());
        let nonce = self.nonce();
        // Fails only past 256 GiB per message; frames are far smaller.
        let tag = aead
            .encrypt_inout_detached(&nonce, ad, data.as_mut_slice().into())
            .unwrap_or_default();
        data.extend_from_slice(&tag);
        self.n = self.n.wrapping_add(1);
    }

    /// Decrypts `data` (ciphertext and tag) in place, dropping the tag. The
    /// counter moves only when the tag verifies, as in Paseo.
    fn open(&mut self, ad: &[u8], data: &mut Vec<u8>) -> Result<(), ()> {
        if self.n == u64::MAX {
            return Err(());
        }
        let body = data.len().checked_sub(TAG_BYTES).ok_or(())?;
        let tag = Tag::try_from(&data[body..]).map_err(|_| ())?;
        data.truncate(body);
        let aead = ChaCha20Poly1305::new(&self.key.into());
        aead.decrypt_inout_detached(&self.nonce(), ad, data.as_mut_slice().into(), &tag)
            .map_err(|_| ())?;
        self.n += 1;
        Ok(())
    }
}

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().into()
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    // Any key length is valid for HMAC.
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC takes any key");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

/// Noise §4.3 HKDF with two outputs.
fn hkdf2(chaining_key: &[u8; 32], input: &[u8]) -> ([u8; 32], [u8; 32]) {
    let temp = hmac(chaining_key, &[input]);
    let first = hmac(&temp, &[&[0x01]]);
    let second = hmac(&temp, &[&first, &[0x02]]);
    (first, second)
}

/// X25519, refusing the all-zero result of a low-order point.
fn dh(secret: &[u8; 32], public: &[u8; 32]) -> Result<[u8; 32], Error> {
    let shared = x25519(*secret, *public);
    if shared == [0; 32] {
        Err(Error::Key)
    } else {
        Ok(shared)
    }
}

/// Noise §5.2 SymmetricState.
struct Symmetric {
    h: [u8; 32],
    ck: [u8; 32],
    cipher: Option<Cipher>,
}

impl Symmetric {
    fn new() -> Self {
        Self {
            h: *PROTOCOL_NAME,
            ck: *PROTOCOL_NAME,
            cipher: None,
        }
    }

    fn mix_hash(&mut self, data: &[u8]) {
        self.h = sha256(&[&self.h, data]);
    }

    fn mix_key(&mut self, input: &[u8]) {
        let (ck, key) = hkdf2(&self.ck, input);
        self.ck = ck;
        self.cipher = Some(Cipher { key, n: 0 });
    }

    fn encrypt_and_hash(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let mut data = plaintext.to_vec();
        if let Some(cipher) = &mut self.cipher {
            cipher.seal(&self.h, &mut data);
        }
        self.mix_hash(&data);
        data
    }

    fn decrypt_and_hash(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, Error> {
        let mut data = ciphertext.to_vec();
        if let Some(cipher) = &mut self.cipher {
            cipher
                .open(&self.h, &mut data)
                .map_err(|()| Error::Handshake)?;
        }
        self.mix_hash(ciphertext);
        Ok(data)
    }

    /// The two transport keys: the initiator sends with the first.
    fn split(&self) -> (Cipher, Cipher) {
        let (send, receive) = hkdf2(&self.ck, &[]);
        (Cipher { key: send, n: 0 }, Cipher { key: receive, n: 0 })
    }
}

// ------------------------------------------------------------------ handshake

/// The board's side of the handshake, from `e2ee_hello` to `e2ee_ready`.
pub struct Initiator {
    symmetric: Symmetric,
    ephemeral: [u8; 32],
    hello: String,
}

impl Initiator {
    /// Starts a handshake against `daemon_key` with two fresh random secrets:
    /// the Noise ephemeral, and the routing key the relay checks in `key`
    /// (it must be a valid X25519 public key; nothing else uses it).
    pub fn start(
        daemon_key: &[u8; 32],
        ephemeral: [u8; 32],
        routing: [u8; 32],
    ) -> Result<Self, Error> {
        let mut symmetric = Symmetric::new();
        symmetric.mix_hash(PROLOGUE);
        symmetric.mix_hash(daemon_key);
        // -> e, es
        let public = x25519(ephemeral, X25519_BASEPOINT_BYTES);
        symmetric.mix_hash(&public);
        symmetric.mix_key(&dh(&ephemeral, daemon_key)?);
        let mut message = public.to_vec();
        message.extend_from_slice(&symmetric.encrypt_and_hash(&PARAMS));
        // Field order as Paseo's client writes it; the retries resend these bytes.
        let hello = format!(
            r#"{{"type":"e2ee_hello","key":"{}","capabilities":{{"binaryCiphertext":false}},"protected":{{"v":{ENVELOPE_VERSION},"message":"{}"}}}}"#,
            encode(&x25519(routing, X25519_BASEPOINT_BYTES)),
            encode(&message),
        );
        Ok(Self {
            symmetric,
            ephemeral,
            hello,
        })
    }

    /// The `e2ee_hello` text frame, sent until the daemon answers.
    pub fn hello(&self) -> &str {
        &self.hello
    }

    /// Completes the handshake with the daemon's `e2ee_ready` frame.
    pub fn finish(mut self, ready: &Value) -> Result<Channel, Error> {
        let envelope = &ready["protected"];
        // A ready without `protected` is the legacy mode, which the board refuses.
        if envelope["v"].as_u64() != Some(ENVELOPE_VERSION) {
            return Err(Error::Handshake);
        }
        let message = envelope["message"]
            .as_str()
            .and_then(decode)
            .ok_or(Error::Handshake)?;
        if message.len() < KEY_BYTES || message.len() > MAX_MESSAGE_BYTES {
            return Err(Error::Handshake);
        }
        // <- e, ee
        let (remote, payload) = message.split_at(KEY_BYTES);
        let remote: [u8; 32] = remote.try_into().map_err(|_| Error::Handshake)?;
        self.symmetric.mix_hash(&remote);
        let shared = dh(&self.ephemeral, &remote).map_err(|_| Error::Handshake)?;
        self.ephemeral = [0; 32];
        self.symmetric.mix_key(&shared);
        let params = self.symmetric.decrypt_and_hash(payload)?;
        // Version 1, and no flag the board did not offer.
        match params.as_slice() {
            [0x01, flags] if flags & !PARAMS_KNOWN_FLAGS == 0 && flags & 0x01 == 0 => {}
            _ => return Err(Error::Handshake),
        }
        let (send, receive) = self.symmetric.split();
        Ok(Channel { send, receive })
    }
}

// ------------------------------------------------------------------ records

/// An open channel: seals the board's frames and opens the daemon's.
pub struct Channel {
    send: Cipher,
    receive: Cipher,
}

/// Wire bytes of a record carrying `frame` bytes.
fn record_bytes(frame: usize) -> usize {
    let chunks = frame.div_ceil(MAX_CHUNK_DATA).max(1);
    frame + chunks * (LENGTH_PREFIX_BYTES + HEADER_BYTES + TAG_BYTES)
}

impl Channel {
    /// One text frame as the base64 text message the relay carries.
    pub fn seal(&mut self, text: &str) -> String {
        let frame = text.as_bytes();
        let total = u32::try_from(frame.len()).unwrap_or(u32::MAX).to_be_bytes();
        let chunks = frame.len().div_ceil(MAX_CHUNK_DATA).max(1);
        let mut record = Vec::with_capacity(record_bytes(frame.len()));
        for index in 0..chunks {
            let start = index * MAX_CHUNK_DATA;
            let data = &frame[start..frame.len().min(start + MAX_CHUNK_DATA)];
            let mut flags = 0;
            if index == 0 {
                flags |= FLAG_FIRST;
            }
            if index == chunks - 1 {
                flags |= FLAG_FINAL;
            }
            let mut chunk = Vec::with_capacity(HEADER_BYTES + data.len() + TAG_BYTES);
            chunk.extend_from_slice(&[KIND_TEXT, flags]);
            chunk.extend_from_slice(&total);
            chunk.extend_from_slice(data);
            self.send.seal(&[], &mut chunk);
            let length = u16::try_from(chunk.len()).unwrap_or(u16::MAX);
            record.extend_from_slice(&length.to_be_bytes());
            record.extend_from_slice(&chunk);
        }
        encode(&record)
    }

    /// One text message from the daemon: the text frame it carries, or `None`
    /// for a binary frame (the board subscribes to none). Any failure ends
    /// the channel.
    pub fn open(&mut self, message: &str) -> Result<Option<String>, Error> {
        let record = decode(message).ok_or(Error::Record)?;
        let mut rest = record.as_slice();
        let mut frame: Vec<u8> = Vec::new();
        let mut kind = KIND_TEXT;
        let (mut total, mut expected, mut index) = (0usize, 0usize, 0usize);
        while !rest.is_empty() {
            if index > 0 && index >= expected {
                return Err(Error::Record);
            }
            let (length, tail) = rest
                .split_first_chunk::<LENGTH_PREFIX_BYTES>()
                .ok_or(Error::Record)?;
            let length = usize::from(u16::from_be_bytes(*length));
            if !(TAG_BYTES + HEADER_BYTES..=MAX_MESSAGE_BYTES).contains(&length) {
                return Err(Error::Record);
            }
            let (sealed, tail) = tail.split_at_checked(length).ok_or(Error::Record)?;
            rest = tail;
            let mut chunk = sealed.to_vec();
            self.receive
                .open(&[], &mut chunk)
                .map_err(|()| Error::Record)?;
            let flags = chunk[1];
            if flags & !(FLAG_FIRST | FLAG_FINAL) != 0 || chunk[0] > KIND_BINARY {
                return Err(Error::Record);
            }
            let claimed = u32::from_be_bytes([chunk[2], chunk[3], chunk[4], chunk[5]]) as usize;
            if index == 0 {
                // The claimed length fixes the record's size: check before allocating.
                if flags & FLAG_FIRST == 0
                    || claimed > MAX_FRAME_BYTES
                    || record_bytes(claimed) != record.len()
                {
                    return Err(Error::Record);
                }
                kind = chunk[0];
                total = claimed;
                expected = claimed.div_ceil(MAX_CHUNK_DATA).max(1);
                frame.reserve_exact(total);
            } else if flags & FLAG_FIRST != 0 || chunk[0] != kind || claimed != total {
                return Err(Error::Record);
            }
            let last = index == expected - 1;
            let size = if last {
                total - index * MAX_CHUNK_DATA
            } else {
                MAX_CHUNK_DATA
            };
            if (flags & FLAG_FINAL != 0) != last || chunk.len() - HEADER_BYTES != size {
                return Err(Error::Record);
            }
            frame.extend_from_slice(&chunk[HEADER_BYTES..]);
            index += 1;
        }
        if index == 0 || index != expected || frame.len() != total {
            return Err(Error::Record);
        }
        if kind == KIND_BINARY {
            return Ok(None);
        }
        String::from_utf8(frame)
            .map(Some)
            .map_err(|_| Error::Record)
    }
}

/// The daemon's end of a channel as it is right after the handshake, for
/// tests that play the daemon.
#[cfg(test)]
impl Channel {
    pub(crate) fn mirror(&self) -> Self {
        Self {
            send: Cipher {
                key: self.receive.key,
                n: 0,
            },
            receive: Cipher {
                key: self.send.key,
                n: 0,
            },
        }
    }
}

// ------------------------------------------------------------------ base64

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding, as Paseo writes it.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let b = [
            group[0],
            group.get(1).copied().unwrap_or(0),
            group.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= group.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Base64 as Paseo reads it: standard or URL-safe letters, padding optional.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // A lone leftover character carries no whole byte.
    (bits < 6).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../protocol/fixtures/relay.json")).unwrap()
    }

    fn hex32(text: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    fn daemon_key(v: &Value) -> [u8; 32] {
        decode(v["daemonKeyB64"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap()
    }

    fn handshake(v: &Value) -> Channel {
        let start = Initiator::start(
            &daemon_key(v),
            hex32(v["boardEphemeralSecret"].as_str().unwrap()),
            hex32(v["boardRoutingSecret"].as_str().unwrap()),
        )
        .unwrap();
        let ready: Value =
            serde_json::from_str(v["handshake"]["readyText"].as_str().unwrap()).unwrap();
        start.finish(&ready).unwrap()
    }

    #[test]
    fn the_hello_is_byte_for_byte_what_paseo_builds() {
        let v = vectors();
        let start = Initiator::start(
            &daemon_key(&v),
            hex32(v["boardEphemeralSecret"].as_str().unwrap()),
            hex32(v["boardRoutingSecret"].as_str().unwrap()),
        )
        .unwrap();
        assert_eq!(start.hello(), v["handshake"]["helloText"].as_str().unwrap());
    }

    #[test]
    fn the_daemons_ready_opens_the_same_keys_as_paseo() {
        let v = vectors();
        let channel = handshake(&v);
        assert_eq!(
            channel.send.key,
            hex32(v["handshake"]["sendKey"].as_str().unwrap())
        );
        assert_eq!(
            channel.receive.key,
            hex32(v["handshake"]["receiveKey"].as_str().unwrap())
        );
    }

    #[test]
    fn records_match_paseo_in_both_directions() {
        let v = vectors();
        let mut channel = handshake(&v);
        let out = &v["boardToDaemon"];
        assert_eq!(
            channel.seal(out["plaintext"].as_str().unwrap()),
            out["frame"].as_str().unwrap()
        );
        for frame in v["daemonToBoard"].as_array().unwrap() {
            assert_eq!(
                channel
                    .open(frame["frame"].as_str().unwrap())
                    .unwrap()
                    .as_deref(),
                frame["plaintext"].as_str()
            );
        }
    }

    #[test]
    fn a_tampered_or_replayed_record_ends_the_channel() {
        let v = vectors();
        let first = v["daemonToBoard"][0]["frame"].as_str().unwrap();
        let mut channel = handshake(&v);
        let mut bytes = decode(first).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(channel.open(&encode(&bytes)), Err(Error::Record));
        // The counter did not move: the genuine record still opens, once.
        assert!(channel.open(first).unwrap().is_some());
        assert_eq!(channel.open(first), Err(Error::Record));
    }

    #[test]
    fn a_ready_from_another_key_or_without_protection_is_refused() {
        let v = vectors();
        let mut other = daemon_key(&v);
        other[0] ^= 0x40;
        let ready: Value =
            serde_json::from_str(v["handshake"]["readyText"].as_str().unwrap()).unwrap();
        let fresh = |key: &[u8; 32]| {
            Initiator::start(
                key,
                hex32(v["boardEphemeralSecret"].as_str().unwrap()),
                hex32(v["boardRoutingSecret"].as_str().unwrap()),
            )
        };
        assert!(matches!(
            fresh(&other).unwrap().finish(&ready),
            Err(Error::Handshake)
        ));
        let legacy = serde_json::json!({"type": "e2ee_ready", "capabilities": {}});
        assert!(matches!(
            fresh(&daemon_key(&v)).unwrap().finish(&legacy),
            Err(Error::Handshake)
        ));
        assert!(matches!(fresh(&[0; 32]), Err(Error::Key)));
    }

    #[test]
    fn long_frames_span_chunks_and_round_trip() {
        // Both directions share the framing: a channel whose keys are swapped
        // reads what the other seals.
        let v = vectors();
        let mut board = handshake(&v);
        let mut daemon = board.mirror();
        let text: String = (0..MAX_CHUNK_DATA + 1000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let sealed = board.seal(&text);
        assert_eq!(decode(&sealed).unwrap().len(), record_bytes(text.len()));
        assert_eq!(
            daemon.open(&sealed).unwrap().as_deref(),
            Some(text.as_str())
        );
    }

    #[test]
    fn base64_reads_both_alphabets_with_or_without_padding() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", &[0xfb, 0xff, 0xbf]] {
            let text = encode(bytes);
            assert_eq!(decode(&text).as_deref(), Some(bytes));
            let url = text
                .trim_end_matches('=')
                .replace('+', "-")
                .replace('/', "_");
            assert_eq!(decode(&url).as_deref(), Some(bytes));
        }
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(decode("a"), None);
        assert_eq!(decode("ab c"), None);
    }
}
