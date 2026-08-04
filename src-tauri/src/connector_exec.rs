// AYGENT — GENERIC CONNECTOR EXECUTOR. One HTTP path for every connector in the
// registry, replacing per-provider adapter functions.
//
// TRUST BOUNDARY: this runs on the PRIVILEGED Rust side. It resolves the secret
// from the OS keychain and attaches it to the outbound request at call time. The
// jailed daemon (the "brain") never receives a credential, only rendered text.
// Same model as the path broker: the sandbox asks, Rust decides.

use crate::connectors::{self, Access, Connector, ConnectorTool, Render};
use crate::writer::Db;

/// Resolved credential material for one connection: secret fields (from the
/// keychain) plus non-secret config fields (from config_json, e.g. a Supabase
/// project URL). Merged into one map for {placeholder} substitution.
pub struct Creds {
    pub fields: serde_json::Map<String, serde_json::Value>,
}

impl Creds {
    fn as_value(&self) -> serde_json::Value {
        serde_json::Value::Object(self.fields.clone())
    }
}

/// Execute a connector tool. Returns the tool convention `(text, is_error)` —
/// the agent loop shows errors to the model rather than aborting the turn, so it
/// can explain or retry.
pub async fn exec(
    db: &Db,
    agent_id: &str,
    name: &str,
    args: &serde_json::Value,
) -> (String, bool) {
    let Some((conn_def, tool)) = connectors::lookup_tool(name) else {
        return (format!("unknown connector tool `{name}`"), true);
    };

    // AUTHORIZATION GATE. Two independent checks, both must pass:
    //   1. the connection is connected AND enabled for THIS agent
    //   2. if the tool writes, the agent's access_mode is 'write'
    // Write tools aren't normally even in the tool list, but a model can
    // hallucinate a name — so re-check here. Never trust the caller's list.
    let access = match crate::connections::access_for_agent(db, agent_id, conn_def.id) {
        Ok(a) => a,
        Err(e) => return (e, true),
    };
    if tool.access == Access::Write && access != "write" {
        return (
            format!(
                "`{name}` changes data in {}, and this agent only has READ access. \
                 Turn on \"Allow write actions\" for {} in Connections to permit it.",
                conn_def.label, conn_def.label
            ),
            true,
        );
    }

    let creds = match crate::connections::creds_for_agent(db, agent_id, conn_def.id) {
        Ok(c) => c,
        Err(e) => return (e, true),
    };

    match call(conn_def, tool, args, &creds).await {
        Ok(text) => (text, false),
        Err(e) => (e, true),
    }
}

/// Build and send the request, then render the reply.
pub async fn call(
    c: &Connector,
    t: &ConnectorTool,
    args: &serde_json::Value,
    creds: &Creds,
) -> Result<String, String> {
    // Substitution context: tool args first, then credential/config fields.
    // Args win on collision — a connector field can't be overridden by the
    // model, because we insert creds only where the arg is absent.
    let mut ctx = args.as_object().cloned().unwrap_or_default();
    for (k, v) in creds.fields.iter() {
        ctx.entry(k.clone()).or_insert_with(|| v.clone());
    }
    let ctx = serde_json::Value::Object(ctx);

    // Required-arg check up front: a clear message beats a provider 400.
    for p in t.params {
        if p.required {
            let missing = connectors::dig(args, p.name)
                .map(|v| v.is_null() || v.as_str() == Some(""))
                .unwrap_or(true);
            if missing {
                return Err(format!("`{}` requires the `{}` argument.", t.name, p.name));
            }
        }
    }

    let base = connectors::fill(c.base_url, &ctx);
    let path = connectors::fill(t.path, &ctx);
    let url = format!("{}{}", base.trim_end_matches('/'), path);

    let client = reqwest::Client::builder()
        .user_agent("AYGENT/0.1")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let method = reqwest::Method::from_bytes(t.method.as_bytes())
        .map_err(|_| format!("bad method {}", t.method))?;
    let mut req = client.request(method, &url);

    // Credential header, built from the connector's template.
    if !c.auth_header.is_empty() {
        req = req.header(c.auth_header, connectors::fill(c.auth_value, &ctx));
    }
    for (k, v) in c.headers {
        req = req.header(*k, connectors::fill(v, &ctx));
    }
    // Query params, templated (GitHub's search `q` embeds args).
    let q: Vec<(String, String)> = t
        .query
        .iter()
        .map(|(k, v)| (k.to_string(), connectors::fill(v, &ctx)))
        .collect();
    if !q.is_empty() {
        req = req.query(&q);
    }
    // Body for writes / GraphQL. Templated then parsed so we send real JSON.
    if !t.body.is_empty() {
        let filled = fill_json(t.body, &ctx);
        let body: serde_json::Value = serde_json::from_str(&filled)
            .map_err(|e| format!("building request body: {e}"))?;
        req = req.json(&body);
    }

    let resp = req.send().await.map_err(|e| format!("{} request: {e}", c.label))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        // Actionable, not just the status code. 403 on these APIs usually means
        // "the credential is valid but wasn't granted this" — different fix than
        // a bad token, so say both.
        return Err(format!(
            "{} rejected the request ({}). The saved credential may be expired, or may not \
             have permission for this. Reconnect {} in Connections, or re-check its scopes.",
            c.label, status.as_u16(), c.label
        ));
    }
    if !status.is_success() {
        let detail = extract_error(&text);
        return Err(format!("{} error {}{}", c.label, status.as_u16(), detail));
    }

    let body: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    // GraphQL returns 200 with an errors array — a success status that isn't one.
    if let Some(errs) = body.get("errors").and_then(|e| e.as_array()) {
        if !errs.is_empty() {
            let msg = errs
                .iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!("{} error: {msg}", c.label));
        }
    }
    Ok(connectors::render(&t.render, &body))
}

/// Fill a JSON template, escaping substituted values so a quote or newline in a
/// model-supplied string can't break out and corrupt the JSON structure.
fn fill_json(tpl: &str, ctx: &serde_json::Value) -> String {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(start) = rest.find('{') {
        // `{"` or `{}` is literal JSON, not a placeholder.
        let after = &rest[start + 1..];
        let is_placeholder = after
            .chars()
            .next()
            .map(|ch| ch.is_alphanumeric() || ch == '_')
            .unwrap_or(false)
            && after.find('}').map(|e| !after[..e].contains('"')).unwrap_or(false);
        if !is_placeholder {
            out.push_str(&rest[..start + 1]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..start]);
        let end = after.find('}').unwrap();
        let key = &after[..end];
        let val = connectors::dig(ctx, key)
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        // Escape for a JSON string context.
        let escaped = serde_json::to_string(&val).unwrap_or_else(|_| "\"\"".into());
        // Strip EXACTLY the one wrapping quote each side — not trim_matches,
        // which would also eat a legitimately escaped trailing \" and corrupt
        // the JSON we're building.
        let inner = escaped
            .strip_prefix('"')
            .and_then(|x| x.strip_suffix('"'))
            .unwrap_or(&escaped);
        out.push_str(inner);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Pull a human message out of an error body (each API nests it differently).
fn extract_error(text: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        let t = text.trim();
        return if t.is_empty() { String::new() } else { format!(": {}", t.chars().take(200).collect::<String>()) };
    };
    for path in ["message", "error.message", "error", "error_description", "errors.0.message"] {
        if let Some(m) = connectors::dig(&v, path).and_then(|m| m.as_str()) {
            return format!(": {m}");
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_json_escapes_injected_quotes() {
        // A model-supplied title containing a quote must NOT break the JSON.
        let ctx = serde_json::json!({ "title": "he said \"hi\"", "body": "line1\nline2" });
        let out = fill_json(r#"{"title":"{title}","body":"{body}"}"#, &ctx);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("must stay valid JSON");
        assert_eq!(parsed["title"], "he said \"hi\"");
        assert_eq!(parsed["body"], "line1\nline2");
    }

    #[test]
    fn fill_json_preserves_literal_braces() {
        let ctx = serde_json::json!({ "query": "bug" });
        let out = fill_json(r#"{"query":"{query}","page_size":20}"#, &ctx);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["query"], "bug");
        assert_eq!(parsed["page_size"], 20);
    }

    #[test]
    fn extract_error_reads_common_shapes() {
        assert_eq!(extract_error(r#"{"message":"Bad credentials"}"#), ": Bad credentials");
        assert_eq!(extract_error(r#"{"error":{"message":"nope"}}"#), ": nope");
    }
}
