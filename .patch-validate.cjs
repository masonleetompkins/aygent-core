// Adds the missing `validate` field to Connector + `body` to ValidateSpec, and
// fills in each descriptor's validation call. Run once, then delete.
const fs = require("fs");

// --- 1. connectors.rs: ValidateSpec gains a body; Connector gains validate ---
const cp = "src-tauri/src/connectors.rs";
let c = fs.readFileSync(cp, "utf8");

c = c.replace(
  `    /// Response header listing granted scopes, if the provider sends one.
    pub scopes_header: &'static str,
}`,
  `    /// Response header listing granted scopes, if the provider sends one.
    pub scopes_header: &'static str,
    /// JSON body for validation calls that must be POSTs (GraphQL APIs).
    pub body: &'static str,
}`
);

c = c.replace(
  `    pub headers: &'static [(&'static str, &'static str)],
    pub tools: &'static [ConnectorTool],
}`,
  `    pub headers: &'static [(&'static str, &'static str)],
    /// How to prove the credential works AND learn which account it is. \`None\`
    /// means we can't verify at paste time (avoid — a credential that fails an
    /// hour later inside an agent turn is a terrible first experience).
    pub validate: Option<ValidateSpec>,
    pub tools: &'static [ConnectorTool],
}`
);
fs.writeFileSync(cp, c);

// --- 2. descriptors.rs: add a validate spec to each connector ---
const dp = "src-tauri/src/connectors/descriptors.rs";
let d = fs.readFileSync(dp, "utf8");

// GitHub: GET /user -> login. Also reports classic-PAT scopes in a header.
d = d.replace(
  `    headers: &[
        ("Accept", "application/vnd.github+json"),
        ("X-GitHub-Api-Version", "2022-11-28"),
    ],
    tools:`,
  `    headers: &[
        ("Accept", "application/vnd.github+json"),
        ("X-GitHub-Api-Version", "2022-11-28"),
    ],
    validate: Some(ValidateSpec {
        method: "GET",
        url: "https://api.github.com/user",
        identity_path: "login",
        scopes_header: "x-oauth-scopes",
        body: "",
    }),
    tools:`
);

// Notion: /users/me -> the bot's owner name.
d = d.replace(
  `    headers: &[("Notion-Version", "2022-06-28"), ("Content-Type", "application/json")],
    tools:`,
  `    headers: &[("Notion-Version", "2022-06-28"), ("Content-Type", "application/json")],
    validate: Some(ValidateSpec {
        method: "GET",
        url: "https://api.notion.com/v1/users/me",
        identity_path: "name",
        scopes_header: "",
        body: "",
    }),
    tools:`
);

// Linear: GraphQL viewer query -> the user's name.
d = d.replace(
  `    headers: &[("Content-Type", "application/json")],
    tools: &[ConnectorTool {
        name: "linear_my_issues",`,
  `    headers: &[("Content-Type", "application/json")],
    validate: Some(ValidateSpec {
        method: "POST",
        url: "https://api.linear.app/graphql",
        identity_path: "data.viewer.name",
        scopes_header: "",
        body: r#"{"query":"query { viewer { name email } }"}"#,
    }),
    tools: &[ConnectorTool {
        name: "linear_my_issues",`
);
fs.writeFileSync(dp, d);
console.log("validate specs added");
