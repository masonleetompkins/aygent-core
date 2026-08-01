// AYGENT — Whisper transcription (UI task #6, Mason 08-01). OpenAI's
// audio/transcriptions endpoint. Available when an OpenAI key is in the
// Keychain AND the Whisper tool is enabled in Tools (per folder).
//
// Two callers:
//  - the AGENT tool "transcribe_audio" (file in the jail -> text), dispatched
//    from exec_tool like generate_pdf/fetch_url.
//  - the UI mic button (task #7) via the `transcribe_audio_b64` command
//    (recorded blob -> text into the input box). Both funnel here.
//
// The daemon has no network; the request runs on the privileged Rust side,
// key straight from the Keychain, never in any env or the repo.

use crate::keychain;

const OPENAI_TRANSCRIBE_URL: &str = "https://api.openai.com/v1/audio/transcriptions";
/// 25 MB — OpenAI's documented per-file limit for this endpoint.
const MAX_BYTES: usize = 25 * 1024 * 1024;

/// Transcribe raw audio bytes. `filename` matters: the API infers the container
/// from its extension (webm/mp3/wav/m4a/ogg/flac...).
pub async fn transcribe_bytes(bytes: Vec<u8>, filename: &str) -> Result<String, String> {
    if bytes.is_empty() { return Err("no audio data".into()); }
    if bytes.len() > MAX_BYTES {
        return Err(format!("audio is {:.1} MB — Whisper's limit is 25 MB; split it or compress", bytes.len() as f64 / 1_048_576.0));
    }
    let key = keychain::get_key("openai")
        .map_err(|_| "no OpenAI key set — add one in Settings to use Whisper".to_string())?;

    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str("application/octet-stream")
        .map_err(|e| format!("mime: {e}"))?;
    let form = reqwest::multipart::Form::new()
        .text("model", "whisper-1")
        .part("file", part);

    let resp = reqwest::Client::new()
        .post(OPENAI_TRANSCRIBE_URL)
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("whisper request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("whisper {status}: {}", body.chars().take(300).collect::<String>()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("whisper parse: {e}"))?;
    v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
        .ok_or_else(|| "whisper returned no text".into())
}

/// UI mic path (task #7): base64-encoded blob from the WebView recorder.
#[tauri::command]
pub async fn transcribe_audio_b64(b64: String, filename: String) -> Result<String, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| format!("bad base64 audio: {e}"))?;
    transcribe_bytes(bytes, &filename).await
}
