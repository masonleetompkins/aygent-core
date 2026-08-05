// Appends the remaining connectors. If the registry works, each of these is
// PURE DATA — no new Rust logic anywhere. That's the whole thesis.
const fs = require("fs");
const p = "src-tauri/src/connectors/descriptors.rs";
let s = fs.readFileSync(p, "utf8");

const add = String.raw`
// ---------------------------------------------------------------------------
// SLACK — bot token (xoxb-). Workspace-scoped, not user-scoped: the identity we
// capture is the WORKSPACE (team), because that's what a user needs to tell two
// Slack connections apart.
// ---------------------------------------------------------------------------

const SLACK: Connector = Connector {
    id: "slack",
    label: "Slack",
    category: "Comms",
    blurb: "Read channel history and post messages as your bot.",
    auth_kind: "bot_token",
    auth_fields: &[AuthField {
        key: "token",
        label: "Bot User OAuth Token",
        help: "From your app's OAuth & Permissions page. Starts with xoxb-.",
        secret: true,
        placeholder: "xoxb-…",
    }],
    credential_url: "https://api.slack.com/apps",
    docs_url: "https://api.slack.com/web",
    setup_steps: &[
        "Create a Slack app (from scratch) and pick your workspace.",
        "Under OAuth & Permissions add bot scopes: channels:read, channels:history, chat:write. Install to workspace.",
        "Copy the Bot User OAuth Token and paste it here — then /invite your bot to any channel it should see.",
    ],
    write_warning: "The agent will be able to post messages in channels your bot has joined. People will see them.",
    base_url: "https://slack.com/api",
    auth_header: "Authorization",
    auth_value: "Bearer {token}",
    headers: &[("Content-Type", "application/json; charset=utf-8")],
    validate: Some(ValidateSpec {
        method: "POST",
        url: "https://slack.com/api/auth.test",
        identity_path: "team",
        scopes_header: "",
        body: "{}",
    }),
    tools: &[
        ConnectorTool {
            name: "slack_list_channels",
            description: "List Slack channels the bot can see, with their ids.",
            access: Access::Read,
            method: "GET",
            path: "/conversations.list",
            query: &[("limit", "100"), ("exclude_archived", "true")],
            body: "",
            params: &[],
            render: Render::Items {
                root: "channels",
                line: "- #{name} (id {id})",
                empty: "No channels visible — invite the bot to a channel first.",
            },
        },
        ConnectorTool {
            name: "slack_read_channel",
            description: "Read recent messages from a Slack channel by its channel id.",
            access: Access::Read,
            method: "GET",
            path: "/conversations.history",
            query: &[("channel", "{channel}"), ("limit", "{limit}")],
            body: "",
            params: &[
                ToolParam { name: "channel", ty: "string", description: "Channel id from slack_list_channels.", required: true },
                ToolParam { name: "limit", ty: "number", description: "How many messages (default 20).", required: false },
            ],
            render: Render::Items {
                root: "messages",
                line: "- {user}: {text}",
                empty: "No messages in that channel.",
            },
        },
        ConnectorTool {
            name: "slack_post_message",
            description: "Post a message to a Slack channel. This is visible to everyone in it.",
            access: Access::Write,
            method: "POST",
            path: "/chat.postMessage",
            query: &[],
            body: r#"{"channel":"{channel}","text":"{text}"}"#,
            params: &[
                ToolParam { name: "channel", ty: "string", description: "Channel id.", required: true },
                ToolParam { name: "text", ty: "string", description: "Message body.", required: true },
            ],
            render: Render::One { line: "Posted to {channel} at {ts}." },
        },
    ],
};

// ---------------------------------------------------------------------------
// GOOGLE (service account) — replaces the OAuth-verification-gated placeholder.
//
// TRADEOFF, stated plainly: a service account sidesteps Google's OAuth app
// verification entirely (no review, no consent screen), but it is a SEPARATE
// identity — it sees only what's explicitly SHARED with its email address. That
// single fact generates every "why is my calendar empty" support ticket, so it
// is step 3 of setup, not a footnote in docs.
//
// NOTE: a service account needs a signed JWT assertion, which is real crypto and
// NOT expressible as a descriptor. auth_kind 'service_account_json' is recognized
// here so the catalog card and setup steps ship now; the token-minting adapter is
// the one piece of per-provider code still owed. Marked with no tools rather than
// pretending to work — a connector that lies about being ready is worse than one
// that says "setup only".
// ---------------------------------------------------------------------------

const GOOGLE: Connector = Connector {
    id: "google",
    label: "Google Calendar & Drive",
    category: "Productivity",
    blurb: "Read calendars and Drive files you share with the agent's own Google identity.",
    auth_kind: "service_account_json",
    auth_fields: &[AuthField {
        key: "service_account_json",
        label: "Service account key (JSON)",
        help: "The whole JSON file contents from Google Cloud.",
        secret: true,
        placeholder: "{\"type\":\"service_account\",…}",
    }],
    credential_url: "https://console.cloud.google.com/iam-admin/serviceaccounts",
    docs_url: "https://developers.google.com/identity/protocols/oauth2/service-account",
    setup_steps: &[
        "In Google Cloud, create a service account and add a JSON key. Enable the Calendar and Drive APIs.",
        "Paste the JSON key here.",
        "IMPORTANT: share each calendar or Drive folder WITH the service account's ...iam.gserviceaccount.com email. It is a separate identity and sees nothing until you do.",
    ],
    write_warning: "The agent will be able to create and change events on calendars you shared with it.",
    base_url: "https://www.googleapis.com",
    auth_header: "Authorization",
    auth_value: "Bearer {access_token}",
    headers: &[],
    validate: None,
    tools: &[],
};

// ---------------------------------------------------------------------------
// SUPABASE — the multi-field case: project URL (non-secret, becomes the base
// URL) + service key (secret). Proves credentials aren't always one token.
// ---------------------------------------------------------------------------

const SUPABASE: Connector = Connector {
    id: "supabase",
    label: "Supabase",
    category: "Infra",
    blurb: "Query your project's tables directly over the REST API.",
    auth_kind: "url_key",
    auth_fields: &[
        AuthField {
            key: "project_url",
            label: "Project URL",
            help: "Settings → API → Project URL.",
            secret: false,
            placeholder: "https://abcdefgh.supabase.co",
        },
        AuthField {
            key: "service_key",
            label: "Service role key",
            help: "Settings → API → service_role. Full database access — keep it private.",
            secret: true,
            placeholder: "eyJhbGciOi…",
        },
    ],
    credential_url: "https://supabase.com/dashboard/project/_/settings/api",
    docs_url: "https://supabase.com/docs/guides/api",
    setup_steps: &[
        "Open your project → Settings → API.",
        "Copy the Project URL and the service_role key.",
        "Paste both here. The service_role key bypasses row-level security, so only enable write when you mean it.",
    ],
    write_warning: "The service role key bypasses row-level security. The agent will be able to INSERT and UPDATE any table in this project.",
    base_url: "{project_url}/rest/v1",
    auth_header: "Authorization",
    auth_value: "Bearer {service_key}",
    headers: &[("apikey", "{service_key}"), ("Content-Type", "application/json")],
    validate: Some(ValidateSpec {
        method: "GET",
        url: "{project_url}/rest/v1/",
        identity_path: "",
        scopes_header: "",
        body: "",
    }),
    tools: &[
        ConnectorTool {
            name: "supabase_select",
            description: "Read rows from a Supabase table. Optionally filter with PostgREST syntax, \
                          e.g. filter 'status=eq.open'.",
            access: Access::Read,
            method: "GET",
            path: "/{table}",
            query: &[("select", "*"), ("limit", "{limit}")],
            body: "",
            params: &[
                ToolParam { name: "table", ty: "string", description: "Table name.", required: true },
                ToolParam { name: "limit", ty: "number", description: "Max rows (default 20).", required: false },
            ],
            render: Render::Json,
        },
        ConnectorTool {
            name: "supabase_insert",
            description: "Insert one row into a Supabase table. `values` is a JSON object of columns.",
            access: Access::Write,
            method: "POST",
            path: "/{table}",
            query: &[],
            body: "{values}",
            params: &[
                ToolParam { name: "table", ty: "string", description: "Table name.", required: true },
                ToolParam { name: "values", ty: "string", description: "JSON object of column values.", required: true },
            ],
            render: Render::Json,
        },
    ],
};

// ---------------------------------------------------------------------------
// STRIPE — read-only by design here. Money movement (charges, refunds) is
// deliberately ABSENT: irreversible financial actions should not be one
// hallucinated tool call away. Reads answer the actual question ("how's MRR?").
// ---------------------------------------------------------------------------

const STRIPE: Connector = Connector {
    id: "stripe",
    label: "Stripe",
    category: "Business",
    blurb: "Read recent payments, customers, and subscriptions.",
    auth_kind: "pat",
    auth_fields: &[AuthField {
        key: "token",
        label: "Stripe secret key",
        help: "Use a RESTRICTED key with read-only permissions.",
        secret: true,
        placeholder: "rk_live_… or sk_test_…",
    }],
    credential_url: "https://dashboard.stripe.com/apikeys",
    docs_url: "https://docs.stripe.com/api",
    setup_steps: &[
        "Open Stripe → Developers → API keys.",
        "Create a RESTRICTED key with read access to Charges, Customers, and Subscriptions.",
        "Paste it here. Don't paste an unrestricted secret key.",
    ],
    write_warning: "",
    base_url: "https://api.stripe.com/v1",
    auth_header: "Authorization",
    auth_value: "Bearer {token}",
    headers: &[],
    validate: Some(ValidateSpec {
        method: "GET",
        url: "https://api.stripe.com/v1/account",
        identity_path: "settings.dashboard.display_name",
        scopes_header: "",
        body: "",
    }),
    tools: &[
        ConnectorTool {
            name: "stripe_recent_payments",
            description: "List recent Stripe charges with amount and status.",
            access: Access::Read,
            method: "GET",
            path: "/charges",
            query: &[("limit", "20")],
            body: "",
            params: &[],
            render: Render::Items {
                root: "data",
                line: "- {amount} {currency} · {status} · {billing_details.email}",
                empty: "No charges yet.",
            },
        },
        ConnectorTool {
            name: "stripe_subscriptions",
            description: "List active Stripe subscriptions.",
            access: Access::Read,
            method: "GET",
            path: "/subscriptions",
            query: &[("limit", "20"), ("status", "active")],
            body: "",
            params: &[],
            render: Render::Items {
                root: "data",
                line: "- {id} · {status} · customer {customer}",
                empty: "No active subscriptions.",
            },
        },
    ],
};

// ---------------------------------------------------------------------------
// RESEND — transactional email. Sending is irreversible and public-facing, so
// it's a Write tool with a blunt warning.
// ---------------------------------------------------------------------------

const RESEND: Connector = Connector {
    id: "resend",
    label: "Resend",
    category: "Comms",
    blurb: "Check delivery of your transactional email — and send it.",
    auth_kind: "pat",
    auth_fields: &[AuthField {
        key: "token",
        label: "Resend API key",
        help: "From the API Keys page.",
        secret: true,
        placeholder: "re_…",
    }],
    credential_url: "https://resend.com/api-keys",
    docs_url: "https://resend.com/docs/api-reference",
    setup_steps: &[
        "Open Resend → API Keys.",
        "Create a key (sending access only, if you don't need writes).",
        "Paste it here.",
    ],
    write_warning: "The agent will be able to SEND real email from your verified domain. Sent email cannot be recalled.",
    base_url: "https://api.resend.com",
    auth_header: "Authorization",
    auth_value: "Bearer {token}",
    headers: &[("Content-Type", "application/json")],
    validate: Some(ValidateSpec {
        method: "GET",
        url: "https://api.resend.com/domains",
        identity_path: "",
        scopes_header: "",
        body: "",
    }),
    tools: &[
        ConnectorTool {
            name: "resend_list_domains",
            description: "List sending domains configured in Resend and their verification status.",
            access: Access::Read,
            method: "GET",
            path: "/domains",
            query: &[],
            body: "",
            params: &[],
            render: Render::Items {
                root: "data",
                line: "- {name} · {status}",
                empty: "No sending domains configured.",
            },
        },
        ConnectorTool {
            name: "resend_send_email",
            description: "Send an email via Resend. The recipient will really receive it — confirm the \
                          address and body with the user before calling this.",
            access: Access::Write,
            method: "POST",
            path: "/emails",
            query: &[],
            body: r#"{"from":"{from}","to":["{to}"],"subject":"{subject}","text":"{text}"}"#,
            params: &[
                ToolParam { name: "from", ty: "string", description: "Sender on a verified domain.", required: true },
                ToolParam { name: "to", ty: "string", description: "Recipient address.", required: true },
                ToolParam { name: "subject", ty: "string", description: "Subject line.", required: true },
                ToolParam { name: "text", ty: "string", description: "Plain-text body.", required: true },
            ],
            render: Render::One { line: "Sent — message id {id}." },
        },
    ],
};

// ---------------------------------------------------------------------------
// CLOUDFLARE — read-only: zones + recent analytics. DNS edits can take a site
// down, so they stay out of v1 on purpose.
// ---------------------------------------------------------------------------

const CLOUDFLARE: Connector = Connector {
    id: "cloudflare",
    label: "Cloudflare",
    category: "Infra",
    blurb: "List your zones and read DNS records.",
    auth_kind: "pat",
    auth_fields: &[AuthField {
        key: "token",
        label: "API token",
        help: "Use a token, not the global API key.",
        secret: true,
        placeholder: "your API token",
    }],
    credential_url: "https://dash.cloudflare.com/profile/api-tokens",
    docs_url: "https://developers.cloudflare.com/api/",
    setup_steps: &[
        "Open Cloudflare → My Profile → API Tokens → Create Token.",
        "Use the 'Read all resources' template (or Zone:Read + DNS:Read).",
        "Paste the token here.",
    ],
    write_warning: "",
    base_url: "https://api.cloudflare.com/client/v4",
    auth_header: "Authorization",
    auth_value: "Bearer {token}",
    headers: &[("Content-Type", "application/json")],
    validate: Some(ValidateSpec {
        method: "GET",
        url: "https://api.cloudflare.com/client/v4/user/tokens/verify",
        identity_path: "result.id",
        scopes_header: "",
        body: "",
    }),
    tools: &[
        ConnectorTool {
            name: "cloudflare_list_zones",
            description: "List Cloudflare zones (domains) on this account.",
            access: Access::Read,
            method: "GET",
            path: "/zones",
            query: &[("per_page", "50")],
            body: "",
            params: &[],
            render: Render::Items {
                root: "result",
                line: "- {name} · {status} · id {id}",
                empty: "No zones on this account.",
            },
        },
        ConnectorTool {
            name: "cloudflare_dns_records",
            description: "List DNS records for a zone id (from cloudflare_list_zones).",
            access: Access::Read,
            method: "GET",
            path: "/zones/{zone_id}/dns_records",
            query: &[("per_page", "100")],
            body: "",
            params: &[ToolParam { name: "zone_id", ty: "string", description: "Zone id.", required: true }],
            render: Render::Items {
                root: "result",
                line: "- {type} {name} → {content}",
                empty: "No DNS records in that zone.",
            },
        },
    ],
};

// ---------------------------------------------------------------------------
// VERCEL — deployment visibility. The question this answers is "did my deploy
// break?", which is exactly what a dashboard button should be able to ask.
// ---------------------------------------------------------------------------

const VERCEL: Connector = Connector {
    id: "vercel",
    label: "Vercel",
    category: "Infra",
    blurb: "Check your projects and recent deployment status.",
    auth_kind: "pat",
    auth_fields: &[AuthField {
        key: "token",
        label: "Access token",
        help: "From Account Settings → Tokens.",
        secret: true,
        placeholder: "your Vercel token",
    }],
    credential_url: "https://vercel.com/account/settings/tokens",
    docs_url: "https://vercel.com/docs/rest-api",
    setup_steps: &[
        "Open Vercel → Account Settings → Tokens.",
        "Create a token scoped to the account or team you want visible.",
        "Paste it here.",
    ],
    write_warning: "",
    base_url: "https://api.vercel.com",
    auth_header: "Authorization",
    auth_value: "Bearer {token}",
    headers: &[],
    validate: Some(ValidateSpec {
        method: "GET",
        url: "https://api.vercel.com/v2/user",
        identity_path: "user.username",
        scopes_header: "",
        body: "",
    }),
    tools: &[
        ConnectorTool {
            name: "vercel_list_projects",
            description: "List your Vercel projects.",
            access: Access::Read,
            method: "GET",
            path: "/v9/projects",
            query: &[("limit", "50")],
            body: "",
            params: &[],
            render: Render::Items {
                root: "projects",
                line: "- {name} (id {id})",
                empty: "No projects on this account.",
            },
        },
        ConnectorTool {
            name: "vercel_recent_deployments",
            description: "List recent Vercel deployments with their state (READY, ERROR, BUILDING). \
                          Use when the user asks whether a deploy succeeded.",
            access: Access::Read,
            method: "GET",
            path: "/v6/deployments",
            query: &[("limit", "20")],
            body: "",
            params: &[],
            render: Render::Items {
                root: "deployments",
                line: "- {name} · {state} · {url}",
                empty: "No deployments found.",
            },
        },
    ],
};
`;

// Insert the new connectors before the ALL declaration, then extend ALL.
const allDecl = `/// THE CATALOG. Order here is display order in the Connections screen.
pub const ALL: &[Connector] = &[GITHUB, NOTION, LINEAR];`;
if (!s.includes(allDecl)) throw new Error("ALL decl not found");
s = s.replace(allDecl, add + `
/// THE CATALOG. Order here is display order in the Connections screen.
pub const ALL: &[Connector] = &[
    GITHUB, LINEAR, VERCEL, CLOUDFLARE, SUPABASE, // Dev + Infra
    NOTION, GOOGLE,                               // Productivity
    SLACK, RESEND,                                // Comms
    STRIPE,                                       // Business
];`);
fs.writeFileSync(p, s);
console.log("added SLACK, GOOGLE, SUPABASE, STRIPE, RESEND, CLOUDFLARE, VERCEL");
