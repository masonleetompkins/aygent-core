// AYGENT — CONNECTOR DESCRIPTORS. This file is the catalog. Adding a provider
// should mean adding an entry here and NOTHING else: no new tool schema in
// lib.rs, no new executor arm, no new UI card, no BUTTON_TOOLS edit.
//
// House rules for an entry:
//   * `credential_url` points at the EXACT page that mints the credential, not a
//     docs homepage. Every extra click is a support ticket.
//   * `setup_steps` are three short imperatives a non-technical user can follow.
//   * Read tools first; every write tool needs a `write_warning` that names the
//     blast radius in plain words.
//   * Never name the auth mechanism in user-facing copy. "Connect Notion", not
//     "Paste your Notion internal integration PAT".

use super::*;

// ---------------------------------------------------------------------------
// GITHUB — ported from the hand-written adapter. auth_kind 'pat'.
// ---------------------------------------------------------------------------

const GITHUB: Connector = Connector {
    id: "github",
    label: "GitHub",
    category: "Dev",
    blurb: "Read your pull requests, issues, and repos — and let an agent push code.",
    auth_kind: "pat",
    auth_fields: &[AuthField {
        key: "token",
        label: "GitHub token",
        help: "Fine-grained token with read access to your repositories and pull requests.",
        secret: true,
        placeholder: "github_pat_… or ghp_…",
    }],
    credential_url: "https://github.com/settings/personal-access-tokens/new",
    docs_url: "https://docs.github.com/rest",
    setup_steps: &[
        "Open the GitHub token page.",
        "Give it read access to Repositories → Pull requests, Issues, Contents.",
        "Generate it, then paste it here.",
    ],
    write_warning: "The agent will be able to open issues and comment as you.",
    base_url: "https://api.github.com",
    auth_header: "Authorization",
    auth_value: "Bearer {token}",
    headers: &[
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
    tools: &[
        ConnectorTool {
            name: "github_list_prs",
            description: "List YOUR open GitHub pull requests (authored by you across all repos). \
                          Use when the user asks about their PRs / what they're working on.",
            access: Access::Read,
            method: "GET",
            path: "/search/issues",
            query: &[
                ("q", "is:open is:pr author:@me"),
                ("sort", "updated"),
                ("order", "desc"),
                ("per_page", "20"),
            ],
            body: "",
            params: &[],
            render: Render::Items {
                root: "items",
                line: "- #{number} {title}\n  {html_url}",
                empty: "You have no open pull requests.",
            },
        },
        ConnectorTool {
            name: "github_list_issues",
            description: "List open GitHub issues assigned to you across all repos.",
            access: Access::Read,
            method: "GET",
            path: "/search/issues",
            query: &[
                ("q", "is:open is:issue assignee:@me"),
                ("sort", "updated"),
                ("order", "desc"),
                ("per_page", "20"),
            ],
            body: "",
            params: &[],
            render: Render::Items {
                root: "items",
                line: "- #{number} {title}\n  {html_url}",
                empty: "No open issues assigned to you.",
            },
        },
        ConnectorTool {
            name: "github_create_issue",
            description: "Open a new issue on a repo. Requires owner, repo, and title.",
            access: Access::Write,
            method: "POST",
            path: "/repos/{owner}/{repo}/issues",
            query: &[],
            body: r#"{"title":"{title}","body":"{body}"}"#,
            params: &[
                ToolParam { name: "owner", ty: "string", description: "Repo owner/org.", required: true },
                ToolParam { name: "repo", ty: "string", description: "Repo name.", required: true },
                ToolParam { name: "title", ty: "string", description: "Issue title.", required: true },
                ToolParam { name: "body", ty: "string", description: "Issue body (markdown).", required: false },
            ],
            render: Render::One { line: "Opened issue #{number}: {html_url}" },
        },
    ],
};

// ---------------------------------------------------------------------------
// NOTION — internal integration token. The gotcha that generates every support
// ticket: the integration sees NOTHING until the user shares a page with it.
// That belongs in setup_steps, not in a docs link.
// ---------------------------------------------------------------------------

const NOTION: Connector = Connector {
    id: "notion",
    label: "Notion",
    category: "Productivity",
    blurb: "Search your workspace and read or append to pages.",
    auth_kind: "pat",
    auth_fields: &[AuthField {
        key: "token",
        label: "Notion integration secret",
        help: "From your integration's Configuration tab.",
        secret: true,
        placeholder: "ntn_… or secret_…",
    }],
    credential_url: "https://www.notion.so/my-integrations",
    docs_url: "https://developers.notion.com/reference/intro",
    setup_steps: &[
        "Create a new internal integration and copy its secret.",
        "IMPORTANT: open the Notion page you want reachable → ••• → Connections → add your integration. Without this it can see nothing.",
        "Paste the secret here.",
    ],
    write_warning: "The agent will be able to create and edit pages in the parts of your workspace you shared with it.",
    base_url: "https://api.notion.com/v1",
    auth_header: "Authorization",
    auth_value: "Bearer {token}",
    headers: &[("Notion-Version", "2022-06-28"), ("Content-Type", "application/json")],
    validate: Some(ValidateSpec {
        method: "GET",
        url: "https://api.notion.com/v1/users/me",
        identity_path: "name",
        scopes_header: "",
        body: "",
    }),
    tools: &[
        ConnectorTool {
            name: "notion_search",
            description: "Search Notion pages and databases shared with this integration by title.",
            access: Access::Read,
            method: "POST",
            path: "/search",
            query: &[],
            body: r#"{"query":"{query}","page_size":20}"#,
            params: &[ToolParam {
                name: "query",
                ty: "string",
                description: "Words to match in page titles. Empty lists everything shared.",
                required: false,
            }],
            render: Render::Items {
                root: "results",
                line: "- [{object}] {id}\n  {url}",
                empty: "Nothing matched — check the page is shared with the integration.",
            },
        },
        ConnectorTool {
            name: "notion_read_page",
            description: "Read the block content of a Notion page by its page id.",
            access: Access::Read,
            method: "GET",
            path: "/blocks/{page_id}/children",
            query: &[("page_size", "100")],
            body: "",
            params: &[ToolParam {
                name: "page_id",
                ty: "string",
                description: "The Notion page id (from notion_search).",
                required: true,
            }],
            render: Render::Json,
        },
    ],
};

// ---------------------------------------------------------------------------
// LINEAR — GraphQL, which the generic executor handles fine: it's just a POST
// with a JSON body. Proof the registry isn't REST-only.
// ---------------------------------------------------------------------------

const LINEAR: Connector = Connector {
    id: "linear",
    label: "Linear",
    category: "Dev",
    blurb: "Read the issues assigned to you and your team's current cycle.",
    auth_kind: "pat",
    auth_fields: &[AuthField {
        key: "token",
        label: "Linear API key",
        help: "Personal API key from Linear settings.",
        secret: true,
        placeholder: "lin_api_…",
    }],
    credential_url: "https://linear.app/settings/account/security",
    docs_url: "https://developers.linear.app/docs",
    setup_steps: &[
        "Open Linear → Settings → Security & access → Personal API keys.",
        "Create a key and copy it.",
        "Paste it here.",
    ],
    write_warning: "",
    base_url: "https://api.linear.app",
    auth_header: "Authorization",
    auth_value: "{token}",
    headers: &[("Content-Type", "application/json")],
    validate: Some(ValidateSpec {
        method: "POST",
        url: "https://api.linear.app/graphql",
        identity_path: "data.viewer.name",
        scopes_header: "",
        body: r#"{"query":"query { viewer { name email } }"}"#,
    }),
    tools: &[ConnectorTool {
        name: "linear_my_issues",
        description: "List the Linear issues currently assigned to you, newest first.",
        access: Access::Read,
        method: "POST",
        path: "/graphql",
        query: &[],
        body: r#"{"query":"query { viewer { assignedIssues(first: 25, orderBy: updatedAt) { nodes { identifier title state { name } url } } } }"}"#,
        params: &[],
        render: Render::Items {
            root: "data.viewer.assignedIssues.nodes",
            line: "- {identifier} [{state.name}] {title}\n  {url}",
            empty: "No issues assigned to you.",
        },
    }],
};

/// THE CATALOG. Order here is display order in the Connections screen.
pub const ALL: &[Connector] = &[GITHUB, NOTION, LINEAR];
