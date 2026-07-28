// AYGENT — Web fetch (M1.9, first real agent tool).
//
// THE JAIL BOUNDARY: the Node daemon runs under a Seatbelt profile with NO
// network. So an agent that wants to read the web CANNOT open a socket itself —
// exactly like file I/O, the privileged Rust side makes the request and hands
// back only the extracted text. The agent's reach to the internet is mediated,
// inspectable, and (later) policy-gated. This is the same trust model as the
// path broker, applied to the network.
//
// Slice scope: GET a URL, follow redirects, cap the body, strip HTML to readable
// text. No JS execution, no browser — a lightweight reader, not automation. That
// covers the real use case (a scheduled briefing agent reading articles/APIs).

/// Result of a fetch: the extracted text + a little metadata for the model.
pub struct FetchResult {
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub text: String,
    pub truncated: bool,
}

/// Hard cap on returned text so one fetch can't blow the model's context or
/// memory. ~40k chars is plenty for an article; the model asked for a summary,
/// not the raw DOM.
const MAX_CHARS: usize = 40_000;
/// Cap the raw download too (defense against a huge/hostile response).
const MAX_BYTES: usize = 5 * 1024 * 1024;

/// Fetch a URL and return extracted, readable text. Async (reqwest). Only http/
/// https are allowed — no file://, ftp://, data:, etc. (SSRF-ish surface stays
/// closed; a real allow/deny policy is a fast-follow when Connections lands).
pub async fn fetch(url: &str) -> Result<FetchResult, String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("only http(s) URLs are allowed".into());
    }
    // Block obvious internal targets (basic SSRF guard — hardened later).
    if is_blocked_host(&lower) {
        return Err("refused: that host is not allowed (internal/localhost)".into());
    }

    let client = reqwest::Client::builder()
        .user_agent("AYGENT/0.1 (+https://aygent.app)")
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let resp = client.get(u).send().await.map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status().as_u16();
    let final_url = resp.url().to_string();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Read up to MAX_BYTES.
    let bytes = resp.bytes().await.map_err(|e| format!("read body: {e}"))?;
    let slice = if bytes.len() > MAX_BYTES { &bytes[..MAX_BYTES] } else { &bytes[..] };
    let raw = String::from_utf8_lossy(slice).to_string();

    // Extract: HTML -> readable text; everything else (json/text/md) as-is.
    let is_html = content_type.contains("html") || raw.trim_start().to_ascii_lowercase().starts_with("<!doctype html") || raw.contains("<html");
    let extracted = if is_html { html_to_text(&raw) } else { raw };

    let mut text = extracted;
    let mut truncated = false;
    if text.chars().count() > MAX_CHARS {
        text = text.chars().take(MAX_CHARS).collect();
        truncated = true;
    }

    Ok(FetchResult { final_url, status, content_type, text, truncated })
}

/// Block localhost / private-range hosts (basic SSRF guard). String-level for
/// now (no DNS resolution); a full resolve-and-check lands with Connections.
fn is_blocked_host(lower_url: &str) -> bool {
    // Pull the host portion crudely.
    let after_scheme = lower_url.splitn(2, "://").nth(1).unwrap_or("");
    let host = after_scheme
        .split(['/', '?', '#']).next().unwrap_or("")
        .rsplit('@').next().unwrap_or("") // strip creds
        .split(':').next().unwrap_or(""); // strip port
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "metadata.google.internal")
        || host.ends_with(".local")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("169.254.") // link-local / cloud metadata
        // 172.16.0.0 – 172.31.255.255
        || (host.starts_with("172.") && host.split('.').nth(1)
                .and_then(|o| o.parse::<u8>().ok())
                .map(|o| (16..=31).contains(&o)).unwrap_or(false))
}

/// Minimal HTML -> text: drop script/style/head, strip tags, collapse space,
/// decode a handful of common entities. Not a full readability engine, but it
/// gives the model clean prose instead of raw markup. Dependency-free.
fn html_to_text(html: &str) -> String {
    let mut s = html.to_string();

    // Remove whole <script>/<style>/<head>/<noscript>/<svg> blocks (case-insensitive).
    for tag in ["script", "style", "head", "noscript", "svg"] {
        s = strip_block(&s, tag);
    }
    // Turn common block-enders into newlines so paragraphs survive.
    for br in ["</p>", "<br>", "<br/>", "<br />", "</div>", "</li>", "</h1>", "</h2>", "</h3>", "</tr>"] {
        s = s.replace(br, "\n").replace(&br.to_ascii_uppercase(), "\n");
    }
    // Strip all remaining tags.
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    // Decode a few entities.
    let out = out
        .replace("&nbsp;", " ").replace("&amp;", "&").replace("&lt;", "<")
        .replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'")
        .replace("&rsquo;", "'").replace("&lsquo;", "'")
        .replace("&ldquo;", "\"").replace("&rdquo;", "\"").replace("&mdash;", "—");

    // Collapse runs of blank lines + trailing spaces.
    let mut cleaned = String::with_capacity(out.len());
    let mut blank = 0;
    for line in out.lines() {
        let t = line.trim();
        if t.is_empty() {
            blank += 1;
            if blank <= 1 { cleaned.push('\n'); }
        } else {
            blank = 0;
            cleaned.push_str(t);
            cleaned.push('\n');
        }
    }
    cleaned.trim().to_string()
}

/// Remove every <tag>...</tag> block (case-insensitive) for a given tag name.
fn strip_block(s: &str, tag: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if let Some(rel) = lower[i..].find(&open) {
            let start = i + rel;
            out.push_str(&s[i..start]);
            // find the matching close (or end of string).
            if let Some(crel) = lower[start..].find(&close) {
                i = start + crel + close.len();
            } else {
                break; // unclosed — drop the rest
            }
        } else {
            out.push_str(&s[i..]);
            break;
        }
    }
    out
}

/// Blocking bridge: run the async fetch on a worker so a SYNC tool-exec path can
/// call it without threading async through every call site (same pattern the
/// embed path uses). Returns a compact, model-friendly string.
pub fn fetch_blocking(url: &str) -> Result<String, String> {
    let url = url.to_string();
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| "no tokio runtime for web fetch".to_string())?;
    // Run the future to completion on a blocking thread that borrows the runtime.
    let res = std::thread::scope(|scope| {
        scope.spawn(|| handle.block_on(fetch(&url))).join().unwrap()
    })?;
    let head = format!(
        "URL: {}\nstatus: {}\ncontent-type: {}{}\n\n",
        res.final_url, res.status, res.content_type,
        if res.truncated { " (truncated)" } else { "" }
    );
    Ok(format!("{head}{}", res.text))
}
