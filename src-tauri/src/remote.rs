// AYGENT REMOTE — device-side client (the Mac end of masonlee.build/remote).
//
// ARCHITECTURE (v2 — ACCOUNT pairing, see context/plan-aygent-remote.md):
//   browser  ⇄  Supabase Realtime broadcast channel `remote:{user_id}`  ⇄  this
// Both ends dial OUT; nobody opens ports.
//
// CRYPTO MODEL (v2, Mason 08-05): ONE symmetric channel key per account.
// The Mac GENERATES it at pair time and registers it with the site; any
// browser the user is logged into fetches it over authed HTTPS and can talk
// immediately — no per-browser keypairs, no SAS, no link step. Trade-off
// (explicit, Mason's call): encryption gates on ACCOUNT AUTH rather than
// strict E2E-vs-relay — the site's DB holds the key. Payloads are still
// XChaCha20-Poly1305 sealed in transit and at rest in Realtime's pipeline;
// re-pair rotates the key; unpair deletes it everywhere.
//
// TRUST: this module runs on the PRIVILEGED Rust side. The channel key and
// device JWT live in the macOS keychain. What remote is ALLOWED to do is
// decided HERE — the server never grants capabilities, it only relays.

use base64::Engine;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::XChaCha20Poly1305;
use serde::{Deserialize, Serialize};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

// Keychain slots (namespaced like connection credentials).
const KC_SECRET: &str = "remote:device_secret"; // v2: the 32-byte channel key, b64
const KC_JWT: &str = "remote:device_jwt";
const KC_META: &str = "remote:meta"; // JSON: {user_id, device_id, channel, site}
const KC_ENABLED: &str = "remote:enabled"; // "0" = user toggled offline; absent/other = online

/// Max plaintext bytes per envelope chunk. Spike measured the Realtime cap at
/// ~250KB; 48KB pre-seal leaves generous headroom for the seal + JSON + b64.
pub const CHUNK_BYTES: usize = 48 * 1024;

// ---------------------------------------------------------------------------
// KEYS + PAIRING STATE
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteMeta {
    pub user_id: String,
    pub device_id: String,
    pub channel: String,
    pub site: String, // e.g. https://www.masonlee.build
}

/// Generate a FRESH 32-byte channel key and store it (re-pair = rotation).
fn fresh_channel_key() -> Result<[u8; 32], String> {
    use chacha20poly1305::aead::rand_core::RngCore;
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    crate::keychain::set_key(KC_SECRET, &B64.encode(key))?;
    Ok(key)
}

fn load_channel_key() -> Result<[u8; 32], String> {
    let b64 = crate::keychain::get_key(KC_SECRET)?;
    B64.decode(&b64)
        .map_err(|e| format!("channel key decode: {e}"))?
        .try_into()
        .map_err(|_| "channel key wrong length".to_string())
}

pub fn load_meta() -> Option<RemoteMeta> {
    let json = crate::keychain::get_key(KC_META).ok()?;
    serde_json::from_str(&json).ok()
}

pub fn load_jwt() -> Option<String> {
    crate::keychain::get_key(KC_JWT).ok()
}

pub fn is_paired() -> bool {
    load_meta().is_some() && load_jwt().is_some()
}

/// User preference: should the Mac hold its Realtime session open?
/// Default ON — pairing implies wanting to be reachable. OFF = paired but
/// silent: no socket, no heartbeat, until toggled back.
pub fn is_enabled() -> bool {
    crate::keychain::get_key(KC_ENABLED).map(|v| v != "0").unwrap_or(true)
}

pub fn set_enabled(on: bool) {
    let _ = crate::keychain::set_key(KC_ENABLED, if on { "1" } else { "0" });
}

/// Forget everything. Called from Settings ("Unpair") — the site row is
/// deleted separately; our key is gone here.
pub fn unpair() {
    let _ = crate::keychain::delete_key(KC_SECRET);
    let _ = crate::keychain::delete_key(KC_JWT);
    let _ = crate::keychain::delete_key(KC_META);
    let _ = crate::keychain::delete_key(KC_ENABLED);
}

// ---------------------------------------------------------------------------
// PAIRING — claim the code the user typed (Settings → AYGENT Remote).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ClaimResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    device_id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    device_jwt: Option<String>,
    #[serde(default)]
    channel: Option<String>,
}

/// Claim a pairing code against the site. Generates a FRESH channel key
/// (re-pair = rotation), registers it account-wide, stores jwt + meta.
/// After this succeeds, ANY logged-in browser can use the remote.
pub async fn pair(site: &str, code: &str, device_name: &str) -> Result<RemoteMeta, String> {
    let key = fresh_channel_key()?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http: {e}"))?;

    let resp = client
        .post(format!("{}/api/remote/claim", site.trim_end_matches('/')))
        .json(&serde_json::json!({
            "code": code.trim().to_uppercase(),
            "channel_key": B64.encode(key),
            "device_name": device_name,
        }))
        .send()
        .await
        .map_err(|e| format!("pairing request: {e}"))?;

    let body: ClaimResponse = resp.json().await.map_err(|e| format!("pairing decode: {e}"))?;
    if !body.ok {
        // A failed claim must not leave half-state behind.
        unpair();
        return Err(body.error.unwrap_or_else(|| "pairing failed".into()));
    }
    let meta = RemoteMeta {
        user_id: body.user_id.ok_or("missing user_id")?,
        device_id: body.device_id.ok_or("missing device_id")?,
        channel: body.channel.ok_or("missing channel")?,
        site: site.trim_end_matches('/').to_string(),
    };
    crate::keychain::set_key(KC_JWT, &body.device_jwt.ok_or("missing jwt")?)?;
    crate::keychain::set_key(KC_META, &serde_json::to_string(&meta).unwrap())?;
    Ok(meta)
}

// ---------------------------------------------------------------------------
// ENVELOPE — seal/open + chunking. Wire shape (broadcast event "env"):
//   { v:2, from:"dev"|"web", turn, seq, last, nonce, ct }
// XChaCha20-Poly1305 IETF AEAD with the account channel key. The browser
// side is libsodium's crypto_aead_xchacha20poly1305_ietf_* — the same
// construction (proven interop: examples/crypto_interop.rs).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    pub from: String,
    pub turn: String,
    pub seq: u32,
    pub last: bool,
    pub nonce: String,
    pub ct: String,
}

pub struct Sealer {
    cipher: XChaCha20Poly1305,
}

impl Sealer {
    /// Build from the account channel key in the keychain.
    pub fn new() -> Result<Self, String> {
        let key = load_channel_key()?;
        Ok(Self { cipher: XChaCha20Poly1305::new((&key).into()) })
    }

    /// Test-only: build from an explicit key (no keychain).
    pub fn from_key(key: [u8; 32]) -> Self {
        Self { cipher: XChaCha20Poly1305::new((&key).into()) }
    }

    /// Seal one plaintext into 1..N chunked envelopes.
    pub fn seal(&self, turn: &str, plaintext: &[u8]) -> Result<Vec<Envelope>, String> {
        let chunks: Vec<&[u8]> = if plaintext.is_empty() {
            vec![&[][..]]
        } else {
            plaintext.chunks(CHUNK_BYTES).collect()
        };
        let n = chunks.len();
        let mut out = Vec::with_capacity(n);
        for (i, chunk) in chunks.into_iter().enumerate() {
            let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
            let ct = self
                .cipher
                .encrypt(&nonce, chunk)
                .map_err(|_| "seal failed".to_string())?;
            out.push(Envelope {
                v: 2,
                from: "dev".into(),
                turn: turn.to_string(),
                seq: i as u32,
                last: i == n - 1,
                nonce: B64.encode(nonce),
                ct: B64.encode(ct),
            });
        }
        Ok(out)
    }

    /// Open one envelope's chunk.
    pub fn open(&self, env: &Envelope) -> Result<Vec<u8>, String> {
        let nonce_bytes = B64.decode(&env.nonce).map_err(|e| format!("nonce: {e}"))?;
        let ct = B64.decode(&env.ct).map_err(|e| format!("ct: {e}"))?;
        let nonce = chacha20poly1305::aead::generic_array::GenericArray::from_slice(&nonce_bytes);
        self.cipher
            .decrypt(nonce, ct.as_slice())
            .map_err(|_| "open failed (stale channel key? re-pair)".to_string())
    }
}

/// Reassembler for inbound chunked messages, keyed by turn id.
#[derive(Default)]
pub struct Reassembler {
    partial: std::collections::HashMap<String, Vec<(u32, Vec<u8>)>>,
}

impl Reassembler {
    /// Feed a decrypted chunk; returns the full plaintext when `last` arrives
    /// and all sequence numbers are present.
    pub fn feed(&mut self, env: &Envelope, plain: Vec<u8>) -> Option<Vec<u8>> {
        let parts = self.partial.entry(env.turn.clone()).or_default();
        parts.push((env.seq, plain));
        if !env.last {
            return None;
        }
        let expected = env.seq + 1;
        if (parts.len() as u32) < expected {
            return None; // out-of-order delivery — keep waiting
        }
        let mut parts = self.partial.remove(&env.turn)?;
        parts.sort_by_key(|(s, _)| *s);
        parts.dedup_by_key(|(s, _)| *s);
        Some(parts.into_iter().flat_map(|(_, b)| b).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealer() -> Sealer {
        Sealer::from_key([7u8; 32])
    }

    #[test]
    fn seal_open_roundtrip() {
        let s = sealer();
        let envs = s.seal("t1", b"hello remote").unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(s.open(&envs[0]).unwrap(), b"hello remote");
    }

    #[test]
    fn different_key_fails_open() {
        let a = sealer();
        let b = Sealer::from_key([8u8; 32]);
        let envs = a.seal("t1", b"secret").unwrap();
        assert!(b.open(&envs[0]).is_err(), "wrong key must not decrypt");
    }

    #[test]
    fn chunking_splits_and_reassembles() {
        let s = sealer();
        let big = vec![b'x'; CHUNK_BYTES * 2 + 100]; // 3 chunks
        let envs = s.seal("t2", &big).unwrap();
        assert_eq!(envs.len(), 3);
        assert!(envs[2].last && !envs[0].last);

        let mut re = Reassembler::default();
        // Out-of-order arrival: 1, 0, 2.
        let mut done = None;
        for &i in &[1usize, 0, 2] {
            let plain = s.open(&envs[i]).unwrap();
            done = re.feed(&envs[i], plain);
        }
        assert_eq!(done.expect("reassembled"), big);
    }

    #[test]
    fn tampered_ciphertext_fails_open() {
        let s = sealer();
        let mut envs = s.seal("t3", b"secret").unwrap();
        let mut ct = B64.decode(&envs[0].ct).unwrap();
        ct[0] ^= 0xFF;
        envs[0].ct = B64.encode(ct);
        assert!(s.open(&envs[0]).is_err(), "AEAD must reject tampering");
    }
}
