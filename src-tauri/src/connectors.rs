// AYGENT — CONNECTOR REGISTRY. A provider is DATA, not code.
//
// Before this module, adding one integration meant editing five places: an
// adapter fn in connections.rs, a tool schema in lib.rs, an executor match arm
// in lib.rs, a hand-written card in Connections.tsx, and BUTTON_TOOLS in
// dashboard_data.rs. That's O(N) code for O(N) providers — a copy-paste swamp by
// the fourth provider.
//
// Here a provider is ONE `Connector` descriptor: how to authenticate, how to
// validate the credential, and a list of tools with the HTTP call + how to
// render the reply. One generic executor runs all of them.
//
// TRUST MODEL (unchanged, and the reason this lives in Rust): the jailed daemon
// never holds a credential. connections.rs resolves the secret from the OS
// keychain and this module attaches it to the outbound call at call time. The
// brain only ever sees rendered text.
//
// READ vs WRITE: every tool declares `access`. Write tools are only exposed to
// an agent whose connection is explicitly in write mode (agent_connection
// .access_mode = 'write'). Read is the default everywhere.

use serde::Serialize;

// ---------------------------------------------------------------------------
// AUTH — the shape of the credential, and how to prove it works.
// ---------------------------------------------------------------------------

/// One field the user must supply to connect. Most providers need exactly one
/// (a token); Supabase needs two (project URL + key) — which is precisely why
/// this is a LIST and not a single `token: String`.
#[derive(Debug, Clone, Serialize)]
pub struct AuthField {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    /// Secret fields go to the keychain; non-secret ones (a project URL) go to
    /// the connection's config_json so we can build request URLs later.
    pub secret: bool,
    pub placeholder: &'static str,
}

/// How to validate the credential at connect time AND learn who it belongs to.
/// A connection that can't name its own account is a connection the user can't
/// tell apart from another one — which is the whole per-agent-accounts problem.
#[derive(Debug, Clone, Serialize)]
pub struct ValidateSpec {
    pub method: &'static str,
    /// May contain {field} placeholders from the auth fields (Supabase URL).
    pub url: &'static str,
    /// Dotted path into the JSON reply naming the account, e.g. "login",
    /// "bot.user_id", "team.name". Empty = don't try.
    pub identity_path: &'static str,
    /// Response header listing granted scopes, if the provider sends one.
    pub scopes_header: &'static str,
    /// JSON body for validation calls that must be POSTs (GraphQL APIs).
    pub body: &'static str,
}

// ---------------------------------------------------------------------------
// TOOLS — the HTTP call, the arguments, and how the reply becomes text.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Access {
    Read,
    Write,
}

/// An argument the model may pass. Kept deliberately thin — these become a JSON
/// schema for the provider's tool-use API.
#[derive(Debug, Clone, Serialize)]
pub struct ToolParam {
    pub name: &'static str,
    pub ty: &'static str, // "string" | "number" | "boolean"
    pub description: &'static str,
    pub required: bool,
}

/// How to turn the JSON reply into the text the agent reads. Models do better
/// with a tight rendered list than with a wall of raw JSON, and it keeps
/// provider payload churn out of the context window.
#[derive(Debug, Clone, Serialize)]
pub enum Render {
    /// Walk an array at `root` (dotted path, "" = the body itself) and format
    /// each element with `line`, substituting {dotted.paths}.
    Items {
        root: &'static str,
        line: &'static str,
        empty: &'static str,
    },
    /// Format the whole body once with a template.
    One { line: &'static str },
    /// Pretty-printed JSON, truncated. Escape hatch for shapes not worth modeling.
    Json,
    /// GitHub Contents API file reply: base64-DECODE `content` and return the
    /// plain file text (with a 1-line path/sha/size header) instead of dumping
    /// the base64 envelope through the 4000-char Json cap. Whole-file reads on
    /// private repos, no client-side base64, no clone. Non-file replies (a dir
    /// listing array, a >1MB blob pointer) fall back to truncated JSON.
    GithubContent,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectorTool {
    /// IRREVERSIBLE if called: permanent delete, money movement, sending mail to
    /// real humans. Shipped ENABLED like everything else (the user asked for full
    /// capability out of the box) but flagged so the UI can mark it and the
    /// per-tool switch is an obvious place to look.
    pub danger: bool,
    /// Globally unique tool name exposed to the model, e.g. "github_list_prs".
    /// Convention: <connector>_<verb>_<noun>.
    pub name: &'static str,
    pub description: &'static str,
    pub access: Access,
    pub method: &'static str,
    /// Appended to the connector's base_url. {arg} placeholders come from the
    /// model's tool arguments; {field} placeholders from stored auth fields.
    pub path: &'static str,
    pub query: &'static [(&'static str, &'static str)],
    /// JSON body template for writes; {arg} substituted then parsed.
    pub body: &'static str,
    pub params: &'static [ToolParam],
    /// Params whose value is a JSON *fragment* to splice into the body verbatim
    /// (block arrays, property schemas, filters). Validated as parseable JSON
    /// before use — the model can send structure, but not malformed structure.
    pub raw_params: &'static [&'static str],
    /// Params to base64-encode before substitution. GitHub's contents API wants
    /// file bodies base64'd; making the model do it would be a reliability tax
    /// for no reason.
    pub b64_params: &'static [&'static str],
    /// Overrides the connector's base_url for this tool. Some providers span
    /// hosts (Google Sheets is sheets.googleapis.com while Calendar and Drive are
    /// www.googleapis.com) while sharing one credential.
    pub base_override: &'static str,
    pub render: Render,
}

#[derive(Debug, Clone, Serialize)]
pub struct Connector {
    pub id: &'static str,
    pub label: &'static str,
    /// Grouping for the catalog UI, e.g. "Dev", "Productivity", "Infra", "Comms".
    pub category: &'static str,
    /// One line the user reads to decide if they want this.
    pub blurb: &'static str,
    pub auth_kind: &'static str, // 'pat' | 'bot_token' | 'service_account_json' | 'url_key'
    pub auth_fields: &'static [AuthField],
    /// Deep link to the EXACT page that mints the credential. Not a docs
    /// homepage — the page. Every extra click here is a support ticket.
    pub credential_url: &'static str,
    pub docs_url: &'static str,
    /// Numbered steps shown on the card. Keep to three where possible.
    pub setup_steps: &'static [&'static str],
    /// Shown when the user turns WRITE on. Say the blast radius out loud.
    pub write_warning: &'static str,
    pub base_url: &'static str,
    /// Header carrying the credential + its value template ({token}).
    pub auth_header: &'static str,
    pub auth_value: &'static str,
    /// Constant headers every request needs (Notion-Version, Accept, ...).
    pub headers: &'static [(&'static str, &'static str)],
    /// How to prove the credential works AND learn which account it is. `None`
    /// means we can't verify at paste time (avoid — a credential that fails an
    /// hour later inside an agent turn is a terrible first experience).
    pub validate: Option<ValidateSpec>,
    pub tools: &'static [ConnectorTool],
}

impl Connector {
    pub fn read_tools(&self) -> impl Iterator<Item = &ConnectorTool> {
        self.tools.iter().filter(|t| t.access == Access::Read)
    }
    /// Tools visible to an agent at the given access mode. Read mode NEVER sees
    /// write tools — the model can't call what it isn't given.
    pub fn tools_for(&self, write: bool) -> impl Iterator<Item = &ConnectorTool> {
        self.tools
            .iter()
            .filter(move |t| write || t.access == Access::Read)
    }

    /// Tools actually granted to an agent: access mode, minus anything the user
    /// switched off by name. `disabled` holds fully-qualified tool names.
    pub fn tools_granted<'a>(
        &'a self,
        write: bool,
        disabled: &'a std::collections::HashSet<String>,
    ) -> impl Iterator<Item = &'a ConnectorTool> {
        self.tools_for(write).filter(move |t| !disabled.contains(t.name))
    }

    /// Every tool this connector could ever offer — for the UI's switch list,
    /// which must show OFF tools too or you can't turn them back on.
    pub fn all_tools(&self) -> impl Iterator<Item = &ConnectorTool> {
        self.tools.iter()
    }
}

// ---------------------------------------------------------------------------
// LOOKUP
// ---------------------------------------------------------------------------

/// Find a connector by its provider id (matches `connection.provider`).
pub fn by_id(id: &str) -> Option<&'static Connector> {
    ALL.iter().find(|c| c.id == id)
}

/// Resolve a tool name to its connector + tool. This is what replaces the
/// hardcoded `else if name == "github_list_prs"` arm in the agent loop.
pub fn lookup_tool(name: &str) -> Option<(&'static Connector, &'static ConnectorTool)> {
    for c in ALL {
        if let Some(t) = c.tools.iter().find(|t| t.name == name) {
            return Some((c, t));
        }
    }
    None
}

pub fn is_connector_tool(name: &str) -> bool {
    lookup_tool(name).is_some()
}

/// The whole catalog, for the Connections screen. Non-secret by construction.
pub fn catalog() -> &'static [Connector] {
    ALL
}

/// Read-only connector tools, for the dashboard button whitelist. Derived, so a
/// new connector can never accidentally hand a WRITE tool to a dashboard button.
pub fn read_tool_names() -> Vec<&'static str> {
    ALL.iter().flat_map(|c| c.read_tools().map(|t| t.name)).collect()
}

/// The JSON schema for one tool, in the shape the provider tool-use APIs want.
pub fn tool_schema(t: &ConnectorTool) -> serde_json::Value {
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    for p in t.params {
        props.insert(
            p.name.to_string(),
            serde_json::json!({ "type": p.ty, "description": p.description }),
        );
        if p.required {
            required.push(serde_json::Value::String(p.name.to_string()));
        }
    }
    serde_json::json!({
        "name": t.name,
        "description": t.description,
        "input_schema": {
            "type": "object",
            "properties": serde_json::Value::Object(props),
            "required": serde_json::Value::Array(required),
        }
    })
}

// ---------------------------------------------------------------------------
// TEMPLATING — {placeholder} substitution from tool args + stored auth fields.
// ---------------------------------------------------------------------------

/// Read a dotted path out of a JSON value: "bot.user_id", "items.0.title".
pub fn dig<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    if path.is_empty() {
        return Some(v);
    }
    let mut cur = v;
    for seg in path.split('.') {
        cur = if let Ok(i) = seg.parse::<usize>() {
            cur.get(i)?
        } else {
            cur.get(seg)?
        };
    }
    Some(cur)
}

/// Scalar JSON as plain text (strings unquoted — the point is readable output).
fn scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Substitute every {path} in `tpl` from `ctx`. Missing values render empty —
/// a half-filled line beats a hard error on an optional field.
pub fn fill(tpl: &str, ctx: &serde_json::Value) -> String {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            None => {
                out.push('{');
                rest = after;
            }
            Some(end) => {
                let key = &after[..end];
                out.push_str(&dig(ctx, key).map(scalar).unwrap_or_default());
                rest = &after[end + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Render a reply per the tool's Render spec.
pub fn render(r: &Render, body: &serde_json::Value) -> String {
    match r {
        Render::GithubContent => render_github_content(body),
        Render::Json => {
            let s = serde_json::to_string_pretty(body).unwrap_or_default();
            s.chars().take(4000).collect()
        }
        Render::One { line } => fill(line, body),
        Render::Items { root, line, empty } => {
            let arr = dig(body, root).and_then(|v| v.as_array().cloned()).unwrap_or_default();
            if arr.is_empty() {
                return empty.to_string();
            }
            let mut out = String::new();
            for item in arr.iter().take(50) {
                out.push_str(&fill(line, item));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
    }
}

/// Decode a GitHub Contents API file reply into plain file text. GitHub returns
/// {content: <base64 with embedded newlines>, encoding:"base64", sha, size, path}
/// for a file under 1MB. We decode it and return the WHOLE file (capped at a
/// real source-file size, not 4000 chars) with a 1-line header so the model has
/// the sha for a later write. Non-file replies degrade to truncated JSON.
const GH_TEXT_CAP: usize = 200_000; // chars of DECODED text (~a large source file)
fn render_github_content(body: &serde_json::Value) -> String {
    use base64::Engine;
    // A directory listing is a JSON ARRAY, not a file object -> fall back to JSON.
    if body.is_array() {
        let s = serde_json::to_string_pretty(body).unwrap_or_default();
        return s.chars().take(4000).collect();
    }
    let encoding = body.get("encoding").and_then(|e| e.as_str()).unwrap_or("");
    let content_b64 = body.get("content").and_then(|c| c.as_str()).unwrap_or("");
    let path = body.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let sha = body.get("sha").and_then(|s| s.as_str()).unwrap_or("");
    let size = body.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
    // A file >1MB has encoding "none" (or empty content) and must be fetched via
    // the git blob API -> say so plainly instead of returning an empty file.
    if encoding != "base64" || content_b64.is_empty() {
        if size > 1_000_000 {
            return format!("{path} is {size} bytes (>1MB), which the GitHub Contents API will not inline. Read it via its git blob sha ({sha}) instead.");
        }
        // Unknown shape (symlink, submodule, or an error object) -> raw JSON.
        let s = serde_json::to_string_pretty(body).unwrap_or_default();
        return s.chars().take(4000).collect();
    }
    // GitHub base64 has embedded newlines; strip all whitespace before decoding.
    let clean: String = content_b64.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = match base64::engine::general_purpose::STANDARD.decode(clean.as_bytes()) {
        Ok(b) => b,
        Err(e) => return format!("could not decode {path}: base64 error {e}"),
    };
    let text = String::from_utf8_lossy(&bytes);
    let header = format!("# {path} · sha {sha} · {size} bytes\n");
    if text.chars().count() > GH_TEXT_CAP {
        let truncated: String = text.chars().take(GH_TEXT_CAP).collect();
        format!("{header}{truncated}\n\n[… file exceeds {GH_TEXT_CAP} chars; showing the first {GH_TEXT_CAP}. This is unusual for a source file.]")
    } else {
        format!("{header}{text}")
    }
}

mod descriptors;
pub use descriptors::ALL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_substitutes_dotted_paths() {
        let v = serde_json::json!({ "title": "Fix jail", "user": { "login": "mason" } });
        assert_eq!(fill("{user.login}: {title}", &v), "mason: Fix jail");
    }

    #[test]
    fn fill_leaves_missing_keys_empty_not_broken() {
        let v = serde_json::json!({ "a": 1 });
        assert_eq!(fill("[{a}][{nope}]", &v), "[1][]");
    }

    #[test]
    fn render_items_handles_empty() {
        let r = Render::Items { root: "items", line: "- {t}", empty: "nothing here" };
        assert_eq!(render(&r, &serde_json::json!({ "items": [] })), "nothing here");
    }

    #[test]
    fn render_items_formats_each_row() {
        let r = Render::Items { root: "items", line: "- {t}", empty: "-" };
        let body = serde_json::json!({ "items": [ { "t": "one" }, { "t": "two" } ] });
        assert_eq!(render(&r, &body), "- one\n- two");
    }

    /// THE FIX: github_read_file must return the WHOLE decoded file, not a
    /// 4000-char slice of base64. A CLAUDE.md-sized file is ~10x the old cap.
    #[test]
    fn github_content_decodes_full_file_not_truncated_base64() {
        use base64::Engine;
        // A body larger than the old 4000-char Json cap once base64-inflated.
        let file_text = "line\n".repeat(2000); // 10_000 chars of real text
        let b64 = base64::engine::general_purpose::STANDARD.encode(file_text.as_bytes());
        // GitHub inserts newlines into the base64 every 60 chars — simulate that.
        let wrapped: String = b64.as_bytes().chunks(60).map(|c| format!("{}\n", String::from_utf8_lossy(c))).collect();
        let body = serde_json::json!({
            "content": wrapped, "encoding": "base64",
            "path": "CLAUDE.md", "sha": "abc123", "size": file_text.len(),
        });
        let out = render(&Render::GithubContent, &body);
        assert!(out.starts_with("# CLAUDE.md · sha abc123"), "header present: {}", &out[..40]);
        assert!(out.contains(&file_text), "the FULL decoded file must be present, not a base64 slice");
        assert!(out.len() > 4000, "must exceed the old 4000-char cap");
    }

    #[test]
    fn github_content_directory_listing_falls_back_to_json() {
        // A directory read returns a JSON ARRAY, not a file object.
        let body = serde_json::json!([{ "name": "a.rs", "type": "file" }]);
        let out = render(&Render::GithubContent, &body);
        assert!(out.contains("a.rs"), "listing still renders: {out}");
    }

    /// The invariant that matters: read mode must never surface a write tool.
    #[test]
    fn read_mode_hides_write_tools() {
        for c in catalog() {
            assert!(
                c.tools_for(false).all(|t| t.access == Access::Read),
                "{} leaked a write tool in read mode",
                c.id
            );
        }
    }

    /// Tool names are the executor's routing key — collisions would silently
    /// dispatch to the wrong provider's API.
    #[test]
    fn tool_names_are_globally_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in catalog() {
            for t in c.tools {
                assert!(seen.insert(t.name), "duplicate tool name {}", t.name);
            }
        }
    }

    /// Every connector must tell the user where to get the credential.
    #[test]
    fn every_connector_has_setup_guidance() {
        for c in catalog() {
            assert!(!c.auth_fields.is_empty(), "{} has no auth fields", c.id);
            assert!(!c.credential_url.is_empty(), "{} has no credential_url", c.id);
            assert!(!c.setup_steps.is_empty(), "{} has no setup steps", c.id);
            if c.tools.iter().any(|t| t.access == Access::Write) {
                assert!(!c.write_warning.is_empty(), "{} has writes but no warning", c.id);
            }
        }
    }
}

#[cfg(test)]
mod inventory_tests {
    /// The Tools screen must show EXACTLY what the agent loop gives the model.
    /// If the inventory drifts from the real tool list, the UI becomes a lie —
    /// and this is the specific direction that matters: a WRITE tool listed (or
    /// granted) while the connection is in read mode.
    #[test]
    fn read_mode_inventory_contains_no_write_tools() {
        for c in crate::connectors::catalog() {
            let read_names: Vec<&str> = c.tools_for(false).map(|t| t.name).collect();
            for t in c.tools.iter().filter(|t| t.access == crate::connectors::Access::Write) {
                assert!(
                    !read_names.contains(&t.name),
                    "{} would show write tool {} in read mode",
                    c.id, t.name
                );
            }
        }
    }

    /// Every connector tool must be resolvable by name, or the inventory can
    /// display a row whose button does nothing when clicked.
    #[test]
    fn every_listed_connector_tool_is_executable() {
        for c in crate::connectors::catalog() {
            for t in c.tools {
                let found = crate::connectors::lookup_tool(t.name);
                assert!(found.is_some(), "{} is listed but not routable", t.name);
                let (owner, _) = found.unwrap();
                assert_eq!(owner.id, c.id, "{} routes to the wrong connector", t.name);
            }
        }
    }

    /// Dashboard buttons may only ever run READ tools. Derived from the registry,
    /// so this guards every future connector automatically.
    #[test]
    fn dashboard_buttons_never_get_a_write_tool() {
        let allowed = crate::dashboard_data::button_tools();
        for c in crate::connectors::catalog() {
            for t in c.tools.iter().filter(|t| t.access == crate::connectors::Access::Write) {
                assert!(
                    !allowed.contains(&t.name),
                    "write tool {} must not be runnable from a dashboard button",
                    t.name
                );
            }
        }
    }
}

#[cfg(test)]
mod catalog_integrity {
    /// A connector must never silently DISAPPEAR from the catalog.
    ///
    /// This exists because I destroyed the Slack connector with an over-wide
    /// text replacement while editing its neighbor, and only noticed because the
    /// compiler happened to catch a dangling reference in the ALL array. Had
    /// Slack been last in that list, it would have vanished quietly and the only
    /// symptom would have been a missing card in the UI.
    #[test]
    fn every_expected_connector_is_present() {
        const EXPECTED: &[&str] = &[
            "github", "notion", "linear", "google", "slack",
            "supabase", "stripe", "resend", "cloudflare", "vercel",
        ];
        for id in EXPECTED {
            assert!(
                super::by_id(id).is_some(),
                "connector `{id}` is missing from the catalog — did an edit clobber it?"
            );
        }
        assert_eq!(
            super::catalog().len(),
            EXPECTED.len(),
            "catalog size changed — update EXPECTED deliberately, don't let it drift"
        );
    }

    /// Every connector must offer real capability. Shipping a connector with a
    /// couple of read-only tools was the exact complaint that prompted this work:
    /// "hardly useful". Four is a floor, not a target.
    #[test]
    fn no_connector_is_trivially_small() {
        for c in super::catalog() {
            assert!(
                c.tools.len() >= 4,
                "{} has only {} tools — a connected account should unlock real capability",
                c.id,
                c.tools.len()
            );
        }
    }

    /// Anything that can change or destroy user data must be marked Write, so the
    /// per-tool switches and the UI labels tell the truth.
    #[test]
    fn mutating_methods_are_marked_write() {
        for c in super::catalog() {
            for t in c.tools {
                let mutating = matches!(t.method, "POST" | "PATCH" | "PUT" | "DELETE");
                // POST is also used for read-only queries (GraphQL, search), so
                // only DELETE/PATCH/PUT are unambiguously mutations.
                if matches!(t.method, "PATCH" | "PUT" | "DELETE") {
                    assert_eq!(
                        t.access,
                        super::Access::Write,
                        "{} uses {} but is marked Read",
                        t.name,
                        t.method
                    );
                }
                let _ = mutating;
            }
        }
    }

    /// Descriptions are what the MODEL reads to choose a tool. A vague or missing
    /// one makes a capability unusable no matter how correct the HTTP call is.
    #[test]
    fn every_tool_has_a_usable_description() {
        for c in super::catalog() {
            for t in c.tools {
                assert!(
                    t.description.len() > 25,
                    "{} has too thin a description for a model to choose it well",
                    t.name
                );
            }
        }
    }

    /// Raw params must actually appear in the body template, or the model is told
    /// to send structure that gets silently dropped.
    #[test]
    fn raw_params_are_referenced_by_the_body() {
        for c in super::catalog() {
            for t in c.tools {
                for r in t.raw_params {
                    let needle = format!("{{{r}}}");
                    assert!(
                        t.body.contains(&needle),
                        "{}: raw param `{}` never appears in its body template",
                        t.name,
                        r
                    );
                }
            }
        }
    }
}
