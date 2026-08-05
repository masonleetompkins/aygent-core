// AYGENT REMOTE — device-side client (the Mac end of masonlee.build/remote).
//
// ARCHITECTURE (see Cleo/context/plan-aygent-remote.md):
//   browser  ⇄  Supabase Realtime broadcast channel `remote:{user_id}`  ⇄  this
// Both ends dial OUT; nobody opens ports. Payloads are E2E-sealed with
// crypto_box (X25519 + XChaCha20-Poly1305): the relay and Mason's server carry
// ciphertext only. Public keys are exchanged through the site's device row at
// pair time; a 6-digit SAS derived from both pubkeys is confirmed on both
// screens to rule out a key-swapping relay.
//
// TRUST: this module runs on the PRIVILEGED Rust side. The device secret key
// and device JWT live in the macOS keychain. What remote is ALLOWED to do is
// decided HERE (capability manifest sent at hello) — the server never grants
// capabilities, it only relays.

use base64::Engine;
use crypto_box::aead::{Aead, AeadCore, OsRng};
use crypto_box::{ChaChaBox, PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

// Keychain slots (namespaced like connection credentials).
const KC_SECRET: &str = "remote:device_secret";
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

pub struct DeviceKeys {
    pub secret: SecretKey,
    pub public_b64: String,
}

/// Load the device keypair from the keychain, or create + store a fresh one.
/// Re-pairing intentionally rotates: `pair()` always generates new keys.
fn fresh_keys() -> Result<DeviceKeys, String> {
    let secret = SecretKey::generate(&mut OsRng);
    let public_b64 = B64.encode(secret.public_key().as_bytes());
    crate::keychain::set_key(KC_SECRET, &B64.encode(secret.to_bytes()))?;
    Ok(DeviceKeys { secret, public_b64 })
}

fn load_keys() -> Result<DeviceKeys, String> {
    let b64 = crate::keychain::get_key(KC_SECRET)?;
    let bytes: [u8; 32] = B64
        .decode(&b64)
        .map_err(|e| format!("device key decode: {e}"))?
        .try_into()
        .map_err(|_| "device key wrong length".to_string())?;
    let secret = SecretKey::from(bytes);
    let public_b64 = B64.encode(secret.public_key().as_bytes());
    Ok(DeviceKeys { secret, public_b64 })
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

/// Forget everything. Called from Settings ("Unpair") — the browser's next
/// exchange has nothing to target, and our secret is gone.
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

/// Claim a pairing code against the site. Generates FRESH keys (re-pair =
/// rotation), registers the public key, stores jwt + meta in the keychain.
pub async fn pair(site: &str, code: &str, device_name: &str) -> Result<RemoteMeta, String> {
    let keys = fresh_keys()?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http: {e}"))?;

    let resp = client
        .post(format!("{}/api/remote/claim", site.trim_end_matches('/')))
        .json(&serde_json::json!({
            "code": code.trim().to_uppercase(),
            "device_pubkey": keys.public_b64,
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
// SAS — 6-digit short authentication string, shown on BOTH screens.
// Derived from both public keys sorted, so both ends compute the same number
// and a relay that swapped either key changes it.
// ---------------------------------------------------------------------------

pub fn sas_code(device_pub_b64: &str, browser_pub_b64: &str) -> String {
    use sha2::{Digest, Sha256};
    let (a, b) = if device_pub_b64 <= browser_pub_b64 {
        (device_pub_b64, browser_pub_b64)
    } else {
        (browser_pub_b64, device_pub_b64)
    };
    let d = Sha256::digest(format!("aygent-remote-sas:{a}:{b}").as_bytes());
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

// ---------------------------------------------------------------------------
// ENVELOPE — seal/open + chunking. Wire shape (broadcast event "env"):
//   { v:1, from:"dev"|"web", turn, seq, last, nonce, ct }
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
    boxer: ChaChaBox,
}

impl Sealer {
    /// Build from our secret + the browser's public key (fetched from the
    /// device row after the browser publishes it).
    pub fn new(browser_pub_b64: &str) -> Result<Self, String> {
        let keys = load_keys()?;
        let bytes: [u8; 32] = B64
            .decode(browser_pub_b64)
            .map_err(|e| format!("browser key decode: {e}"))?
            .try_into()
            .map_err(|_| "browser key wrong length".to_string())?;
        Ok(Self { boxer: ChaChaBox::new(&PublicKey::from(bytes), &keys.secret) })
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
            let nonce = ChaChaBox::generate_nonce(&mut OsRng);
            let ct = self
                .boxer
                .encrypt(&nonce, chunk)
                .map_err(|_| "seal failed".to_string())?;
            out.push(Envelope {
                v: 1,
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
        let nonce = crypto_box::aead::generic_array::GenericArray::from_slice(&nonce_bytes);
        self.boxer
            .decrypt(nonce, ct.as_slice())
            .map_err(|_| "open failed (wrong key? re-pair)".to_string())
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
        if parts.len() as u32 != expected {
            // Missing chunks: keep waiting (out-of-order delivery) — but if the
            // last flag arrived and count exceeds expected, something's wrong.
            if (parts.len() as u32) < expected {
                return None;
            }
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

    fn pair_of_sealers() -> (Sealer, ChaChaBox, String) {
        // Simulate both ends without the keychain: device sealer built by hand.
        let dev = SecretKey::generate(&mut OsRng);
        let web = SecretKey::generate(&mut OsRng);
        let dev_sealer = Sealer { boxer: ChaChaBox::new(&web.public_key(), &dev) };
        let web_boxer = ChaChaBox::new(&dev.public_key(), &web);
        (dev_sealer, web_boxer, B64.encode(web.public_key().as_bytes()))
    }

    #[test]
    fn seal_open_roundtrip() {
        let (dev, web, _) = pair_of_sealers();
        let envs = dev.seal("t1", b"hello remote").unwrap();
        assert_eq!(envs.len(), 1);
        let nonce_bytes = B64.decode(&envs[0].nonce).unwrap();
        let ct = B64.decode(&envs[0].ct).unwrap();
        let nonce = crypto_box::aead::generic_array::GenericArray::from_slice(&nonce_bytes);
        let plain = web.decrypt(nonce, ct.as_slice()).unwrap();
        assert_eq!(plain, b"hello remote");
    }

    #[test]
    fn chunking_splits_and_reassembles() {
        let (dev, _, _) = pair_of_sealers();
        let big = vec![b'x'; CHUNK_BYTES * 2 + 100]; // 3 chunks
        let envs = dev.seal("t2", &big).unwrap();
        assert_eq!(envs.len(), 3);
        assert!(envs[2].last && !envs[0].last);

        // Reassemble using the DEVICE's own open (self-test of shapes): build a
        // web-side sealer to open dev-sealed envelopes.
        let mut re = Reassembler::default();
        // Simulate out-of-order arrival: 1, 0, 2.
        let order = [1usize, 0, 2];
        let mut done = None;
        for &i in &order {
            // NOTE: in production the WEB side opens dev envelopes; here we
            // shortcut by re-opening with a mirrored boxer inside the test
            // above. For reassembly we only need the plaintext chunks:
            let plain = big
                [i * CHUNK_BYTES..((i + 1) * CHUNK_BYTES).min(big.len())]
                .to_vec();
            done = re.feed(&envs[i], plain);
        }
        assert_eq!(done.expect("reassembled"), big);
    }

    #[test]
    fn sas_is_stable_and_order_independent() {
        let a = "AAAApubkeyAAAA";
        let b = "BBBBpubkeyBBBB";
        assert_eq!(sas_code(a, b), sas_code(b, a));
        assert_eq!(sas_code(a, b).len(), 6);
        assert_ne!(sas_code(a, b), sas_code(a, "CCCC"), "key swap must change the SAS");
    }

    #[test]
    fn tampered_ciphertext_fails_open() {
        let (dev, web, _) = pair_of_sealers();
        let mut envs = dev.seal("t3", b"secret").unwrap();
        // Flip a byte in the ciphertext.
        let mut ct = B64.decode(&envs[0].ct).unwrap();
        ct[0] ^= 0xFF;
        envs[0].ct = B64.encode(ct);
        let nonce_bytes = B64.decode(&envs[0].nonce).unwrap();
        let ct2 = B64.decode(&envs[0].ct).unwrap();
        let nonce = crypto_box::aead::generic_array::GenericArray::from_slice(&nonce_bytes);
        assert!(web.decrypt(nonce, ct2.as_slice()).is_err(), "AEAD must reject tampering");
    }
}
