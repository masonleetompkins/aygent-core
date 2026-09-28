// AYGENT DASHBOARDS — spec types + storage (M1 foundation).
//
// THE SAFETY RULE (Mason, 2026-08-04 — deliberate product decision):
//   The dashboard NEVER runs an LLM call automatically. Ever. A model turn
//   happens only when the user presses a button or types a prompt. Nothing here
//   runs on a timer: no intervals, no TTL expiry, no background fetch. Data is
//   PULL-ONLY, driven by an explicit Refresh.
// Nothing in this file may schedule work. If a future change needs recurrence it
// belongs in scheduler.rs, where recurring spend is already explicit + visible.
//
// WHY SPECS, NOT CODE: the model authors a declarative JSON spec; a fixed React
// renderer draws it from CSS variables. Model-authored JS in the UI process
// would hold `invoke()` and bypass the Capability enum + Seatbelt entirely
// (CONTRACTS.md #3). A spec also VALIDATES — so a bad emit becomes a structured
// error the model repairs, not a runtime crash.
//
// FORWARD-COMPAT: every enum below is `#[serde(tag = "kind")]` and stored as
// JSON in a column — the scheduler.rs precedent. Adding a variant later needs NO
// migration.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// Position on the 12-column grid. `h` is in grid rows (~40px + gap).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub x: u32,
    #[serde(default)]
    pub y: u32,
    #[serde(default = "default_w")]
    pub w: u32,
    #[serde(default = "default_h")]
    pub h: u32,
}
fn default_w() -> u32 { 4 }
fn default_h() -> u32 { 4 }

impl Default for Layout {
    fn default() -> Self { Self { x: 0, y: 0, w: default_w(), h: default_h() } }
}

impl Layout {
    /// Clamp into the 12-col grid. A module that claims 40 columns or zero
    /// height would break the grid for every other module, so we never trust
    /// model-authored geometry — we correct it.
    pub fn sanitize(&mut self) {
        self.w = self.w.clamp(1, GRID_COLS);
        self.h = self.h.clamp(1, 40);
        self.x = self.x.min(GRID_COLS.saturating_sub(self.w));
        self.y = self.y.min(4096);
    }
}

pub const GRID_COLS: u32 = 12;

// ---------------------------------------------------------------------------
// DataSource — where a module's data comes from
// ---------------------------------------------------------------------------

/// M1 ships `Static` only; the rest are declared now so the SHAPE is frozen
/// before M3 fills them in (same discipline as mem_chunk in SCHEMA_V1).
///
/// COST + REFRESH POLICY (see `is_paid`):
///   Static    — free, nothing to fetch
///   Binding   — free, whitelisted internal read        → global Refresh: YES
///   Http      — free, GET via web.rs                   → global Refresh: YES
///   Exec      — free, Pro-Mode shell, approval-gated   → global Refresh: YES (once approved)
///   AgentTurn — COSTS MONEY, a real model turn         → global Refresh: NEVER
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DataSource {
    /// Literal values baked into the spec. Always works, costs nothing.
    Static { data: serde_json::Value },

    /// A whitelisted internal read (agent.profile, schedules, savepoints,
    /// memory.search, file.read, …). Validated against BINDINGS in M3.
    Binding {
        path: String,
        #[serde(default)]
        args: serde_json::Value,
    },

    /// HTTP GET through web.rs + a JSONPath-ish extractor. Needs `net.http`.
    Http {
        url: String,
        #[serde(default)]
        extract: Option<String>,
    },

    /// A Pro-Mode shell command whose stdout is parsed. Needs `shell.exec`.
    ///
    /// APPROVAL GATE: even though nothing auto-runs, Refresh is ONE click that
    /// fans out to N commands — the user approved the BUTTON, not each command
    /// the model wrote. So an Exec module renders `pending` until a human
    /// approves the literal string, and ANY edit to it revokes approval.
    /// `approved_cmd` is the exact string that was approved; the runner refuses
    /// unless `approved_cmd == Some(cmd)`.
    Exec {
        cmd: String,
        #[serde(default)]
        approved_cmd: Option<String>,
        #[serde(default)]
        extract: Option<String>,
    },

    /// A real model turn with a required output schema. THE ONLY PAID SOURCE.
    /// Excluded from global Refresh by `is_paid()` — it gets its own explicit,
    /// separately-labeled control so spending is never an ambiguous click.
    AgentTurn {
        prompt: String,
        #[serde(default)]
        schema: serde_json::Value,
    },
}

impl DataSource {
    /// Short human name for error/assertion messages. (Restored: the fd50bcb
    /// warning sweep removed it as dead code while a test still used it —
    /// "unused" must mean unused by tests too.)
    #[allow(dead_code)] // enum accessor; kept for completeness
    pub fn label(&self) -> &'static str {
        match self {
            DataSource::Static { .. } => "static",
            DataSource::Binding { .. } => "binding",
            DataSource::Http { .. } => "http",
            DataSource::Exec { .. } => "exec",
            DataSource::AgentTurn { .. } => "agent_turn",
        }
    }
    /// Does refreshing this source spend money? The global Refresh button MUST
    /// skip every source where this is true. This single predicate is the
    /// enforcement point for the safety rule — keep the check here, not in the
    /// UI, so a new caller can't forget it.
    pub fn is_paid(&self) -> bool {
        matches!(self, DataSource::AgentTurn { .. })
    }

    /// Is this source ready to run, or is it waiting on a human? Exec is
    /// `pending` until its literal command string has been approved.
    pub fn is_pending_approval(&self) -> bool {
        match self {
            DataSource::Exec { cmd, approved_cmd, .. } => approved_cmd.as_deref() != Some(cmd.as_str()),
            _ => false,
        }
    }

}

impl Default for DataSource {
    fn default() -> Self { DataSource::Static { data: serde_json::Value::Null } }
}

// ---------------------------------------------------------------------------
// Module kinds
// ---------------------------------------------------------------------------

/// The v1 vocabulary (11). Each maps to exactly ONE reviewed React component
/// that reads only CSS variables — which is what makes "one design system"
/// structurally true instead of a guideline.
///
/// M1 RENDERS: stat, markdown, list. The rest are accepted by the validator and
/// render as a stub card, so a spec written today stays valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleKind {
    Stat,
    List,
    Table,
    Markdown,
    Chart,
    Progress,
    Timeline,
    Actions,
    Form,
    Status,
    Feed,
}

impl ModuleKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModuleKind::Stat => "stat",
            ModuleKind::List => "list",
            ModuleKind::Table => "table",
            ModuleKind::Markdown => "markdown",
            ModuleKind::Chart => "chart",
            ModuleKind::Progress => "progress",
            ModuleKind::Timeline => "timeline",
            ModuleKind::Actions => "actions",
            ModuleKind::Form => "form",
            ModuleKind::Status => "status",
            ModuleKind::Feed => "feed",
        }
    }

    /// Every accepted kind, for the tool description + validator error text.
    pub const ALL: [&'static str; 11] = [
        "stat", "list", "table", "markdown", "chart", "progress",
        "timeline", "actions", "form", "status", "feed",
    ];
}

// ---------------------------------------------------------------------------
// Actions — buttons that do things. A tagged enum; NEVER arbitrary code.
// ---------------------------------------------------------------------------

/// Declared now, wired in M4. `RunPrompt` is the one that spends money, and it
/// only ever fires from a real click (safety rule).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Send a prompt to this agent. `open_chat` surfaces it in the Chat screen
    /// instead of running headless, so the user sees what they paid for.
    RunPrompt {
        prompt: String,
        #[serde(default)]
        open_chat: bool,
    },
    /// Invoke a registered tool by name. Capability-gated at the registry.
    RunTool {
        tool: String,
        #[serde(default)]
        args: serde_json::Value,
        #[serde(default)]
        confirm: bool,
    },
    RefreshModule { module_id: String },
    Navigate { screen: String },
    OpenUrl { url: String },
    RevealInFinder { path: String },
    /// Local dashboard state — lets a form filter a table with no backend hop.
    SetVar { name: String, value: serde_json::Value },
    RunSchedule { schedule_id: String },
}

/// A labeled button. `tone` maps to the existing Pill/Button token tones.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSpec {
    pub label: String,
    pub action: Action,
    #[serde(default)]
    pub tone: Option<String>,
}

// ---------------------------------------------------------------------------
// The module + validation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleSpec {
    #[serde(default)]
    pub id: String,
    pub kind: ModuleKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub source: DataSource,
    /// Kind-specific presentation options (unit, format, columns, …). Opaque
    /// here on purpose: the renderer owns its own props, so adding an option
    /// never touches Rust.
    #[serde(default)]
    pub props: serde_json::Value,
    #[serde(default)]
    pub actions: Vec<ActionSpec>,
}

/// A structured, MODEL-READABLE validation failure. This is the repair loop:
/// the tool returns these verbatim so the model can fix its own emit on the
/// next round instead of the user seeing a broken card.
#[derive(Debug, Clone, Serialize)]
pub struct SpecError {
    pub field: String,
    pub problem: String,
    pub hint: String,
}

impl SpecError {
    fn new(field: &str, problem: &str, hint: &str) -> Self {
        Self { field: field.into(), problem: problem.into(), hint: hint.into() }
    }
}

/// Parse + validate a model-authored module. Returns either a sanitized spec or
/// the full list of problems (all of them, not just the first — one round-trip
/// should be enough to fix everything).
pub fn validate_module(raw: &serde_json::Value) -> Result<ModuleSpec, Vec<SpecError>> {
    let mut errs: Vec<SpecError> = Vec::new();

    // `kind` first: without it nothing else is meaningful.
    let kind_str = raw.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if kind_str.is_empty() {
        errs.push(SpecError::new(
            "kind",
            "missing required field `kind`",
            &format!("one of: {}", ModuleKind::ALL.join(", ")),
        ));
    } else if !ModuleKind::ALL.contains(&kind_str) {
        errs.push(SpecError::new(
            "kind",
            &format!("unknown module kind `{kind_str}`"),
            &format!("one of: {}", ModuleKind::ALL.join(", ")),
        ));
    }

    // Now the whole struct. serde gives a decent message; we reframe it so the
    // model gets a field name it can act on rather than a Rust type path.
    let mut spec: ModuleSpec = match serde_json::from_value(raw.clone()) {
        Ok(s) => s,
        Err(e) => {
            if errs.is_empty() {
                errs.push(SpecError::new(
                    "module",
                    &format!("could not parse module spec: {e}"),
                    "expected { kind, title, layout:{x,y,w,h}, source:{kind,…}, props, actions[] }",
                ));
            }
            return Err(errs);
        }
    };

    if spec.title.trim().is_empty() {
        errs.push(SpecError::new(
            "title",
            "missing or empty `title`",
            "every module needs a short human label, e.g. \"Open PRs\"",
        ));
    }

    // Source sanity — catch the empties that would render a card that silently
    // does nothing.
    match &spec.source {
        DataSource::Http { url, .. } if !url.starts_with("https://") => {
            errs.push(SpecError::new(
                "source.url",
                "http source must use https",
                "use an https:// URL (plain http is refused)",
            ));
        }
        DataSource::Exec { cmd, .. } if cmd.trim().is_empty() => {
            errs.push(SpecError::new("source.cmd", "empty command", "provide the exact shell command to run"));
        }
        DataSource::Binding { path, .. } if path.trim().is_empty() => {
            errs.push(SpecError::new("source.path", "empty binding path", "e.g. `schedules`, `savepoints`, `agent.profile`"));
        }
        DataSource::AgentTurn { prompt, .. } if prompt.trim().is_empty() => {
            errs.push(SpecError::new("source.prompt", "empty prompt", "describe exactly what the turn should produce"));
        }
        _ => {}
    }

    // An `actions` module with no buttons is a blank card.
    if spec.kind == ModuleKind::Actions && spec.actions.is_empty() {
        errs.push(SpecError::new(
            "actions",
            "an `actions` module needs at least one button",
            "add actions: [{ label, action: { kind: \"run_prompt\", prompt: \"…\" } }]",
        ));
    }

    if !errs.is_empty() {
        return Err(errs);
    }

    // Never trust model geometry — correct it rather than rejecting, so a
    // slightly-off layout still ships a usable card.
    spec.layout.sanitize();
    if spec.id.trim().is_empty() {
        spec.id = new_id();
    }
    Ok(spec)
}

/// Dependency-free id (agents.rs precedent): base36 ms + cheap random suffix.
pub fn new_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let rand: u32 = {
        let x = &ms as *const _ as usize as u64;
        let mut h = x ^ (ms as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        h ^= h >> 29;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 32;
        (h & 0xFFFF_FFFF) as u32
    };
    format!("d{}{:07x}", to_base36(ms as u64), rand)
}

fn to_base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(D[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

use crate::writer::Db;
use rusqlite::{params, OptionalExtension};

/// A module as the UI receives it: the spec plus its cache envelope. The cache
/// ships with every read because dashboards are pull-only — the UI must be able
/// to paint last-known values instantly instead of a wall of spinners.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleRow {
    pub id: String,
    pub spec: ModuleSpec,
    pub cached: Option<serde_json::Value>,
    pub fetched_at: Option<i64>,
    pub error: Option<String>,
    /// Derived, so the UI never re-implements the cost rule.
    pub is_paid: bool,
    pub pending_approval: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardView {
    pub id: String,
    pub agent_id: String,
    pub title: String,
    pub modules: Vec<ModuleRow>,
}

/// Get the agent's dashboard id, creating the (empty) dashboard on first touch.
/// Idempotent: `agent_id` is UNIQUE, so a race just re-reads.
pub fn ensure_dashboard(db: &Db, agent_id: &str) -> Result<String, String> {
    if let Ok(conn) = db.reader() {
        if let Some(id) = conn
            .query_row("SELECT id FROM dashboard WHERE agent_id = ?1", params![agent_id], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| format!("read dashboard: {e}"))?
        {
            return Ok(id);
        }
    }
    let id = new_id();
    let (aid, did, now) = (agent_id.to_string(), id.clone(), now_ms());
    db.write(move |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO dashboard (id, agent_id, title, created_at, updated_at)
             VALUES (?1, ?2, 'Dashboard', ?3, ?3)",
            params![did, aid, now],
        )
        .map_err(|e| format!("insert dashboard: {e}"))?;
        Ok(())
    })?;
    // Re-read: if another writer won the race, INSERT OR IGNORE was a no-op and
    // the real id is theirs, not ours.
    let conn = db.reader()?;
    conn.query_row("SELECT id FROM dashboard WHERE agent_id = ?1", params![agent_id], |r| r.get::<_, String>(0))
        .map_err(|e| format!("reread dashboard: {e}"))
}

/// Load a full dashboard. Grid order (top row first, then left-to-right) so the
/// DOM order matches the visual order — keyboard nav and screen readers follow
/// the layout for free.
pub fn load(db: &Db, agent_id: &str) -> Result<DashboardView, String> {
    let dash_id = ensure_dashboard(db, agent_id)?;
    let conn = db.reader()?;
    let title: String = conn
        .query_row("SELECT title FROM dashboard WHERE id = ?1", params![dash_id], |r| r.get(0))
        .unwrap_or_else(|_| "Dashboard".into());

    let mut stmt = conn
        .prepare(
            "SELECT id, spec_json, cached_json, fetched_at, error
             FROM dashboard_module WHERE dashboard_id = ?1
             ORDER BY y ASC, x ASC",
        )
        .map_err(|e| format!("prep modules: {e}"))?;
    let rows = stmt
        .query_map(params![dash_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| format!("query modules: {e}"))?;

    let mut modules = Vec::new();
    for (id, spec_json, cached_json, fetched_at, error) in rows.flatten() {
        // A spec that no longer parses (hand-edited db, or a downgrade) must not
        // take down the whole dashboard — skip it and keep rendering the rest.
        let Ok(spec) = serde_json::from_str::<ModuleSpec>(&spec_json) else { continue };
        let cached = cached_json.and_then(|c| serde_json::from_str(&c).ok());
        modules.push(ModuleRow {
            id,
            is_paid: spec.source.is_paid(),
            pending_approval: spec.source.is_pending_approval(),
            spec,
            cached,
            fetched_at,
            error,
        });
    }
    Ok(DashboardView { id: dash_id, agent_id: agent_id.to_string(), title, modules })
}

/// Snapshot every module BEFORE a mutation, so undo is a read.
fn snapshot(conn: &rusqlite::Connection, dash_id: &str, summary: &str) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT spec_json FROM dashboard_module WHERE dashboard_id = ?1 ORDER BY y, x")
        .map_err(|e| format!("prep snapshot: {e}"))?;
    let specs: Vec<serde_json::Value> = stmt
        .query_map(params![dash_id], |r| r.get::<_, String>(0))
        .map_err(|e| format!("query snapshot: {e}"))?
        .flatten()
        .filter_map(|s| serde_json::from_str(&s).ok())
        .collect();
    conn.execute(
        "INSERT INTO dashboard_revision (dashboard_id, modules_json, summary, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![dash_id, serde_json::Value::Array(specs).to_string(), summary, now_ms()],
    )
    .map_err(|e| format!("insert revision: {e}"))?;
    Ok(())
}

/// Insert or replace a module (id present = update). Always snapshots first.
pub fn upsert_module(db: &Db, agent_id: &str, spec: ModuleSpec, summary: String) -> Result<String, String> {
    let dash_id = ensure_dashboard(db, agent_id)?;
    let mid = spec.id.clone();
    let out = mid.clone();
    db.write(move |conn| {
        let tx = conn.transaction().map_err(|e| format!("txn: {e}"))?;
        snapshot(&tx, &dash_id, &summary)?;
        let spec_json = serde_json::to_string(&spec).map_err(|e| format!("ser spec: {e}"))?;
        let now = now_ms();
        // Preserve created_at on update via COALESCE against the existing row.
        tx.execute(
            "INSERT INTO dashboard_module
               (id, dashboard_id, kind, title, x, y, w, h, spec_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     COALESCE((SELECT created_at FROM dashboard_module WHERE id = ?1), ?10), ?10)
             ON CONFLICT(id) DO UPDATE SET
               kind=excluded.kind, title=excluded.title,
               x=excluded.x, y=excluded.y, w=excluded.w, h=excluded.h,
               spec_json=excluded.spec_json, updated_at=excluded.updated_at",
            params![
                mid, dash_id, spec.kind.as_str(), spec.title,
                spec.layout.x, spec.layout.y, spec.layout.w, spec.layout.h,
                spec_json, now
            ],
        )
        .map_err(|e| format!("upsert module: {e}"))?;
        tx.execute("UPDATE dashboard SET updated_at = ?1 WHERE id = ?2", params![now, dash_id])
            .map_err(|e| format!("touch dashboard: {e}"))?;
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(())
    })?;
    Ok(out)
}

pub fn remove_module(db: &Db, agent_id: &str, module_id: String) -> Result<(), String> {
    let dash_id = ensure_dashboard(db, agent_id)?;
    db.write(move |conn| {
        let tx = conn.transaction().map_err(|e| format!("txn: {e}"))?;
        snapshot(&tx, &dash_id, &format!("remove module {module_id}"))?;
        tx.execute(
            "DELETE FROM dashboard_module WHERE id = ?1 AND dashboard_id = ?2",
            params![module_id, dash_id],
        )
        .map_err(|e| format!("delete module: {e}"))?;
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(())
    })
}

/// Bulk-reposition modules. One snapshot for the whole rearrange, so a drag
/// session undoes as a single step rather than twelve.
pub fn arrange(db: &Db, agent_id: &str, moves: Vec<(String, Layout)>) -> Result<(), String> {
    let dash_id = ensure_dashboard(db, agent_id)?;
    db.write(move |conn| {
        let tx = conn.transaction().map_err(|e| format!("txn: {e}"))?;
        snapshot(&tx, &dash_id, "rearrange")?;
        let now = now_ms();
        for (mid, mut layout) in moves {
            layout.sanitize();
            // spec_json is the source of truth, so patch the geometry INSIDE it
            // too — otherwise a reload from spec would resurrect old positions.
            let cur: Option<String> = tx
                .query_row(
                    "SELECT spec_json FROM dashboard_module WHERE id = ?1 AND dashboard_id = ?2",
                    params![mid, dash_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| format!("read spec: {e}"))?;
            let Some(cur) = cur else { continue };
            let mut spec: ModuleSpec = serde_json::from_str(&cur).map_err(|e| format!("parse spec: {e}"))?;
            spec.layout = Layout { x: layout.x, y: layout.y, w: layout.w, h: layout.h };
            let spec_json = serde_json::to_string(&spec).map_err(|e| format!("ser spec: {e}"))?;
            tx.execute(
                "UPDATE dashboard_module SET x=?1, y=?2, w=?3, h=?4, spec_json=?5, updated_at=?6
                 WHERE id=?7 AND dashboard_id=?8",
                params![layout.x, layout.y, layout.w, layout.h, spec_json, now, mid, dash_id],
            )
            .map_err(|e| format!("update layout: {e}"))?;
        }
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(())
    })
}

/// Restore the most recent snapshot — "undo that". Pops the revision so repeated
/// calls walk backwards through history.
pub fn undo(db: &Db, agent_id: &str) -> Result<bool, String> {
    let dash_id = ensure_dashboard(db, agent_id)?;
    db.write(move |conn| {
        let tx = conn.transaction().map_err(|e| format!("txn: {e}"))?;
        let row: Option<(i64, String)> = tx
            .query_row(
                "SELECT id, modules_json FROM dashboard_revision WHERE dashboard_id = ?1 ORDER BY id DESC LIMIT 1",
                params![dash_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("read revision: {e}"))?;
        let Some((rev_id, modules_json)) = row else { return Ok(false) };
        let specs: Vec<ModuleSpec> = serde_json::from_str(&modules_json).map_err(|e| format!("parse revision: {e}"))?;

        tx.execute("DELETE FROM dashboard_module WHERE dashboard_id = ?1", params![dash_id])
            .map_err(|e| format!("clear modules: {e}"))?;
        let now = now_ms();
        for spec in specs {
            let spec_json = serde_json::to_string(&spec).map_err(|e| format!("ser spec: {e}"))?;
            tx.execute(
                "INSERT INTO dashboard_module (id, dashboard_id, kind, title, x, y, w, h, spec_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    spec.id, dash_id, spec.kind.as_str(), spec.title,
                    spec.layout.x, spec.layout.y, spec.layout.w, spec.layout.h, spec_json, now
                ],
            )
            .map_err(|e| format!("restore module: {e}"))?;
        }
        tx.execute("DELETE FROM dashboard_revision WHERE id = ?1", params![rev_id])
            .map_err(|e| format!("pop revision: {e}"))?;
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(true)
    })
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn dashboard_load(db: tauri::State<Db>, agent_id: String) -> Result<DashboardView, String> {
    load(&db, &agent_id)
}

/// Validate + upsert a module. Returns the structured error list on failure so
/// the caller (UI or agent tool) can show or repair it.
#[tauri::command]
pub fn dashboard_upsert_module(
    db: tauri::State<Db>,
    agent_id: String,
    module: serde_json::Value,
) -> Result<serde_json::Value, String> {
    match validate_module(&module) {
        Ok(spec) => {
            let summary = format!("upsert {} \"{}\"", spec.kind.as_str(), spec.title);
            let id = upsert_module(&db, &agent_id, spec, summary)?;
            Ok(serde_json::json!({ "ok": true, "id": id }))
        }
        Err(errs) => Ok(serde_json::json!({ "ok": false, "errors": errs })),
    }
}

#[tauri::command]
pub fn dashboard_remove_module(db: tauri::State<Db>, agent_id: String, module_id: String) -> Result<(), String> {
    remove_module(&db, &agent_id, module_id)
}

#[tauri::command]
pub fn dashboard_arrange(
    db: tauri::State<Db>,
    agent_id: String,
    moves: Vec<serde_json::Value>,
) -> Result<(), String> {
    let parsed: Vec<(String, Layout)> = moves
        .into_iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let layout: Layout = serde_json::from_value(m.get("layout")?.clone()).ok()?;
            Some((id, layout))
        })
        .collect();
    arrange(&db, &agent_id, parsed)
}

#[tauri::command]
pub fn dashboard_undo(db: tauri::State<Db>, agent_id: String) -> Result<bool, String> {
    undo(&db, &agent_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> serde_json::Value { serde_json::from_str(s).unwrap() }

    #[test]
    fn accepts_a_minimal_stat() {
        let m = validate_module(&v(r#"{"kind":"stat","title":"MRR"}"#)).unwrap();
        assert_eq!(m.kind, ModuleKind::Stat);
        assert!(!m.id.is_empty(), "id should be generated when omitted");
    }

    #[test]
    fn rejects_unknown_kind_with_a_usable_hint() {
        let errs = validate_module(&v(r#"{"kind":"gauge","title":"x"}"#)).unwrap_err();
        assert_eq!(errs[0].field, "kind");
        assert!(errs[0].hint.contains("stat"), "hint must list the valid kinds");
    }

    #[test]
    fn reports_every_problem_at_once() {
        // No title AND a bad http url: the model should be able to fix both in
        // one round-trip rather than discovering them one at a time.
        let errs = validate_module(&v(r#"{"kind":"list","source":{"kind":"http","url":"http://x.com"}}"#)).unwrap_err();
        assert!(errs.len() >= 2, "expected title + url errors, got {errs:?}");
    }

    #[test]
    fn clamps_hostile_geometry() {
        let m = validate_module(&v(r#"{"kind":"stat","title":"x","layout":{"x":11,"y":0,"w":99,"h":0}}"#)).unwrap();
        assert_eq!(m.layout.w, GRID_COLS);
        assert_eq!(m.layout.x, 0, "a full-width module must start at column 0");
        assert!(m.layout.h >= 1, "zero height would collapse the card");
    }

    // ---- THE SAFETY RULE. These two tests are the rule, in code. ----

    #[test]
    fn only_agent_turn_costs_money() {
        assert!(DataSource::AgentTurn { prompt: "p".into(), schema: serde_json::Value::Null }.is_paid());
        for free in [
            DataSource::Static { data: serde_json::Value::Null },
            DataSource::Binding { path: "schedules".into(), args: serde_json::Value::Null },
            DataSource::Http { url: "https://x.com".into(), extract: None },
            DataSource::Exec { cmd: "git log".into(), approved_cmd: None, extract: None },
        ] {
            assert!(!free.is_paid(), "{} must be free to refresh", free.label());
        }
    }

    // ---- storage round-trip, against a real (temp) database ----

    /// Self-cleaning temp dir — avoids pulling in a dev-dependency for two tests.
    struct TmpDir(std::path::PathBuf);
    impl Drop for TmpDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn temp_db() -> (Db, TmpDir) {
        let base = std::env::temp_dir().join(format!("aygent-dash-test-{}", new_id()));
        std::fs::create_dir_all(&base).expect("mkdir");
        let dir = TmpDir(base.clone());
        let db = Db::start(base).expect("db start");
        // dashboard.agent_id is an FK to agent(id) and foreign_keys=ON, so a
        // real agent row has to exist before a dashboard can attach to it.
        db.write(|conn| {
            conn.execute(
                "INSERT INTO agent (id, name, created_at) VALUES ('a_test', 'Test', 0)",
                [],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .expect("seed agent");
        (db, dir)
    }

    #[test]
    fn module_round_trips_through_storage() {
        let (db, _dir) = temp_db();
        let spec = validate_module(&v(r#"{"kind":"stat","title":"MRR","layout":{"x":0,"y":0,"w":4,"h":3},"source":{"kind":"static","data":{"value":4120}}}"#)).unwrap();
        let id = upsert_module(&db, "a_test", spec, "test".into()).unwrap();

        let view = load(&db, "a_test").unwrap();
        assert_eq!(view.modules.len(), 1);
        assert_eq!(view.modules[0].id, id);
        assert_eq!(view.modules[0].spec.title, "MRR");
        // The cost + approval flags must be DERIVED on read, so the UI can
        // never disagree with the backend about what costs money.
        assert!(!view.modules[0].is_paid);
        assert!(!view.modules[0].pending_approval);
    }

    #[test]
    fn undo_restores_the_previous_layout() {
        let (db, _dir) = temp_db();
        let spec = validate_module(&v(r#"{"kind":"list","title":"Tasks"}"#)).unwrap();
        let id = upsert_module(&db, "a_test", spec, "add".into()).unwrap();

        arrange(&db, "a_test", vec![(id.clone(), Layout { x: 6, y: 2, w: 6, h: 5 })]).unwrap();
        let moved = load(&db, "a_test").unwrap();
        assert_eq!(moved.modules[0].spec.layout.x, 6, "arrange must patch the spec, not just the columns");

        assert!(undo(&db, "a_test").unwrap());
        let back = load(&db, "a_test").unwrap();
        assert_eq!(back.modules[0].spec.layout.x, 0, "undo should restore the pre-drag position");
    }

    #[test]
    fn undo_recovers_a_deleted_module() {
        // The scenario the revision log exists for: one bad prompt wipes a
        // module the user spent real time on. That must be recoverable.
        let (db, _dir) = temp_db();
        let spec = validate_module(&v(r#"{"kind":"markdown","title":"Notes"}"#)).unwrap();
        let id = upsert_module(&db, "a_test", spec, "add".into()).unwrap();

        remove_module(&db, "a_test", id.clone()).unwrap();
        assert_eq!(load(&db, "a_test").unwrap().modules.len(), 0);

        assert!(undo(&db, "a_test").unwrap());
        let back = load(&db, "a_test").unwrap();
        assert_eq!(back.modules.len(), 1);
        assert_eq!(back.modules[0].id, id);
    }

    // ---- M2: the agent tool layer ----

    #[test]
    fn add_module_tool_creates_a_visible_module() {
        let (db, _dir) = temp_db();
        let (out, err) = exec_dashboard_tool(&db, "a_test", "dashboard_add_module",
            &v(r#"{"kind":"stat","title":"Open PRs","layout":{"x":0,"y":0,"w":3,"h":3},"source":{"kind":"static","data":{"value":4}}}"#));
        assert!(!err, "add should succeed: {out}");
        assert_eq!(load(&db, "a_test").unwrap().modules.len(), 1);
    }

    #[test]
    fn a_bad_spec_comes_back_as_repairable_errors_not_a_crash() {
        // THE REPAIR LOOP. A bogus kind must return is_error=true WITH a field
        // list, so the model can correct itself instead of the user seeing a
        // broken card or an apology.
        let (db, _dir) = temp_db();
        let (out, err) = exec_dashboard_tool(&db, "a_test", "dashboard_add_module",
            &v(r#"{"kind":"gauge","title":"Nope"}"#));
        assert!(err, "invalid spec must be flagged as an error");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("errors must be JSON the model can parse");
        assert_eq!(parsed["ok"], false);
        assert!(parsed["errors"][0]["hint"].as_str().unwrap().contains("stat"));
        assert!(parsed["retry"].is_string(), "must tell the model to retry");
        // and nothing was written
        assert_eq!(load(&db, "a_test").unwrap().modules.len(), 0);
    }

    #[test]
    fn add_ignores_a_model_supplied_id() {
        // A model that echoes an existing id on ADD would silently overwrite
        // that module. Two adds with the same id must yield TWO modules.
        let (db, _dir) = temp_db();
        let raw = v(r#"{"id":"collide","kind":"stat","title":"A"}"#);
        exec_dashboard_tool(&db, "a_test", "dashboard_add_module", &raw);
        exec_dashboard_tool(&db, "a_test", "dashboard_add_module", &raw);
        assert_eq!(load(&db, "a_test").unwrap().modules.len(), 2, "add must never overwrite by id");
    }

    #[test]
    fn update_is_a_patch_not_a_replace() {
        // Updating only the title must NOT blank the source/layout the user
        // already has — the most likely way a "tweak" would destroy work.
        let (db, _dir) = temp_db();
        let (out, _) = exec_dashboard_tool(&db, "a_test", "dashboard_add_module",
            &v(r#"{"kind":"stat","title":"Revenue","layout":{"x":2,"y":1,"w":5,"h":4},"source":{"kind":"static","data":{"value":99}}}"#));
        let id = serde_json::from_str::<serde_json::Value>(&out).unwrap()["id"].as_str().unwrap().to_string();

        let (_, err) = exec_dashboard_tool(&db, "a_test", "dashboard_update_module",
            &serde_json::json!({ "id": id, "title": "MRR" }));
        assert!(!err);

        let m = &load(&db, "a_test").unwrap().modules[0];
        assert_eq!(m.spec.title, "MRR", "title should change");
        assert_eq!(m.spec.layout.w, 5, "layout must survive a title-only patch");
        match &m.spec.source {
            DataSource::Static { data } => assert_eq!(data["value"], 99, "data must survive a title-only patch"),
            other => panic!("source was replaced: {other:?}"),
        }
    }

    #[test]
    fn update_of_a_missing_id_is_a_helpful_error() {
        let (db, _dir) = temp_db();
        let (out, err) = exec_dashboard_tool(&db, "a_test", "dashboard_update_module",
            &serde_json::json!({ "id": "nope", "title": "x" }));
        assert!(err);
        assert!(out.contains("dashboard_get"), "should point the model at how to recover: {out}");
    }

    #[test]
    fn get_reports_the_next_free_row_for_placement() {
        // The model needs somewhere to PUT the next card; without this it
        // stacks everything at y=0 and cards overlap.
        let (db, _dir) = temp_db();
        exec_dashboard_tool(&db, "a_test", "dashboard_add_module",
            &v(r#"{"kind":"stat","title":"A","layout":{"x":0,"y":0,"w":3,"h":3}}"#));
        let (out, err) = exec_dashboard_tool(&db, "a_test", "dashboard_get", &serde_json::json!({}));
        assert!(!err);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["module_count"], 1);
        assert_eq!(parsed["next_free_row"], 3);
        assert_eq!(parsed["grid_cols"], 12);
    }

    #[test]
    fn every_advertised_tool_is_dispatchable() {
        // Guards the seam: a schema the model can see but the dispatcher can't
        // route is a silent dead end.
        for schema in tool_schemas() {
            let name = schema["name"].as_str().unwrap();
            assert!(is_dashboard_tool(name), "{name} is advertised but not routed");
        }
    }

    #[test]
    fn dashboard_is_one_per_agent() {
        let (db, _dir) = temp_db();
        let a = ensure_dashboard(&db, "a_test").unwrap();
        let b = ensure_dashboard(&db, "a_test").unwrap();
        assert_eq!(a, b, "ensure_dashboard must be idempotent (v1: one dashboard per agent)");
    }

    #[test]
    fn exec_stays_pending_until_the_exact_string_is_approved() {
        let pending = DataSource::Exec { cmd: "git log".into(), approved_cmd: None, extract: None };
        assert!(pending.is_pending_approval());

        let approved = DataSource::Exec {
            cmd: "git log".into(),
            approved_cmd: Some("git log".into()),
            extract: None,
        };
        assert!(!approved.is_pending_approval());

        // The whole point: editing the command revokes approval. Approving
        // `git log` must not silently authorize `git log && rm -rf .`.
        let edited = DataSource::Exec {
            cmd: "git log && rm -rf .".into(),
            approved_cmd: Some("git log".into()),
            extract: None,
        };
        assert!(edited.is_pending_approval(), "any edit must re-require approval");
    }
}

// ---------------------------------------------------------------------------
// M2 — THE AGENT TOOL LAYER
//
// Five tools let the model build a dashboard by emitting validated specs. The
// contract that makes this work is the REPAIR LOOP: a bad emit never reaches
// the user as a broken card. It comes back as structured `errors` the model
// fixes on its next round. That is the whole reason modules are specs and not
// code — JSON can be told exactly what's wrong with it.
//
// SAFETY: none of these tools fetch, run, or spend. They only read and write
// spec rows. A module created here sits inert until a human clicks Refresh.
// ---------------------------------------------------------------------------

/// The tool schemas handed to the model. Kept in ONE place so the Anthropic,
/// OpenAI, and local-model paths cannot drift apart.
pub fn tool_schemas() -> Vec<serde_json::Value> {
    let kinds = ModuleKind::ALL.join(", ");
    vec![
        serde_json::json!({
            "name": "dashboard_get",
            "description": "Read the user's current dashboard: every module with its id, kind, title, layout and data source. ALWAYS call this before adding or changing modules, so you build on what's there instead of duplicating it.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "dashboard_add_module",
            "description": format!(
                "Add a module to the dashboard. `kind` is one of: {kinds}. \
                 Layout is a 12-column grid: x is the column (0-11), w is width in columns, \
                 h is height in ~44px rows. A stat card is usually w:3 h:3; a list or markdown \
                 card w:5-6 h:5-8. Place new modules where they don't overlap existing ones. \
                 For `static` data you supply the values directly. NOTHING you create runs \
                 automatically — the user refreshes when they choose to."
            ),
            "input_schema": { "type": "object", "properties": {
                "kind": { "type": "string", "description": format!("one of: {kinds}") },
                "title": { "type": "string", "description": "short human label, e.g. \"Open PRs\"" },
                "layout": { "type": "object", "description": "{x,y,w,h} on a 12-col grid", "properties": {
                    "x": {"type":"integer"}, "y": {"type":"integer"},
                    "w": {"type":"integer"}, "h": {"type":"integer"}
                } },
                "source": { "type": "object", "description": "{kind:\"static\", data:...} — static is the only source wired today; others are accepted but won't fetch until a later release" },
                "props": { "type": "object", "description": "presentation options, e.g. {unit:\"$\"} on a stat" },
                "actions": { "type": "array", "description": "buttons: [{label, action:{kind:\"run_prompt\", prompt:\"...\"}}]", "items": {"type":"object"} }
            }, "required": ["kind", "title"] }
        }),
        serde_json::json!({
            "name": "dashboard_update_module",
            "description": "Change an existing module. Pass its `id` (from dashboard_get) plus only the fields you want to change. Use this to retitle, resize, move, or re-point a module rather than deleting and re-adding it.",
            "input_schema": { "type": "object", "properties": {
                "id": { "type": "string" },
                "kind": { "type": "string" }, "title": { "type": "string" },
                "layout": { "type": "object" }, "source": { "type": "object" },
                "props": { "type": "object" }, "actions": { "type": "array", "items": {"type":"object"} }
            }, "required": ["id"] }
        }),
        serde_json::json!({
            "name": "dashboard_remove_module",
            "description": "Remove a module by id. The change is reversible — the user can undo it — but don't remove things you didn't create unless the user asked.",
            "input_schema": { "type": "object", "properties": {
                "id": { "type": "string" }
            }, "required": ["id"] }
        }),
        serde_json::json!({
            "name": "dashboard_arrange",
            "description": "Reposition several modules at once on the 12-column grid. Use this to tidy a layout after adding cards, so nothing overlaps and related modules sit together.",
            "input_schema": { "type": "object", "properties": {
                "moves": { "type": "array", "description": "[{id, layout:{x,y,w,h}}]", "items": {"type":"object"} }
            }, "required": ["moves"] }
        }),
    ]
}

pub fn is_dashboard_tool(name: &str) -> bool {
    matches!(name,
        "dashboard_get" | "dashboard_add_module" | "dashboard_update_module"
        | "dashboard_remove_module" | "dashboard_arrange")
}

/// The instruction block appended to the system prompt when dashboard tools are
/// live. Short on purpose: the schemas carry the mechanics, this carries taste.
pub fn tool_instructions() -> &'static str {
    "\n\nYou can build the user's DASHBOARD with dashboard_get / dashboard_add_module / \
     dashboard_update_module / dashboard_remove_module / dashboard_arrange. Call dashboard_get \
     FIRST so you extend what's already there. Prefer a few well-chosen modules over many \
     noisy ones, give each a specific title, and lay them out so the most important thing is \
     top-left. If a tool returns `errors`, read them and retry with a corrected spec. Nothing \
     you add ever runs on its own — the user decides when data refreshes."
}

/// Execute one dashboard tool. Returns (result_text, is_error) to match the
/// signature every other tool in the loop uses.
///
/// On a validation failure this returns the structured errors as JSON with
/// is_error=true — that pairing is what drives the repair loop: the model sees
/// a failure AND exactly which field to fix.
pub fn exec_dashboard_tool(
    db: &Db,
    agent_id: &str,
    name: &str,
    input: &serde_json::Value,
) -> (String, bool) {
    match name {
        "dashboard_get" => match load(db, agent_id) {
            Ok(view) => {
                // Summarize rather than dumping specs: the model needs ids,
                // kinds and geometry to place the next card, not cached payloads.
                let mods: Vec<serde_json::Value> = view.modules.iter().map(|m| serde_json::json!({
                    "id": m.id,
                    "kind": m.spec.kind.as_str(),
                    "title": m.spec.title,
                    "layout": { "x": m.spec.layout.x, "y": m.spec.layout.y, "w": m.spec.layout.w, "h": m.spec.layout.h },
                    "source": m.spec.source,
                })).collect();
                let next_y = view.modules.iter()
                    .map(|m| m.spec.layout.y + m.spec.layout.h)
                    .max().unwrap_or(0);
                (serde_json::json!({
                    "module_count": mods.len(),
                    "grid_cols": GRID_COLS,
                    "next_free_row": next_y,
                    "modules": mods,
                }).to_string(), false)
            }
            Err(e) => (format!("could not read dashboard: {e}"), true),
        },

        "dashboard_add_module" => {
            let mut raw = input.clone();
            // An id on an ADD would silently overwrite an existing module.
            if let Some(o) = raw.as_object_mut() { o.remove("id"); }
            match validate_module(&raw) {
                Ok(spec) => {
                    let summary = format!("add {} \"{}\"", spec.kind.as_str(), spec.title);
                    let kind = spec.kind.as_str();
                    let title = spec.title.clone();
                    match upsert_module(db, agent_id, spec, summary) {
                        Ok(id) => (serde_json::json!({
                            "ok": true, "id": id, "kind": kind, "title": title,
                            "note": "module added and visible to the user immediately"
                        }).to_string(), false),
                        Err(e) => (format!("could not save module: {e}"), true),
                    }
                }
                Err(errs) => (spec_error_reply(&errs), true),
            }
        }

        "dashboard_update_module" => {
            let Some(id) = input.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
                return ("dashboard_update_module needs the module `id` (call dashboard_get to find it)".into(), true);
            };
            // PATCH semantics: load the current spec and overlay only the keys
            // the model supplied. A partial update must never blank out fields
            // it didn't mention.
            let view = match load(db, agent_id) {
                Ok(v) => v,
                Err(e) => return (format!("could not read dashboard: {e}"), true),
            };
            let Some(existing) = view.modules.iter().find(|m| m.id == id) else {
                return (format!("no module with id `{id}` — call dashboard_get for the current list"), true);
            };
            let mut merged = match serde_json::to_value(&existing.spec) {
                Ok(v) => v, Err(e) => return (format!("could not read module: {e}"), true),
            };
            if let (Some(m), Some(patch)) = (merged.as_object_mut(), input.as_object()) {
                for (k, v) in patch {
                    if k == "id" { continue; }
                    m.insert(k.clone(), v.clone());
                }
            }
            if let Some(m) = merged.as_object_mut() {
                m.insert("id".into(), serde_json::Value::String(id.to_string()));
            }
            match validate_module(&merged) {
                Ok(spec) => {
                    let summary = format!("update \"{}\"", spec.title);
                    match upsert_module(db, agent_id, spec, summary) {
                        Ok(_) => (serde_json::json!({ "ok": true, "id": id }).to_string(), false),
                        Err(e) => (format!("could not save module: {e}"), true),
                    }
                }
                Err(errs) => (spec_error_reply(&errs), true),
            }
        }

        "dashboard_remove_module" => {
            let Some(id) = input.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
                return ("dashboard_remove_module needs the module `id`".into(), true);
            };
            match remove_module(db, agent_id, id.to_string()) {
                Ok(_) => (serde_json::json!({ "ok": true, "removed": id }).to_string(), false),
                Err(e) => (format!("could not remove module: {e}"), true),
            }
        }

        "dashboard_arrange" => {
            let Some(moves) = input.get("moves").and_then(|m| m.as_array()) else {
                return ("dashboard_arrange needs `moves`: [{id, layout:{x,y,w,h}}]".into(), true);
            };
            let parsed: Vec<(String, Layout)> = moves.iter().filter_map(|m| {
                let id = m.get("id")?.as_str()?.to_string();
                let layout: Layout = serde_json::from_value(m.get("layout")?.clone()).ok()?;
                Some((id, layout))
            }).collect();
            if parsed.is_empty() {
                return ("no valid moves — each needs {id, layout:{x,y,w,h}}".into(), true);
            }
            let n = parsed.len();
            match arrange(db, agent_id, parsed) {
                Ok(_) => (serde_json::json!({ "ok": true, "moved": n }).to_string(), false),
                Err(e) => (format!("could not arrange: {e}"), true),
            }
        }

        _ => (format!("unknown dashboard tool `{name}`"), true),
    }
}

/// Format validation failures for the model. Explicitly tells it to retry —
/// without that nudge models tend to apologize to the user instead of fixing.
fn spec_error_reply(errs: &[SpecError]) -> String {
    serde_json::json!({
        "ok": false,
        "errors": errs,
        "retry": "Fix the listed fields and call the tool again. Do not tell the user it failed — just correct it."
    }).to_string()
}
