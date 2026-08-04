// AYGENT — GOOGLE SERVICE-ACCOUNT AUTH.
//
// WHY THIS EXISTS: every other connector in the registry authenticates with a
// static credential the user pastes. Google can't. A service account holds an RSA
// private key and must SIGN a short-lived JWT assertion, then exchange it at
// Google's token endpoint for an access token good for an hour. That's real
// crypto, so it can't be expressed as a descriptor — this is the one piece of
// per-provider code the registry doesn't eliminate.
//
// WHY A SERVICE ACCOUNT AT ALL (Mason's call): the alternative is OAuth, which
// for Calendar scopes means submitting the app for Google's verification review
// before real users can connect. A service account sidesteps that entirely — no
// review, no consent screen, no browser loopback.
//
// THE TRADEOFF, which we surface in the UI rather than bury: a service account is
// a SEPARATE IDENTITY with its own email. It sees nothing until the user shares a
// calendar or Drive folder with that address. Every "why is my calendar empty"
// report traces to this, which is why it's setup step 3 on the card.
//
// TOKEN CACHE: tokens last ~1h. We cache in memory per key_ref and refresh a
// minute early. The private key never leaves the privileged Rust side, and the
// minted access token is never handed to the jailed daemon — same boundary as
// every other connector.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The fields we need out of a downloaded service-account JSON key file.
#[derive(Debug, Deserialize)]
struct ServiceAccountKey {
    client_email: String,
    private_key: String,
    #[serde(default)]
    token_uri: Option<String>,
}

#[derive(Debug, Serialize)]
struct Claims {
    iss: String,
    scope: String,
    aud: String,
    exp: u64,
    iat: u64,
}

/// The scopes we request. READ-ONLY: this connector's tools only read, so asking
/// for write scope would be requesting authority we never use — and a token is a
/// liability proportional to what it can do.
const SCOPES: &str = "https://www.googleapis.com/auth/calendar.readonly \
                      https://www.googleapis.com/auth/drive.readonly";

struct Cached {
    token: String,
    expires_at: u64,
}

fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static C: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validate a pasted service-account JSON and return the identity (its email).
/// Called at connect time so a wrong file (an OAuth client secret is the common
/// mix-up) fails in the dialog with a specific message.
pub fn identity_from_key(json: &str) -> Result<String, String> {
    let key: ServiceAccountKey = serde_json::from_str(json).map_err(|_| {
        "that doesn't look like a service-account key. Download the JSON key from a SERVICE ACCOUNT \
         (IAM & Admin → Service Accounts → Keys), not an OAuth client secret."
            .to_string()
    })?;
    if key.client_email.is_empty() || key.private_key.is_empty() {
        return Err("the key file is missing client_email or private_key.".into());
    }
    if !key.private_key.contains("PRIVATE KEY") {
        return Err("the private_key field doesn't contain a PEM key — the file may be truncated.".into());
    }
    Ok(key.client_email)
}

/// Mint (or reuse) an access token for this service account.
pub async fn access_token(key_json: &str, cache_key: &str) -> Result<String, String> {
    // Reuse a cached token until a minute before expiry.
    if let Ok(c) = cache().lock() {
        if let Some(hit) = c.get(cache_key) {
            if hit.expires_at > now_secs() + 60 {
                return Ok(hit.token.clone());
            }
        }
    }

    let key: ServiceAccountKey =
        serde_json::from_str(key_json).map_err(|e| format!("service account key: {e}"))?;
    let token_uri = key
        .token_uri
        .clone()
        .unwrap_or_else(|| "https://oauth2.googleapis.com/token".to_string());

    let iat = now_secs();
    let claims = Claims {
        iss: key.client_email.clone(),
        scope: SCOPES.split_whitespace().collect::<Vec<_>>().join(" "),
        aud: token_uri.clone(),
        exp: iat + 3600,
        iat,
    };

    let enc_key = jsonwebtoken::EncodingKey::from_rsa_pem(key.private_key.as_bytes())
        .map_err(|e| format!("reading the service account's private key failed: {e}"))?;
    let assertion = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &enc_key,
    )
    .map_err(|e| format!("signing the Google assertion failed: {e}"))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let resp = client
        .post(&token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &assertion),
        ])
        .send()
        .await
        .map_err(|e| format!("google token request: {e}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        // Google's error bodies are actually informative; pass them through
        // instead of collapsing to a status code.
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("error_description")
                    .or_else(|| v.get("error"))
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        return Err(format!(
            "Google refused the service account ({}): {detail}. If this says the API is disabled, \
             enable the Calendar and Drive APIs in that Cloud project.",
            status.as_u16()
        ));
    }

    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("google token decode: {e}"))?;
    let token = body
        .get("access_token")
        .and_then(|t| t.as_str())
        .ok_or("Google's response contained no access_token")?
        .to_string();
    let ttl = body.get("expires_in").and_then(|e| e.as_u64()).unwrap_or(3600);

    if let Ok(mut c) = cache().lock() {
        c.insert(
            cache_key.to_string(),
            Cached { token: token.clone(), expires_at: now_secs() + ttl },
        );
    }
    Ok(token)
}

/// Drop any cached token for this connection (on disconnect / reconnect).
pub fn forget(cache_key: &str) {
    if let Ok(mut c) = cache().lock() {
        c.remove(cache_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_oauth_client_secret_with_a_useful_message() {
        // The most common user mistake: downloading the wrong JSON from Cloud
        // console. A generic parse error would send them hunting; name the fix.
        let oauth_secret = r#"{"web":{"client_id":"x.apps.googleusercontent.com","client_secret":"y"}}"#;
        let err = identity_from_key(oauth_secret).unwrap_err();
        assert!(err.contains("SERVICE ACCOUNT"), "{err}");
    }

    #[test]
    fn rejects_a_truncated_key() {
        let bad = r#"{"client_email":"a@b.iam.gserviceaccount.com","private_key":"oops"}"#;
        let err = identity_from_key(bad).unwrap_err();
        assert!(err.contains("PEM"), "{err}");
    }

    #[test]
    fn accepts_a_well_formed_key_and_returns_the_identity() {
        let ok = r#"{"client_email":"agent@proj.iam.gserviceaccount.com",
                     "private_key":"-----BEGIN PRIVATE KEY-----\nMIIabc\n-----END PRIVATE KEY-----\n"}"#;
        assert_eq!(
            identity_from_key(ok).unwrap(),
            "agent@proj.iam.gserviceaccount.com"
        );
    }

    #[test]
    fn cache_expiry_is_respected() {
        forget("t1");
        if let Ok(mut c) = cache().lock() {
            c.insert("t1".into(), Cached { token: "stale".into(), expires_at: now_secs() + 10 });
        }
        // Within the 60s safety window => must NOT be treated as reusable.
        let reusable = cache()
            .lock()
            .map(|c| c.get("t1").map(|h| h.expires_at > now_secs() + 60).unwrap_or(false))
            .unwrap_or(false);
        assert!(!reusable, "a token expiring in 10s must not be reused");
        forget("t1");
    }
}
