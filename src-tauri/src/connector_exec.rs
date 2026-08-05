// AYGENT — GENERIC CONNECTOR EXECUTOR. One HTTP path for every connector in the
// registry, replacing per-provider adapter functions.
//
// TRUST BOUNDARY: this runs on the PRIVILEGED Rust side. It resolves the secret
// from the OS keychain and attaches it to the outbound request at call time. The
// jailed daemon (the "brain") never receives a credential, only rendered text.
// Same model as the path broker: the sandbox asks, Rust decides.

use crate::connectors::{self, Connector, ConnectorTool};
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
    // The connection must be enabled for this agent at all.
    if let Err(e) = crate::connections::access_for_agent(db, agent_id, conn_def.id) {
        return (e, true);
    }
    // AUTHORIZATION, from the SAME source that built the tool list: the per-tool
    // off-list. Re-checked here because a model can hallucinate a tool name it
    // was never given. (This used to check access_mode instead — a second gate,
    // which is how the UI and the agent came to disagree.)
    let off = crate::connections::disabled_tools(db, agent_id, conn_def.id);
    if off.contains(name) {
        return (
            format!(
                "`{name}` is switched off for this agent. Turn it back on under {} in Connections.",
                conn_def.label
            ),
            true,
        );
    }

    let mut creds = match crate::connections::creds_for_agent(db, agent_id, conn_def.id) {
        Ok(c) => c,
        Err(e) => return (e, true),
    };

    // SERVICE-ACCOUNT AUTH: the stored credential is an RSA private key, not a
    // bearer token. Exchange it for a short-lived access token (cached ~1h) and
    // expose it as {access_token} for the descriptor's auth_value template. The
    // key itself never leaves this side of the boundary.
    if conn_def.auth_kind == "service_account_json" {
        let key_json = creds
            .fields
            .get("service_account_json")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if key_json.is_empty() {
            return ("the saved Google service-account key couldn't be read — reconnect it in Connections.".into(), true);
        }
        match crate::google_auth::access_token(&key_json, conn_def.id).await {
            Ok(tok) => {
                creds.fields.insert("access_token".into(), serde_json::Value::String(tok));
            }
            Err(e) => return (e, true),
        }
    }

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
    // Base64 params (GitHub file contents). Done here so a model never has to
    // encode by hand — that would be a reliability tax for zero benefit.
    for key in t.b64_params {
        if let Some(serde_json::Value::String(plain)) = ctx.get(*key).cloned() {
            use base64::Engine;
            let enc = base64::engine::general_purpose::STANDARD.encode(plain.as_bytes());
            ctx.insert((*key).to_string(), serde_json::Value::String(enc));
        }
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

    let base_tpl = if t.base_override.is_empty() { c.base_url } else { t.base_override };
    let base = connectors::fill(base_tpl, &ctx);
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
    // Query params, templated (GitHub's search `q` embeds args). A param whose
    // value came from a {placeholder} that resolved to EMPTY is dropped — sending
    // `order=` makes PostgREST 400 with "failed to parse order ()", and the user
    // never supplied an order at all. Constant params (no placeholder) are always
    // sent, even if a descriptor deliberately used "".
    let q: Vec<(String, String)> = t
        .query
        .iter()
        .filter_map(|(k, v)| {
            // Both key AND value are templated: PostgREST filters put the COLUMN in
            // the key ({filter_col}) and the condition in the value ({filter_val}).
            let key = connectors::fill(k, &ctx);
            let val = connectors::fill(v, &ctx);
            let key_templated = k.contains('{');
            let val_templated = v.contains('{');
            // Drop the param if a templated side resolved to empty — an omitted
            // optional filter must not send a malformed "=value" or "col=".
            if (key_templated && key.trim().is_empty())
                || (val_templated && val.trim().is_empty())
            {
                None
            } else {
                Some((key, val))
            }
        })
        .collect();
    if !q.is_empty() {
        req = req.query(&q);
    }
    // Body for writes / GraphQL. Templated then parsed so we send real JSON.
    if !t.body.is_empty() {
        let filled = fill_json_with_raw(t.body, &ctx, t.raw_params)?;
        let mut body: serde_json::Value = serde_json::from_str(&filled)
            .map_err(|e| format!("building request body: {e}"))?;
        prune_empty(&mut body);
        req = req.json(&body);
    }

    let resp = req.send().await.map_err(|e| format!("{} request: {e}", c.label))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        // PASS THE PROVIDER'S OWN MESSAGE THROUGH. It usually names the exact
        // cause, and a generic "reconnect the credential" can send the user to
        // re-do the one thing that isn't broken (seen live: Drive worked while
        // Calendar 403'd because that API was simply disabled in the project).
        let detail = extract_error(&text);
        let hint = if status == reqwest::StatusCode::UNAUTHORIZED {
            "The saved credential is being rejected — reconnect it in Connections."
        } else {
            "The credential is valid but this request was refused. Common causes: the API isn't \
             enabled for the project, the token lacks a required scope, or the resource wasn't \
             shared with this account."
        };
        return Err(format!("{} error {}{detail}\n{hint}", c.label, status.as_u16()));
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

/// Fill a JSON body template where SOME params carry JSON structure.
///
/// Two substitution modes, deliberately:
///   * normal params are ESCAPED (a quote in a title can't corrupt the body)
///   * `raw` params are SPLICED VERBATIM so the model can send real structure —
///     a block tree, a property schema, a query filter. Each is validated as
///     parseable JSON first, so "raw" never means "unchecked".
fn fill_json_with_raw(
    tpl: &str,
    ctx: &serde_json::Value,
    raw: &[&str],
) -> Result<String, String> {
    // Validate + normalize every raw param up front. A model that sends
    // malformed JSON gets a precise error naming the argument, not a confusing
    // failure about the whole request body.
    let mut normalized = serde_json::Map::new();
    if let Some(obj) = ctx.as_object() {
        for (k, v) in obj {
            if !raw.contains(&k.as_str()) {
                normalized.insert(k.clone(), v.clone());
                continue;
            }
            let parsed = match v {
                // Already structured (a good model sends real JSON) — use as-is.
                serde_json::Value::Object(_) | serde_json::Value::Array(_) => v.clone(),
                serde_json::Value::String(s) if !s.trim().is_empty() => {
                    serde_json::from_str::<serde_json::Value>(s).map_err(|e| {
                        format!("the `{k}` argument must be valid JSON — {e}")
                    })?
                }
                _ => serde_json::Value::Null,
            };
            normalized.insert(k.clone(), parsed);
        }
    }
    let nctx = serde_json::Value::Object(normalized);

    // Splice raw placeholders first (verbatim), then escape the rest.
    let mut out = tpl.to_string();
    for key in raw {
        let needle = format!("{{{key}}}");
        if !out.contains(&needle) { continue; }
        let val = crate::connectors::dig(&nctx, key)
            .filter(|v| !v.is_null())
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".into()))
            // An omitted optional structure must leave valid JSON behind.
            .unwrap_or_else(|| "null".into());
        out = out.replace(&needle, &val);
    }
    Ok(fill_json(&out, &nctx))
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

/// Drop keys whose value is an empty string or null from a request body.
///
/// Optional params leave `"sha":""` behind when the model omits them, and APIs
/// reject that outright ("sha is not a valid string") — a confusing failure for
/// something the user never typed. A template can't know which optionals were
/// supplied, so the body is cleaned after filling instead.
fn prune_empty(v: &mut serde_json::Value) {
    if let Some(obj) = v.as_object_mut() {
        obj.retain(|_, val| match val {
            serde_json::Value::String(s) => !s.is_empty(),
            serde_json::Value::Null => false,
            _ => true,
        });
        for (_, val) in obj.iter_mut() {
            prune_empty(val);
        }
    }
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

#[cfg(test)]
mod raw_tests {
    use super::*;

    /// Structure must survive intact. This is the whole point of raw params: a
    /// Notion block tree or property schema has to arrive as JSON STRUCTURE, not
    /// as an escaped string — escaping it (the default path) would send Notion a
    /// string where it expects an array, and every rich write would fail.
    #[test]
    fn raw_params_splice_as_structure_not_string() {
        let ctx = serde_json::json!({
            "children": "[{\"type\":\"paragraph\"}]"
        });
        let out = fill_json_with_raw("{\"children\":{children}}", &ctx, &["children"]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["children"].is_array(), "must be an array, got {}", parsed["children"]);
        assert_eq!(parsed["children"][0]["type"], "paragraph");
    }

    /// A model that sends already-structured JSON (not a string) must work too.
    #[test]
    fn raw_params_accept_real_json_values() {
        let ctx = serde_json::json!({ "properties": { "Name": { "title": {} } } });
        let out = fill_json_with_raw("{\"properties\":{properties}}", &ctx, &["properties"]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["properties"]["Name"]["title"].is_object());
    }

    /// Malformed JSON must fail with a message naming the ARGUMENT, so the model
    /// can correct that specific field instead of guessing at the whole request.
    #[test]
    fn malformed_raw_param_names_the_argument() {
        let ctx = serde_json::json!({ "filter": "{not valid json" });
        let err = fill_json_with_raw("{\"filter\":{filter}}", &ctx, &["filter"]).unwrap_err();
        assert!(err.contains("filter"), "{err}");
    }

    /// An omitted optional structure must leave VALID json behind (null), not a
    /// dangling `{filter}` that breaks the whole body.
    #[test]
    fn omitted_raw_param_becomes_null_and_is_pruned() {
        let ctx = serde_json::json!({ "database_id": "abc" });
        let out = fill_json_with_raw(
            "{\"filter\":{filter},\"page_size\":50}", &ctx, &["filter"],
        ).unwrap();
        let mut parsed: serde_json::Value = serde_json::from_str(&out)
            .expect("body must stay parseable when an optional structure is omitted");
        prune_empty(&mut parsed);
        assert!(parsed.get("filter").is_none(), "null optional should be pruned away");
        assert_eq!(parsed["page_size"], 50);
    }

    /// Text params must STILL be escaped even alongside raw ones — a quote in a
    /// page title can't be allowed to break the JSON.
    #[test]
    fn text_params_are_still_escaped_alongside_raw() {
        let ctx = serde_json::json!({
            "content": "he said \"hi\"",
            "children": "[]"
        });
        let out = fill_json_with_raw(
            "{\"content\":\"{content}\",\"children\":{children}}", &ctx, &["children"],
        ).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("must stay valid JSON");
        assert_eq!(parsed["content"], "he said \"hi\"");
        assert!(parsed["children"].is_array());
    }

    /// Empty optional strings must be REMOVED, not sent as "". GitHub rejects
    /// `"sha":""` outright, which would look like a bug the user caused.
    #[test]
    fn prune_empty_removes_unset_optionals() {
        let mut v = serde_json::json!({
            "message": "commit", "sha": "", "branch": "", "content": "abc"
        });
        prune_empty(&mut v);
        assert!(v.get("sha").is_none());
        assert!(v.get("branch").is_none());
        assert_eq!(v["message"], "commit");
        assert_eq!(v["content"], "abc");
    }

    /// A query param whose {placeholder} resolves to empty must be DROPPED, not
    /// sent as `key=`. Found live: supabase_select without an `order` arg sent
    /// `order=` and PostgREST 400'd with "failed to parse order ()". Constant
    /// params (no placeholder) are always kept.
    #[test]
    fn empty_templated_query_params_are_dropped_constants_kept() {
        let ctx = serde_json::json!({ "select": "id,name", "limit": "", "order": "" });
        let query: &[(&str, &str)] = &[
            ("select", "{select}"),   // supplied  -> kept
            ("limit", "{limit}"),     // empty arg -> dropped
            ("order", "{order}"),     // empty arg -> dropped
            ("apikey", "constant"),   // constant  -> kept even though no arg
        ];
        let out: Vec<(String, String)> = query
            .iter()
            .filter_map(|(k, v)| {
                let filled = crate::connectors::fill(v, &ctx);
                let templated = v.contains('{');
                if templated && filled.trim().is_empty() {
                    None
                } else {
                    Some((k.to_string(), filled))
                }
            })
            .collect();
        let keys: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"select"), "supplied param kept");
        assert!(keys.contains(&"apikey"), "constant param kept");
        assert!(!keys.contains(&"limit"), "empty templated param dropped");
        assert!(!keys.contains(&"order"), "empty templated param dropped");
    }

    /// Query KEYS are templated, not just values — PostgREST filters put the
    /// column in the key ({filter_col}=eq.x). Caught mid-delete against Mason's
    /// real DB: the literal "{filter_col}" was sent and PostgREST 400'd with
    /// "failed to parse tree path ({filter_col})". A delete that silently sent the
    /// wrong filter could hit the wrong rows, so this is the dangerous class.
    #[test]
    fn query_keys_are_templated_not_just_values() {
        let ctx = serde_json::json!({ "filter_col": "email", "filter_val": "eq.test@x.com" });
        let query: &[(&str, &str)] = &[("{filter_col}", "{filter_val}")];
        let out: Vec<(String, String)> = query
            .iter()
            .filter_map(|(k, v)| {
                let key = crate::connectors::fill(k, &ctx);
                let val = crate::connectors::fill(v, &ctx);
                let kt = k.contains('{');
                let vt = v.contains('{');
                if (kt && key.trim().is_empty()) || (vt && val.trim().is_empty()) {
                    None
                } else {
                    Some((key, val))
                }
            })
            .collect();
        assert_eq!(out, vec![("email".to_string(), "eq.test@x.com".to_string())],
                   "the column must land in the key position, substituted");
    }

    /// An omitted filter (empty key) drops the whole param — it must NEVER produce
    /// a filterless DELETE, which would hit every row.
    #[test]
    fn empty_filter_key_drops_param_never_deletes_everything() {
        let ctx = serde_json::json!({ "filter_val": "eq.x" }); // filter_col missing
        let query: &[(&str, &str)] = &[("{filter_col}", "{filter_val}")];
        let out: Vec<(String, String)> = query
            .iter()
            .filter_map(|(k, v)| {
                let key = crate::connectors::fill(k, &ctx);
                let val = crate::connectors::fill(v, &ctx);
                let kt = k.contains('{');
                let vt = v.contains('{');
                if (kt && key.trim().is_empty()) || (vt && val.trim().is_empty()) {
                    None
                } else {
                    Some((key, val))
                }
            })
            .collect();
        assert!(out.is_empty(), "a missing filter column must drop the param, not send a blank one");
    }
}
