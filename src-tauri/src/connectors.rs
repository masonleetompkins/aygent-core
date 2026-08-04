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
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectorTool {
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
