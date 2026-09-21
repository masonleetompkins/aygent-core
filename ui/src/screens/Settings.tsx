// Settings — THE WEDGE. Where "no config files, ever" becomes visible and
// delightful. Appearance (theme lives here now, not a debug bar), Providers
// (BYO keys -> Keychain), and the Agent Folder scope. Themed via tokens.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card, Button, Input, Pill } from "../components/ui";
import { saveTheme, type Mode } from "../lib/theme";

const ACCENT_SWATCHES = ["", "#2dd4bf", "#a78bfa", "#4169e1", "#00cafc", "#e0533d", "#22c55e", "#eab308", "#ec4899"]; // royal #4169e1 + electric #00cafc (Mason 7-fix)
const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

// Cost + context info per model. Anthropic's API doesn't return pricing or
// context sizes, so this is a static map keyed by model id, sourced from
// Anthropic's published docs (platform.claude.com, verified 2026-07-24):
//   - 1M-token context: Opus 5, Opus 4.6/4.7/4.8, Sonnet 5, Sonnet 4.6,
//     Fable 5, Mythos 5 (1M is the DEFAULT on these — no beta header needed).
//   - 200k-token context: Sonnet 4.5 and earlier, ALL Haiku, everything else.
//   - Legacy Claude 2.x: 100k.
// Pricing is $ per MILLION tokens (input / output).
// NOTE: model classification/pricing (modelInfo/fmtPrice) moved OUT of Settings
// when the model picker moved to the Agents screen. The Agents form owns model
// ranking + labels now (see screens/Agents.tsx: modelRank/modelLabel).

export function Settings({
  mode, accent, onTheme, folder, onPickFolder, agentId, daemonStatus, daemonOk,
}: {
  mode: Mode; accent: string; onTheme: (m: Mode, a: string) => void;
  folder: string | null; onPickFolder: () => void; agentId: string | null;
  daemonStatus?: string; daemonOk?: boolean;
}) {
  // M1.7: per-agent auto-remember toggle (surface the setting we built).
  const [autoRemember, setAutoRemember] = useState(true);
  useEffect(() => {
    if (!agentId) return;
    invoke<boolean>("memory_get_auto_remember", { agentId }).then(setAutoRemember).catch(() => {});
  }, [agentId]);
  async function toggleAutoRemember(on: boolean) {
    setAutoRemember(on);
    if (agentId) { try { await invoke("memory_set_auto_remember", { agentId, enabled: on }); } catch { /* ignore */ } }
  }
  const [apiKey, setApiKey] = useState("");
  const [keySet, setKeySet] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);
  const [retention, setRetention] = useState(30);
  const [cpMsg, setCpMsg] = useState<string | null>(null);
  const [confirmPurge, setConfirmPurge] = useState(false);

  // M1.4 multi-agent knobs: inter-agent budget + max concurrency.
  const [budget, setBudget] = useState(6);
  const [concurrency, setConcurrency] = useState(6);
  const [knobMsg, setKnobMsg] = useState<string | null>(null);
  useEffect(() => {
    invoke<{ inter_agent_budget: number; max_concurrency: number }>("get_app_knobs")
      .then((k) => { setBudget(k.inter_agent_budget); setConcurrency(k.max_concurrency); })
      .catch(() => {});
  }, []);
  async function saveKnobs(b: number, c: number) {
    setBudget(b); setConcurrency(c); setKnobMsg(null);
    try { await invoke("set_app_knobs", { budget: b, concurrency: c }); setKnobMsg("✓ saved"); }
    catch (e) { setKnobMsg("✗ " + String(e)); }
  }

  useEffect(() => { invoke<boolean>("has_provider_key", { provider: "anthropic" }).then(setKeySet).catch(() => {}); }, []);
  useEffect(() => {
    if (!folder) return;
    invoke<number>("savepoint_get_retention").then(setRetention).catch(() => {});
  }, [folder]);

  async function saveRetention(days: number) {
    setRetention(days); setCpMsg(null);
    try { await invoke("savepoint_set_retention", { days }); setCpMsg(`✓ keeping ${days} days`); }
    catch (e) { setCpMsg("✗ " + String(e)); }
  }
  async function purgeAll() {
    setConfirmPurge(false); setCpMsg(null);
    try { await invoke("savepoint_purge"); setCpMsg("✓ all Save Points purged"); }
    catch (e) { setCpMsg("✗ " + String(e)); }
  }
  async function saveKey() {
    if (!apiKey.trim()) return;
    await invoke("set_provider_key", { provider: "anthropic", key: apiKey.trim() });
    setApiKey(""); setKeySet(true); setTestResult(null);
  }
  async function testKey() {
    setTestResult("testing…");
    try {
      const models = await invoke<string[]>("anthropic_models");
      setTestResult(`✓ connected · ${models.length} models available`);
    } catch (e) { setTestResult("✗ " + String(e)); }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "var(--space-4)", maxWidth: 620 }}>
      <h2 style={{ fontSize: "var(--text-h1)", fontWeight: "var(--weight-heading)", margin: 0 }}>Settings</h2>

      {/* APPEARANCE — at the top. */}
      <Card title="Appearance">
        {/* (daemon sanity-check card is at the bottom) */}
        <div style={{ display: "flex", gap: "var(--space-2)", alignItems: "center" }}>
          <span style={{ fontSize: "var(--text-body)", fontWeight: 600, width: 90 }}>Mode</span>
          <div style={{ display: "flex", gap: "var(--space-2)", flexWrap: "wrap" }}>
            <Button variant={mode === "light" ? "primary" : "secondary"} onClick={() => onTheme("light", accent)}>◐ Light</Button>
            <Button variant={mode === "dark" ? "primary" : "secondary"} onClick={() => onTheme("dark", accent)}>◑ Dark</Button>
            <Button variant={mode === "neutral" ? "primary" : "secondary"} onClick={() => onTheme("neutral", accent)}>◒ Neutral</Button>
            <Button variant={mode === "matrix" ? "primary" : "secondary"} onClick={() => onTheme("matrix", accent)}>▚ Matrix</Button>
          </div>
        </div>
        <div style={{ display: "flex", gap: "var(--space-3)", alignItems: "center", marginTop: "var(--space-1)" }}>
          <span style={{ fontSize: "var(--text-body)", fontWeight: 600, width: 90 }}>Accent</span>
          <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            {ACCENT_SWATCHES.map((c) => (
              <button key={c || "default"} onClick={() => onTheme(mode, c)} title={c || "default (black/white)"}
                style={{
                  width: 26, height: 26, borderRadius: 999, cursor: "pointer",
                  border: `2px solid ${accent === c ? "var(--text)" : "var(--line)"}`,
                  background: c || (mode === "matrix" ? "#00ff41" : mode === "dark" ? "#ffffff" : mode === "neutral" ? "#22201c" : "#0a0a0a"),
                  boxShadow: accent === c ? "var(--elevation)" : "none",
                }} />
            ))}
          </div>
        </div>
      </Card>

      {/* PROVIDERS */}
      <Card title="Providers">
        <p style={hint}>Bring your own keys. They go straight to the macOS Keychain — the UI never keeps them.</p>
        <ProviderRow provider="anthropic" label="Anthropic" placeholder="sk-ant-…" />
        <ProviderRow provider="openai" label="OpenAI" placeholder="sk-…" />
        <ProviderRow provider="openrouter" label="OpenRouter" placeholder="sk-or-…" />
        <ProviderRow provider="meta" label="Muse (Meta)" placeholder="your Muse API key" />
      </Card>

      {/* MODEL card removed (Mason cleanup #3) — model choice lives per-agent in
          the Agents tab. */}

      {/* LOCAL MODELS — OpenRouter-style search (replaces the old Browse button) */}
      <LocalModels folder={folder} activePath="" onChoose={() => {}} />

      {/* MEMORY (M1.7) */}
      <Card title="Memory">
        <p style={hint}>
          When on, this agent quietly remembers durable facts you mention in conversation
          (preferences, decisions, people) into its vault — salience-gated and de-duplicated, so it
          never clutters. Off = it only remembers when you explicitly ask.
        </p>
        {!agentId ? (
          <p style={{ ...hint, color: "var(--text-faint)" }}>Pick an agent in the rail to configure its memory.</p>
        ) : (
          <label style={{ display: "flex", gap: "var(--space-2)", alignItems: "center", fontSize: "var(--text-body)", fontWeight: 600 }}>
            <input type="checkbox" checked={autoRemember} onChange={(e) => toggleAutoRemember(e.target.checked)} />
            Auto-remember from conversation
          </label>
        )}
      </Card>

      {/* SAVE POINTS */}
      <Card title="Save Points">
        <p style={hint}>Every change your agent makes is a Save Point so you can rewind. Keep history for a window, then it prunes automatically.</p>
        {!folder ? (
          <p style={{ ...hint, color: "var(--text-faint)" }}>Pick an agent with a folder to configure Save Points.</p>
        ) : (
          <>
            <div style={{ display: "flex", alignItems: "center", gap: "var(--space-3)" }}>
              <span style={{ fontSize: "var(--text-body)", fontWeight: 600, width: 90 }}>Keep for</span>
              <input
                type="range" min={1} max={90} value={retention}
                onChange={(e) => setRetention(Number(e.target.value))}
                onMouseUp={(e) => saveRetention(Number((e.target as HTMLInputElement).value))}
                onTouchEnd={(e) => saveRetention(Number((e.target as HTMLInputElement).value))}
                style={{ flex: 1, accentColor: "var(--accent)" }}
              />
              <span style={{ fontFamily: "ui-monospace, monospace", fontSize: "var(--text-body)", fontWeight: 700, width: 64, textAlign: "right" }}>
                {retention} day{retention === 1 ? "" : "s"}
              </span>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: "var(--space-2)", marginTop: "var(--space-1)" }}>
              {!confirmPurge ? (
                <Button variant="secondary" onClick={() => setConfirmPurge(true)}>Purge all history…</Button>
              ) : (
                <>
                  <Button onClick={purgeAll}>Confirm purge</Button>
                  <Button variant="secondary" onClick={() => setConfirmPurge(false)}>Cancel</Button>
                  <span style={{ ...hint, color: "var(--danger)", fontSize: 13 }}>Deletes all Save Points (your files are untouched).</span>
                </>
              )}
            </div>
            {cpMsg && <Pill tone={cpMsg.startsWith("✗") ? "danger" : "ok"}>{cpMsg}</Pill>}
          </>
        )}
      </Card>

      {/* AGENT FOLDER card removed — each agent's folder is set in the Agents tab. */}

      {/* AYGENT REMOTE — pair this Mac with masonlee.build/remote. */}
      <RemoteCard />

      {/* DAEMON — quiet sanity-check (moved out of the old persistent top strip).
          Just confirms the local engine is up; not something to stare at. */}
      {daemonStatus && (
        <Card title="System">
          <div style={{ display: "flex", alignItems: "center", gap: "var(--space-2)" }}>
            <span style={{ fontSize: "var(--text-body)", color: daemonOk ? "var(--ok)" : "var(--text-muted)" }}>
              {daemonOk ? "●" : "○"}
            </span>
            <span style={{ fontSize: "var(--text-caption)", color: "var(--text-muted)" }}>{daemonStatus}</span>
          </div>
        </Card>
      )}
    </div>
  );
}

// One selectable model row: radio-style card with name, blurb, cost/context.
function ModelRow({ active, onClick, title, sub, meta, mono }: {
  active: boolean; onClick: () => void; title: string; sub: string; meta: string; mono?: string;
}) {
  return (
    <button onClick={onClick} style={{
      display: "flex", alignItems: "center", gap: 12, textAlign: "left", cursor: "pointer",
      padding: "10px 12px", borderRadius: "var(--radius-control)",
      border: `var(--border-width) solid ${active ? "var(--accent, var(--text))" : "var(--line)"}`,
      background: active ? "var(--bg)" : "transparent",
      boxShadow: active ? "var(--elevation)" : "none",
      color: "var(--text)", font: "inherit",
    }}>
      <span style={{
        width: 14, height: 14, borderRadius: 999, flexShrink: 0,
        border: `2px solid ${active ? "var(--accent, var(--text))" : "var(--line)"}`,
        background: active ? "var(--accent, var(--text))" : "transparent",
      }} />
      <span style={{ display: "flex", flexDirection: "column", gap: 2, minWidth: 0, flex: 1 }}>
        <span style={{ display: "flex", alignItems: "baseline", gap: 8 }}>
          <span style={{ fontWeight: 800, fontSize: 15 }}>{title}</span>
          {mono && <span style={{ fontFamily: "ui-monospace, monospace", fontSize: 11, color: "var(--text-faint)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{mono}</span>}
        </span>
        <span style={{ fontSize: 13, color: "var(--text-muted)" }}>{sub}</span>
      </span>
      <span style={{ fontSize: 12, fontFamily: "ui-monospace, monospace", color: "var(--text-muted)", flexShrink: 0 }}>{meta}</span>
    </button>
  );
}

// ---- LOCAL MODELS ---------------------------------------------------------
// OpenRouter-style search: type to live-search Hugging Face GGUF models,
// auto-fills model strings, select (or paste exact repo id) to show the same
// info card (speed/memory/perf + quant download choices) we already use.
type Perf = { tier: string; badge: string; tokens_per_sec: string; note: string; fits: boolean };
type Quant = { tier: string; quant: string; filename: string; size_gb: number; download_url: string; perf: Perf };

function speedWords(perf: Perf): string {
  switch (perf.tier) {
    case "great": return "fast";
    case "usable": return "okay speed";
    case "partial": return "runs (GPU + CPU split)";
    case "slow": return "slow (mostly CPU)";
    default: return "";
  }
}

function contextWords(tokens: number): { short: string; long: string } {
  if (!tokens) return { short: "—", long: "context size unknown" };
  const k = Math.round(tokens / 1024);
  const words = Math.round((tokens * 0.75) / 1000);
  return { short: `${k}k`, long: `can hold about ${words.toLocaleString()},000 words of conversation` };
}

function quantBlurb(quant: string): { title: string; sub: string } {
  const higher = ["Q5_K_M", "Q6_K", "Q8_0"];
  const efficient = ["IQ4_XS", "IQ3_M", "Q3_K_S", "IQ3_XXS", "Q2_K", "IQ2_M", "IQ2_XS"];
  if (higher.includes(quant)) {
    return { title: "Higher quality", sub: "sharper answers · larger file · needs more memory" };
  }
  if (efficient.includes(quant)) {
    return { title: "Efficient", sub: "smallest file · runs big models on modest memory · slight quality dip" };
  }
  return { title: "Recommended", sub: "nearly identical quality · smaller file · best for most people" };
}
type CatModel = { family: string; family_label: string; repo: string; name: string; params_billions: number; context_tokens: number; downloads: number; quants: Quant[] };
type HW = { summary: string };
type Downloaded = { filename: string; path: string; size_gb: number };

function isRepoId(s: string): boolean {
  const t = s.trim();
  if (!t.includes("/")) return false;
  if (t.includes(" ")) return false;
  return /^[^\/\s]+\/[^\/\s]+$/.test(t);
}

function LocalModels({ folder, activePath, onChoose }: {
  folder: string | null; activePath: string; onChoose: (path: string, name: string) => void;
}) {
  const [hw, setHw] = useState<HW | null>(null);
  const [downloaded, setDownloaded] = useState<Downloaded[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [progress, setProgress] = useState<Record<string, number>>({});
  const [toolCaps, setToolCaps] = useState<Record<string, boolean>>({});

  // OpenRouter-style search state
  const [query, setQuery] = useState("");
  const [focused, setFocused] = useState(false);
  const [searching, setSearching] = useState(false);
  const [results, setResults] = useState<CatModel[]>([]);
  const [selected, setSelected] = useState<CatModel | null>(null);
  const [highlight, setHighlight] = useState(-1);
  const [pulling, setPulling] = useState<string | null>(null);
  const [pullProg, setPullProg] = useState<{ file: string; index: number; files: number; pct: number } | null>(null);
  const [mlxPulled, setMlxPulled] = useState<Array<{ repo: string; path: string; size_gb: number }>>([]);
  const [mlxServe, setMlxServe] = useState<{ running: boolean; repo: string | null } | null>(null);
  const [pullMsg, setPullMsg] = useState<string | null>(null);
  const seqRef = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);

  async function refreshDownloaded() {
    try {
      const list = await invoke<Downloaded[]>("local_downloaded");
      setDownloaded(list);
      for (const d of list) {
        invoke<{ tools_supported: boolean }>("local_tool_capability", { path: d.path })
          .then((c) => setToolCaps((m) => ({ ...m, [d.path]: c.tools_supported })))
          .catch(() => {});
      }
    } catch { /* ignore */ }
  }

  useEffect(() => {
    invoke<HW>("detect_hardware").then(setHw).catch(() => {});
    refreshDownloaded();
    refreshMlxPulled();
    refreshMlxServe();
  }, []);

  // click outside to close dropdown
  useEffect(() => {
    function onDoc(e: MouseEvent) {
      if (!wrapRef.current) return;
      if (!wrapRef.current.contains(e.target as Node)) setFocused(false);
    }
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, []);

  // debounced Hugging Face search — auto-fills model strings as you type
  useEffect(() => {
    const q = query.trim();
    if (q.length < 2) {
      setResults([]);
      setSearching(false);
      setErr(null);
      return;
    }
    // if a valid selection is showing and query still equals its repo, keep it (don't re-search to avoid flicker)
    if (selected && q === selected.repo) {
      setResults([]);
      return;
    }
    // repo-id exact path: we don't live-search for "author/name" while typing it — wait for Enter/blur to lookup
    // but if user is still mid-typing a repo-like string with no space, we still offer search results for convenience
    const seq = ++seqRef.current;
    const t = setTimeout(async () => {
      setSearching(true);
      setErr(null);
      try {
        const res = await invoke<{ hardware: HW; models: CatModel[] }>("local_search", { query: q, limit: 8 });
        if (seq !== seqRef.current) return;
        setHw(res.hardware);
        setResults(res.models || []);
        setHighlight(-1);
      } catch (e) {
        if (seq !== seqRef.current) return;
        setErr(String(e));
        setResults([]);
      } finally {
        if (seq === seqRef.current) setSearching(false);
      }
    }, 280);
    return () => clearTimeout(t);
  }, [query, selected]);

  async function pickModel(m: CatModel) {
    setSelected(m);
    setQuery(m.repo);
    setResults([]);
    setFocused(false);
    setHighlight(-1);
    setErr(null);
  }

  async function resolveExactRepo(repo: string) {
    const r = repo.trim();
    if (!isRepoId(r)) return false;
    setSearching(true);
    setErr(null);
    try {
      const m = await invoke<CatModel>("local_lookup", { repoId: r });
      // local_lookup returns a single scored model object; normalize shape
      // Some backends return { ...model } directly, others may wrap — handle both
      const model = (m as any).repo ? (m as CatModel) : (m as any).model as CatModel;
      if (model && (model as any).repo) {
        setSelected(model);
        setQuery((model as any).repo);
        setResults([]);
        setFocused(false);
        return true;
      }
      // fallback: if lookup returned wrapper, try to use it
      if ((m as any).repo) {
        setSelected(m as CatModel);
        setQuery((m as any).repo);
        setResults([]);
        setFocused(false);
        return true;
      }
    } catch (e) {
      setErr(String(e));
    } finally {
      setSearching(false);
    }
    return false;
  }

  async function handleSubmit() {
    const q = query.trim();
    if (!q) return;
    // exact repo id -> lookup and show card
    if (isRepoId(q)) {
      const ok = await resolveExactRepo(q);
      if (ok) return;
    }
    // otherwise if highlighted result exists, pick it
    if (highlight >= 0 && results[highlight]) {
      await pickModel(results[highlight]);
      return;
    }
    // if single result, auto-pick
    if (results.length === 1) {
      await pickModel(results[0]);
      return;
    }
    // no exact match — keep dropdown open for disambiguation
  }

  async function download(q: Quant) {
    const channel = `dl-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    setErr(null);
    setProgress((p) => ({ ...p, [q.filename]: 0 }));
    const un = await listen<any>(channel, (e) => {
      const { got, total, done } = e.payload || {};
      setProgress((p) => ({ ...p, [q.filename]: done ? 1 : (total ? got / total : 0) }));
    });
    try {
      await invoke("local_download", { channel, url: q.download_url, filename: q.filename });
      await refreshDownloaded();
    } catch (e) { setErr(`Download failed: ${String(e)}`); }
    finally { un(); setProgress((p) => { const n = { ...p }; delete n[q.filename]; return n; }); }
  }

  async function mlxPull(repo: string) {
    const channel = `mlx-pull-${Date.now()}`;
    setPulling(repo); setPullMsg(null); setErr(null); setPullProg(null);
    const un = await listen<any>(channel, (e) => {
      const pl = e.payload || {};
      if (pl.done) { setPullProg(null); return; }
      const { file, index, files, got, total } = pl;
      setPullProg({ file: file || "", index: index || 0, files: files || 0, pct: total ? got / total : 0 });
    });
    try {
      await invoke("mlx_pull_cmd", { channel, repo });
      setPullMsg(`✓ ${repo} cached — pick it in Agents (Local MLX)`);
      await refreshMlxPulled();
      await refreshMlxServe();
    } catch (e) { setPullMsg(`✗ ${String(e)}`); }
    finally { un(); setPulling(null); setPullProg(null); }
  }

  async function refreshMlxPulled() {
    try { setMlxPulled(await invoke<Array<{ repo: string; path: string; size_gb: number }>>("mlx_downloaded")); }
    catch { /* best-effort */ }
  }
  async function refreshMlxServe() {
    try {
      const st = await invoke<{ running: boolean; repo: string | null }>("mlx_status");
      setMlxServe({ running: !!st?.running, repo: st?.repo ?? null });
    } catch { /* best-effort */ }
  }
  async function mlxStopServing() {
    try { await invoke("mlx_stop_cmd"); await refreshMlxServe(); }
    catch (e) { setErr(String(e)); }
  }
  async function mlxDel(repo: string) {
    try { await invoke("mlx_delete_cmd", { repo }); await refreshMlxPulled(); }
    catch (e) { setErr(String(e)); }
  }
  async function del(d: Downloaded) {
    try { await invoke("local_delete", { filename: d.filename }); await refreshDownloaded(); if (selected && d.filename && selected.quants.some(q => q.filename === d.filename)) { /* keep card */ } }
    catch (e) { setErr(String(e)); }
  }

  const isDown = (fname: string) => downloaded.some((d) => d.filename === fname);
  const showDropdown = focused && results.length > 0;

  function renderModelCard(m: CatModel) {
    if (m.family === "mlx") {
      const q = m.quants[0];
      const busy = pulling === m.repo;
      return (
        <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 6, paddingTop: 12, borderTop: "var(--border-width) solid var(--line)" }}>
          <div style={{ display: "flex", alignItems: "baseline", gap: 8, flexWrap: "wrap" }}>
            <span style={{ fontWeight: 800, fontSize: 15 }}>{m.name}</span>
            <Pill tone="ok">MLX · Apple silicon</Pill>
            <span style={{ fontFamily: "ui-monospace, monospace", fontSize: 11, color: "var(--text-faint)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{m.repo}</span>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
            <span style={{ fontSize: 13, color: "var(--text-muted)", flex: 1 }}>~{q ? q.size_gb.toFixed(1) : "?"}GB download · {q ? q.quant : ""} · whole repo (safetensors)</span>
            {busy && pullProg ? <span style={{ fontSize: 12, fontFamily: "ui-monospace, monospace", color: "var(--text-muted)" }}>{pullProg.file} ({pullProg.index + 1}/{pullProg.files}) · {Math.round(pullProg.pct * 100)}%</span> : <Button variant="secondary" onClick={() => void mlxPull(m.repo)} disabled={!!pulling}>{busy ? "Pulling…" : "Pull"}</Button>}
          </div>
          {pullMsg && <div style={{ fontSize: 13, fontWeight: 600, color: pullMsg.startsWith("✗") ? "var(--danger)" : "var(--ok)", overflowWrap: "anywhere" }}>{pullMsg}</div>}
          <span style={{ fontSize: 12, color: "var(--text-faint)" }}>MLX models run chat-only (no file tools) via the Local (MLX) provider — pick this repo in any agent’s setup after pulling.</span>
        </div>
      );
    }
    const rec = m.quants[0];
    const perf = rec?.perf;
    const ctx = contextWords(m.context_tokens);
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 6, paddingTop: 12, borderTop: "var(--border-width) solid var(--line)" }}>
        <div style={{ display: "flex", flexDirection: "column", gap: 3 }}>
          <div style={{ display: "flex", alignItems: "baseline", gap: 8, flexWrap: "wrap" }}>
            <span style={{ fontWeight: 800, fontSize: 15 }}>{m.name}</span>
            <span style={{ fontSize: 12, color: "var(--text-faint)" }}>{m.family_label} · {m.params_billions}B</span>
            <span style={{ fontFamily: "ui-monospace, monospace", fontSize: 11, color: "var(--text-faint)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{m.repo}</span>
          </div>
          <div style={{ display: "flex", gap: 14, flexWrap: "wrap", fontSize: 13 }}>
            {perf && (
              <span title={perf.note}>
                {perf.badge} <b>Speed on your Mac:</b>{" "}
                {perf.fits ? <>{speedWords(perf)}{perf.tokens_per_sec && <span style={{ color: "var(--text-faint)" }}> ({perf.tokens_per_sec})</span>}</> : "won't fit"}
              </span>
            )}
            <span title={ctx.long}>
              🧠 <b>Memory:</b> {ctx.short === "—" ? "unknown" : <>{ctx.short} tokens <span style={{ color: "var(--text-faint)" }}>(≈ a {m.context_tokens >= 100000 ? "whole book" : m.context_tokens >= 30000 ? "long essay" : "few pages"} of conversation)</span></>}
            </span>
          </div>
        </div>
        {m.quants.map((q) => {
          const pct = progress[q.filename];
          const downloading = pct !== undefined;
          return (
            <div key={q.filename} style={{ display: "flex", alignItems: "center", gap: 12, paddingLeft: 4 }}>
              {(() => { const b = quantBlurb(q.quant); return (
              <span style={{ display: "flex", flexDirection: "column", flex: 1, minWidth: 0 }}>
                <span style={{ fontSize: 13, fontWeight: 700 }}>{b.title} <span style={{ fontWeight: 400, color: "var(--text-muted)" }}>— {b.sub}</span></span>
                <span style={{ fontFamily: "ui-monospace, monospace", fontSize: 10, color: "var(--text-faint)" }}>~{q.size_gb.toFixed(1)}GB download · {q.quant} · {q.filename}</span>
              </span>
              ); })()}
              {isDown(q.filename)
                ? <Pill tone="ok">installed ✓</Pill>
                : downloading
                  ? <span style={{ fontSize: 12, fontFamily: "ui-monospace, monospace", width: 90, textAlign: "right" }}>{Math.round(pct * 100)}%</span>
                  : <Button variant="secondary" onClick={() => download(q)} disabled={q.perf.tier === "wont_fit"}>Download</Button>}
            </div>
          );
        })}
      </div>
    );
  }

  return (
    <Card title="Local Models">
      <p style={hint}>Download and run open models entirely on your machine — no accounts, no cloud, fully private. Powered by an engine built right into AYGENT.</p>
      {hw && <Pill tone="muted">🖥 {hw.summary}</Pill>}

      {downloaded.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: 6, marginTop: 4 }}>
          <span style={{ fontSize: 13, fontWeight: 700, color: "var(--text-muted)" }}>Installed</span>
          {downloaded.map((d) => {
            const toolable = toolCaps[d.path];
            return (
            <div key={d.filename} style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <div style={{ flex: 1, border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)", padding: "10px 14px", background: "var(--bg)", boxShadow: "var(--elevation)" }}>
                <div style={{ display: "flex", alignItems: "baseline", gap: 10, flexWrap: "wrap" }}>
                  <b style={{ fontSize: 14 }}>{d.filename.replace(/\.gguf$/i, "")}</b>
                  <span style={{ ...hint, fontFamily: "ui-monospace, monospace", fontSize: 12 }}>{d.size_gb.toFixed(1)}GB · local</span>
                </div>
                <span style={{ ...hint, fontSize: 12 }}>
                  {toolable === undefined ? "installed" : toolable ? "✓ works with file tools" : "chat only — no file tools"}
                </span>
              </div>
              <Button variant="secondary" onClick={() => del(d)}>Delete</Button>
            </div>
            );
          })}
        </div>
      )}
      {downloaded.length > 0 && (
        <p style={{ ...hint, color: "var(--text-faint)", fontSize: 12 }}>
          Downloaded models show up in each agent’s setup (Agents tab) to be selected. Models marked
          “works with file tools” can read &amp; write files in the agent’s folder — and every change is
          a Save Point, so you can always rewind.
        </p>
      )}
      {mlxPulled.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: 6, marginTop: 4 }}>
          <span style={{ fontSize: 13, fontWeight: 700, color: "var(--text-muted)" }}>Installed · MLX</span>
          {mlxPulled.map((m) => (
            <div key={m.repo} style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <div style={{ flex: 1, border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)", padding: "10px 14px", background: "var(--bg)", boxShadow: "var(--elevation)" }}>
                <div style={{ display: "flex", alignItems: "baseline", gap: 10, flexWrap: "wrap" }}>
                  <b style={{ fontSize: 14 }}>{m.repo}</b>
                  <span style={{ ...hint, fontFamily: "ui-monospace, monospace", fontSize: 12 }}>{m.size_gb.toFixed(1)}GB · MLX</span>
                </div>
                <span style={{ ...hint, fontSize: 12 }}>chat only — no file tools</span>
              </div>
              <Button variant="secondary" onClick={() => void mlxDel(m.repo)}>Delete</Button>
            </div>
          ))}
        </div>
      )}

      {mlxServe?.running && (
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <Pill tone="ok">MLX serving {mlxServe.repo}</Pill>
          <Button variant="secondary" onClick={() => void mlxStopServing()}>Stop</Button>
        </div>
      )}
      {/* OpenRouter-style search */}
      <div ref={wrapRef} style={{ position: "relative", marginTop: 6 }}>
        <div style={{ display: "flex", gap: 8 }}>
          <div style={{ flex: 1, position: "relative" }}>
            <span style={{ position: "absolute", left: 12, top: "50%", transform: "translateY(-50%)", color: "var(--text-faint)", fontSize: 14, pointerEvents: "none" }}>⌕</span>
            <input
              ref={inputRef}
              value={query}
              onChange={(e) => { setQuery(e.target.value); if (selected && e.target.value !== selected.repo) setSelected(null); }}
              onFocus={() => setFocused(true)}
              onKeyDown={(e) => {
                if (e.key === "ArrowDown") { e.preventDefault(); setFocused(true); setHighlight((h) => Math.min(results.length - 1, h + 1)); }
                else if (e.key === "ArrowUp") { e.preventDefault(); setHighlight((h) => Math.max(-1, h - 1)); }
                else if (e.key === "Enter") { e.preventDefault(); void handleSubmit(); }
                else if (e.key === "Escape") { setFocused(false); setHighlight(-1); }
              }}
              placeholder="Search Hugging Face — 'Qwen', 'Bonsai', or paste a repo like bartowski/Qwen3-14B-GGUF or mlx-community/Qwen3-4B-4bit"
              style={{
                width: "100%", padding: "10px 36px 10px 32px",
                background: "var(--bg)", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)",
                color: "var(--text)", fontSize: 14, boxShadow: "var(--elevation)", outline: "none",
              }}
            />
            {query && (
              <button
                onClick={() => { setQuery(""); setResults([]); setSelected(null); setErr(null); inputRef.current?.focus(); }}
                style={{
                  position: "absolute", right: 8, top: "50%", transform: "translateY(-50%)",
                  background: "var(--surface)", border: "var(--border-width) solid var(--line)", borderRadius: 999,
                  width: 22, height: 22, display: "flex", alignItems: "center", justifyContent: "center",
                  cursor: "pointer", color: "var(--text-muted)", fontSize: 12, lineHeight: 1,
                }}
                aria-label="Clear"
              >✕</button>
            )}
          </div>
          <Button variant="secondary" onClick={() => void handleSubmit()} disabled={searching}>
            {searching ? "…" : "Go"}
          </Button>
        </div>

        {/* dropdown */}
        {showDropdown && (
          <div style={{
            position: "absolute", left: 0, right: 48, top: "calc(100% + 6px)", zIndex: 20,
            background: "var(--surface)", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
            boxShadow: "var(--elevation)", overflow: "hidden", maxHeight: 360, overflowY: "auto",
          }}>
            {results.map((m, idx) => (
              <button
                key={m.repo}
                onMouseEnter={() => setHighlight(idx)}
                onClick={() => void pickModel(m)}
                style={{
                  display: "flex", flexDirection: "column", gap: 2, width: "100%", textAlign: "left", cursor: "pointer",
                  padding: "10px 12px", border: "none", borderBottom: "var(--border-width) solid var(--line)",
                  background: highlight === idx ? "var(--bg)" : "var(--surface)", color: "var(--text)", font: "inherit",
                }}
              >
                <span style={{ display: "flex", alignItems: "baseline", gap: 8, flexWrap: "wrap" }}>
                  <span style={{ fontWeight: 700, fontSize: 13 }}>{m.repo}</span>
                  <span style={{ fontSize: 11, color: "var(--text-faint)" }}>{m.family_label} · {m.params_billions}B · {m.quants.map(q=>q.quant).join(", ")}</span>
                </span>
                <span style={{ fontSize: 12, color: "var(--text-muted)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{m.name} · {m.downloads.toLocaleString()} downloads</span>
              </button>
            ))}
            <div style={{ padding: "8px 12px", fontSize: 11, color: "var(--text-faint)" }}>
              {searching ? "Searching Hugging Face…" : `${results.length} result${results.length===1?"":"s"} — auto-filled from Hugging Face`}
            </div>
          </div>
        )}

        {searching && !showDropdown && <div style={{ fontSize: 12, color: "var(--text-faint)", marginTop: 6 }}>Searching Hugging Face…</div>}
        {err && <Pill tone="danger">✗ {err}</Pill>}
        {!searching && focused && !showDropdown && query.trim().length >= 2 && results.length === 0 && !selected && (
          <div style={{ fontSize: 12, color: "var(--text-faint)", marginTop: 6 }}>No models found. Try a broader term or paste an exact repo id like <code>bartowski/Qwen3-14B-GGUF</code> and press Go.</div>
        )}
      </div>

      {/* selected info card — same data as before */}
      {selected && renderModelCard(selected)}

      {selected && selected.family !== "mlx" && (
        <div style={{ ...hint, fontSize: 12, color: "var(--text-faint)", marginTop: 8, display: "flex", flexDirection: "column", gap: 3 }}>
          <span><b>Which download should I pick?</b> They're the exact same model at different compression. “Recommended” is nearly identical quality in a smaller file; “Efficient” squeezes big models onto modest memory with a slight quality dip; “Higher quality” needs the most memory. A 🟡 badge means it runs split across GPU + CPU — it works, but the Efficient file will feel much faster.</span>
          <span><b>Speed</b> (“tok/s” = tokens per second) is how fast the AI types. ~15+ feels quick; under ~8 feels sluggish. <b>Memory</b> is how much conversation the model can keep in mind at once. Estimates, not benchmarks.</span>
        </div>
      )}

      {!selected && (
        <p style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>
          Start typing to search Hugging Face — results auto-fill as you type. Paste an exact repo id (e.g. <code>bartowski/Qwen3-14B-GGUF</code>) and press Enter/Go to jump straight to its info card with the same speed &amp; download choices.
        </p>
      )}
    </Card>
  );
}

// ---------------------------------------------------------------------------


// (MLX lives inside Local Models above — no separate section.)

// One provider key row: shows key status, save/replace, and a live Test that
// hits that provider's /models endpoint. Keys go straight to Keychain.
function ProviderRow({ provider, label, placeholder }: { provider: string; label: string; placeholder: string }) {
  const [key, setKey] = useState("");
  const [set, setSet] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  useEffect(() => { invoke<boolean>("has_provider_key", { provider }).then(setSet).catch(() => {}); }, [provider]);
  async function save() {
    if (!key.trim()) return;
    await invoke("set_provider_key", { provider, key: key.trim() });
    setKey(""); setSet(true); setMsg(null);
  }
  async function test() {
    setMsg("testing…");
    try {
      // provider_verify_key actually round-trips auth (Mason 08-02: OpenRouter's
      // old check hit a PUBLIC /models endpoint that says "connected" even for
      // an empty/bad key — this was silently lying).
      await invoke("provider_verify_key", { provider });
      setMsg("✓ key verified — auth works");
    } catch (e) { setMsg("✗ " + String(e)); }
  }
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6, paddingTop: 6 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <span style={{ fontSize: 14, fontWeight: 700, width: 100 }}>{label}</span>
        {set ? <Pill tone="ok">key set ✓</Pill> : <Pill tone="muted">no key</Pill>}
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <Input type="password" mono value={key} onChange={(e) => setKey(e.target.value)} placeholder={set ? "replace key…" : placeholder} />
        <Button onClick={save}>{set ? "Replace" : "Save"}</Button>
        {set && <Button variant="secondary" onClick={test}>Test</Button>}
      </div>
      {msg && <Pill tone={msg.startsWith("✗") ? "danger" : "ok"}>{msg}</Pill>}
    </div>
  );
}

// re-export so App can persist through the same helper
export { saveTheme };

// --- AYGENT REMOTE ----------------------------------------------------------
// Pair this Mac with masonlee.build/remote: enter the site's 8-char code, see
// live status + the SAS to compare against the browser, unpair. The device is
// the source of truth for capability — this card only manages the session.
type RemoteStatus = {
  paired: boolean; running: boolean; enabled: boolean; site: string;
};

function RemoteCard() {
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  async function refresh() {
    try { setStatus(await invoke<RemoteStatus>("remote_status")); }
    catch { /* status is best-effort */ }
  }
  useEffect(() => { refresh(); }, []);

  async function pair() {
    if (!code.trim() || busy) return;
    setBusy(true); setMsg(null);
    try {
      await invoke("remote_pair", { code: code.trim() });
      setCode(""); setMsg("✓ paired — open masonlee.build/remote in any browser you're logged into");
      await refresh();
    } catch (e) { setMsg("✗ " + String(e)); }
    finally { setBusy(false); }
  }
  async function unpair() {
    setBusy(true); setMsg(null);
    try { await invoke("remote_unpair"); setMsg("✓ unpaired — keys wiped"); await refresh(); }
    catch (e) { setMsg("✗ " + String(e)); }
    finally { setBusy(false); }
  }
  async function connect() {
    setBusy(true); setMsg(null);
    try {
      const up = await invoke<boolean>("remote_connect");
      setMsg(up ? "✓ session up" : "waiting for the browser — open /remote on the site first");
      await refresh();
    } catch (e) { setMsg("✗ " + String(e)); }
    finally { setBusy(false); }
  }

  return (
    <Card title="AYGENT Remote">
      {!status?.paired ? (
        <>
          <p style={hint}>
            Use your agents from any browser. Get a pairing code at{" "}
            <b>masonlee.build/remote</b>, then enter it here.
          </p>
          <div style={{ display: "flex", gap: "var(--space-2)" }}>
            <Input
              value={code}
              onChange={(e) => setCode(e.target.value.toUpperCase())}
              placeholder="8-character code"
              maxLength={8}
              style={{ width: 180, textTransform: "uppercase", letterSpacing: 2 }}
            />
            <Button onClick={pair} disabled={busy || code.trim().length !== 8}>Pair</Button>
          </div>
        </>
      ) : (
        <>
          <div style={{ display: "flex", alignItems: "center", gap: "var(--space-2)", flexWrap: "wrap" }}>
            <Pill tone={status.running ? "ok" : "muted"}>
              {status.running ? "connected" : status.enabled ? "paired · connecting" : "paired · offline"}
            </Pill>
            <span style={{ fontSize: "var(--text-caption)", color: "var(--text-muted)" }}>
              your account is paired — any browser you're logged into can chat
            </span>
          </div>
          <div style={{ display: "flex", gap: "var(--space-2)", marginTop: "var(--space-2)" }}>
            {!status.running && <Button onClick={connect} disabled={busy}>Connect now</Button>}
            <Button onClick={unpair} disabled={busy} variant="secondary">Unpair</Button>
          </div>
        </>
      )}
      {msg && <p style={{ ...hint, marginTop: "var(--space-2)" }}>{msg}</p>}
    </Card>
  );
}
