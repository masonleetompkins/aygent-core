// AYGENT — VIDEO v0.3: `aygent-media://` — jailed, range-capable media serving.
//
// The Video tab's <video>/<audio>/<img> elements need real bytes to play real
// footage. They get them ONLY through this scheme, which resolves every request
// against the agent's jail (broker) or the project's assets manifest:
//
//   aygent-media://localhost/<agentId>/<project>/asset/<assetId>   the media file
//   aygent-media://localhost/<agentId>/<project>/cache/<file>      .cache/ thumbs, frames
//   aygent-media://localhost/<agentId>/<project>/render/<file>     renders/ outputs
//   aygent-media://localhost/<agentId>/<project>/lut/<file>        luts/ (UI reads .cube for preview)
//
// Only files INSIDE those three folders (no '..', single component) or referenced
// by an asset row are served. Range requests are honored (206) so seeking works
// without reading a 4 GB file into memory. Read-only; nothing is written.

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, UriSchemeContext};

use crate::broker::{self, Broker};
use crate::video;

pub const SCHEME: &str = "aygent-media";

fn mime_for(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("flac") => "audio/flac",
        Some("ogg") => "audio/ogg",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("cube" | "txt" | "ass" | "srt") => "text/plain; charset=utf-8",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

fn err(status: StatusCode, msg: &str) -> Response<Vec<u8>> {
    Response::builder().status(status).header(header::CONTENT_TYPE, "text/plain").body(msg.as_bytes().to_vec()).unwrap_or_default()
}

fn one_component(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains('/') && !s.contains('\\') && !s.contains('\0')
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) { out.push(v); i += 3; continue; }
        }
        out.push(b[i]); i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Resolve a request path to a readable file, or None.
fn resolve(broker: &Broker, path: &str) -> Option<std::path::PathBuf> {
    let parts: Vec<String> = path.trim_start_matches('/').split('/').map(percent_decode).collect();
    if parts.len() != 4 { return None; }
    let (agent, project, kind, name) = (&parts[0], &parts[1], &parts[2], &parts[3]);
    if !one_component(agent) || !video::slug_ok(project) || !one_component(name) { return None; }
    match kind.as_str() {
        "asset" => {
            let m = video::load_manifest(broker, agent, project);
            let a = m.assets.iter().find(|a| &a.id == name)?;
            video::asset_abs(broker, agent, project, a).ok()
        }
        "cache" | "render" | "lut" => {
            let dir = match kind.as_str() { "cache" => ".cache", "render" => "renders", _ => "luts" };
            let rel = video::rel(project, &format!("{dir}/{name}"));
            let p = broker.resolve(agent, &rel, broker::Mode::Read).ok()?;
            if p.is_file() { Some(p) } else { None }
        }
        _ => None,
    }
}

fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let start: u64 = if a.is_empty() { let n: u64 = b.parse().ok()?; return Some((len.saturating_sub(n), len - 1)); } else { a.parse().ok()? };
    let end: u64 = if b.is_empty() { len - 1 } else { b.parse::<u64>().ok()?.min(len - 1) };
    if start > end || start >= len { return None; }
    Some((start, end))
}

const CHUNK_CAP: u64 = 8 * 1024 * 1024; // serve at most 8 MB per range response; WebKit re-requests

pub fn handle(ctx: UriSchemeContext<'_, tauri::Wry>, req: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let app = ctx.app_handle();
    let Some(broker) = app.try_state::<Arc<Broker>>() else { return err(StatusCode::SERVICE_UNAVAILABLE, "broker not ready") };
    let path = req.uri().path().to_string();
    let Some(file) = resolve(&broker, &path) else { return err(StatusCode::NOT_FOUND, "not found") };
    let Ok(mut f) = std::fs::File::open(&file) else { return err(StatusCode::NOT_FOUND, "cannot open") };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mime = mime_for(&file);
    let range = req.headers().get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|h| parse_range(h, len));

    let (start, end, status) = match range {
        Some((s, e)) => (s, e.min(s + CHUNK_CAP - 1), StatusCode::PARTIAL_CONTENT),
        None if len > CHUNK_CAP && (mime.starts_with("video/") || mime.starts_with("audio/")) => (0, CHUNK_CAP - 1, StatusCode::PARTIAL_CONTENT),
        None => (0, len.saturating_sub(1), StatusCode::OK),
    };
    let n = if len == 0 { 0 } else { end - start + 1 };
    let mut buf = vec![0u8; n as usize];
    if n > 0 {
        if f.seek(SeekFrom::Start(start)).is_err() { return err(StatusCode::INTERNAL_SERVER_ERROR, "seek"); }
        if f.read_exact(&mut buf).is_err() { return err(StatusCode::INTERNAL_SERVER_ERROR, "read"); }
    }
    let mut b = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, n.to_string())
        .header(header::CACHE_CONTROL, if path.contains("/cache/") { "no-cache" } else { "private, max-age=3600" })
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    if status == StatusCode::PARTIAL_CONTENT { b = b.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}")); }
    b.body(buf).unwrap_or_else(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "response"))
}

