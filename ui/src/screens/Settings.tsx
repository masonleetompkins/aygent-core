// Settings — THE WEDGE. Where "no config files, ever" becomes visible and
// delightful. Appearance (theme lives here now, not a debug bar), Providers
// (BYO keys -> Keychain), and the Agent Folder scope. Themed via tokens.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card, Button, Input, Pill } from "../components/ui";
import { saveTheme, type Mode } from "../lib/theme";

const ACCENT_SWATCHES = ["", "#2dd4bf", "#6366f1", "#e0533d", "#22c55e", "#eab308", "#ec4899"];
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
  mode, accent, onTheme, folder, onPickFolder, agentId,
}: {
  mode: Mode; accent: string; onTheme: (m: Mode, a: string) => void;
  folder: string | null; onPickFolder: () => void; agentId: string | null;
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
    invoke<number>("checkpoint_get_retention").then(setRetention).catch(() => {});
  }, [folder]);

  async function saveRetention(days: number) {
    setRetention(days); setCpMsg(null);
    try { await invoke("checkpoint_set_retention", { days }); setCpMsg(`✓ keeping ${days} days`); }
    catch (e) { setCpMsg("✗ " + String(e)); }
  }
  async function purgeAll() {
    setConfirmPurge(false); setCpMsg(null);
    try { await invoke("checkpoint_purge"); setCpMsg("✓ all checkpoints purged"); }
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
    <div style={{ display: "flex", flexDirection: "column", gap: 18, maxWidth: 620 }}>
      <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Settings</h2>

      {/* APPEARANCE — at the top (Mason cleanup #1). */}
      <Card title="Appearance">
        <div style={{ display: "flex", gap: 10, alignItems: "center" }}>
          <span style={{ fontSize: 14, fontWeight: 600, width: 90 }}>Mode</span>
          <Button variant={mode === "light" ? "primary" : "secondary"} onClick={() => onTheme("light", accent)}>◐ Light</Button>
          <Button variant={mode === "dark" ? "primary" : "secondary"} onClick={() => onTheme("dark", accent)}>◑ Dark</Button>
        </div>
        <div style={{ display: "flex", gap: 12, alignItems: "center", marginTop: 4 }}>
          <span style={{ fontSize: 14, fontWeight: 600, width: 90 }}>Accent</span>
          <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            {ACCENT_SWATCHES.map((c) => (
              <button key={c || "default"} onClick={() => onTheme(mode, c)} title={c || "default (black/white)"}
                style={{
                  width: 26, height: 26, borderRadius: 999, cursor: "pointer",
                  border: `2px solid ${accent === c ? "var(--text)" : "var(--line)"}`,
                  background: c || (mode === "light" ? "#0a0a0a" : "#ffffff"),
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
      </Card>

      {/* MODEL card removed (Mason cleanup #3) — model choice lives per-agent in
          the Agents tab. */}

      {/* LOCAL MODELS — DOWNLOAD-ONLY here (Mason cleanup #4). Downloaded models
          show up per-agent in the Agents tab to be selected; no selection here. */}
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
          <label style={{ display: "flex", gap: 8, alignItems: "center", fontSize: 14, fontWeight: 600 }}>
            <input type="checkbox" checked={autoRemember} onChange={(e) => toggleAutoRemember(e.target.checked)} />
            Auto-remember from conversation
          </label>
        )}
      </Card>

      {/* SAVE POINTS (formerly Checkpoints — Mason rename). */}
      <Card title="Save Points">
        <p style={hint}>Every change your agent makes is a Save Point so you can rewind. Keep history for a window, then it prunes automatically.</p>
        {!folder ? (
          <p style={{ ...hint, color: "var(--text-faint)" }}>Pick an agent with a folder to configure Save Points.</p>
        ) : (
          <>
            <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
              <span style={{ fontSize: 14, fontWeight: 600, width: 90 }}>Keep for</span>
              <input
                type="range" min={1} max={90} value={retention}
                onChange={(e) => setRetention(Number(e.target.value))}
                onMouseUp={(e) => saveRetention(Number((e.target as HTMLInputElement).value))}
                onTouchEnd={(e) => saveRetention(Number((e.target as HTMLInputElement).value))}
                style={{ flex: 1, accentColor: "var(--accent)" }}
              />
              <span style={{ fontFamily: "ui-monospace, monospace", fontSize: 14, fontWeight: 700, width: 64, textAlign: "right" }}>
                {retention} day{retention === 1 ? "" : "s"}
              </span>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 10, marginTop: 4 }}>
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

      {/* AGENT FOLDER card removed (Mason cleanup #6) — each agent's folder is set
          in the Agents tab, not globally here. */}
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
// Browse a live, curated GGUF catalog (Qwen/Mistral/Kimi/Llama), see how each
// will run on THIS machine, download in-app, and pick one to chat with. Fully
// self-contained: no Ollama, no terminal.
type Perf = { tier: string; badge: string; tokens_per_sec: string; note: string; fits: boolean };
type Quant = { tier: string; quant: string; filename: string; size_gb: number; download_url: string; perf: Perf };

// Turn a raw tok/s range into plain language for an inexperienced user.
function speedWords(perf: Perf): string {
  switch (perf.tier) {
    case "great": return "fast";
    case "usable": return "okay speed";
    case "slow": return "slow";
    default: return "";
  }
}

// Friendly context-window label: tokens → "~24,000 words of conversation".
// (1 token ≈ 0.75 words.) Shows the raw "128k" too for people who know it.
function contextWords(tokens: number): { short: string; long: string } {
  if (!tokens) return { short: "—", long: "context size unknown" };
  const k = Math.round(tokens / 1024);
  const words = Math.round((tokens * 0.75) / 1000);
  return { short: `${k}k`, long: `can hold about ${words.toLocaleString()},000 words of conversation` };
}

// Per-quant friendly framing: the SAME model at different compression levels.
// The tradeoff is QUALITY vs DOWNLOAD SIZE — speed is essentially the same.
// Match on the RAW quant code, not the tier label, so the two rows always read
// distinctly even if tier strings ever collide.
function quantBlurb(quant: string): { title: string; sub: string } {
  // Higher-precision quants (more bits per weight = sharper, bigger file).
  const higher = ["Q5_K_M", "Q6_K", "Q8_0"];
  if (higher.includes(quant)) {
    return { title: "Higher quality", sub: "sharper answers · larger file · needs more memory" };
  }
  return { title: "Recommended", sub: "nearly identical quality · smaller file · best for most people" };
}
type CatModel = { family: string; family_label: string; repo: string; name: string; params_billions: number; context_tokens: number; downloads: number; quants: Quant[] };
type HW = { summary: string };
type Downloaded = { filename: string; path: string; size_gb: number };

function LocalModels({ folder, activePath, onChoose }: {
  folder: string | null; activePath: string; onChoose: (path: string, name: string) => void;
}) {
  const [hw, setHw] = useState<HW | null>(null);
  const [catalog, setCatalog] = useState<CatModel[]>([]);
  const [downloaded, setDownloaded] = useState<Downloaded[]>([]);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [progress, setProgress] = useState<Record<string, number>>({}); // filename -> 0..1
  const [toolCaps, setToolCaps] = useState<Record<string, boolean>>({}); // path -> tools_supported
  const unlistenRef = useRef<null | (() => void)>(null);

  async function refreshDownloaded() {
    try {
      const list = await invoke<Downloaded[]>("local_downloaded");
      setDownloaded(list);
      // Detect each installed model's tool capability (from its GGUF template).
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
  }, []);

  async function loadCatalog() {
    setLoading(true); setErr(null);
    try {
      const res = await invoke<{ hardware: HW; models: CatModel[] }>("local_catalog", { perFamily: 4 });
      setHw(res.hardware); setCatalog(res.models);
    } catch (e) { setErr(String(e)); }
    finally { setLoading(false); }
  }

  async function download(q: Quant) {
    // Tauri event names must be simple (alphanumeric/-/_/ /:) — a channel built
    // from the filename (dots, slashes) can make listen() silently no-op, which
    // looked exactly like a frozen 0%. Use a safe, unique channel instead.
    const channel = `dl-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    setErr(null);
    setProgress((p) => ({ ...p, [q.filename]: 0 }));
    const un = await listen<any>(channel, (e) => {
      const { got, total, done } = e.payload || {};
      setProgress((p) => ({ ...p, [q.filename]: done ? 1 : (total ? got / total : 0) }));
    });
    unlistenRef.current = un;
    try {
      await invoke("local_download", { channel, url: q.download_url, filename: q.filename });
      await refreshDownloaded();
    } catch (e) { setErr(`Download failed: ${String(e)}`); }
    finally { un(); setProgress((p) => { const n = { ...p }; delete n[q.filename]; return n; }); }
  }

  async function del(d: Downloaded) {
    try { await invoke("local_delete", { filename: d.filename }); await refreshDownloaded(); }
    catch (e) { setErr(String(e)); }
  }

  const isDown = (fname: string) => downloaded.some((d) => d.filename === fname);

  return (
    <Card title="Local Models">
      <p style={hint}>Download and run open models entirely on your machine — no accounts, no cloud, fully private. Powered by an engine built right into AYGENT.</p>
      {hw && <Pill tone="muted">🖥 {hw.summary}</Pill>}

      {/* Downloaded models — pick one to chat with */}
      {downloaded.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: 6, marginTop: 4 }}>
          <span style={{ fontSize: 13, fontWeight: 700, color: "var(--text-muted)" }}>Installed</span>
          {downloaded.map((d) => {
            const toolable = toolCaps[d.path];
            return (
            <div key={d.filename} style={{ display: "flex", alignItems: "center", gap: 10 }}>
              {/* Non-selectable info row (Mason: models are picked per-agent in
                  the Agents tab, so NO radio/select here — just show what's
                  installed). */}
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

      {/* Catalog browser */}
      <div style={{ display: "flex", alignItems: "center", gap: 10, marginTop: 6 }}>
        <Button onClick={loadCatalog} disabled={loading}>{loading ? "Loading…" : catalog.length ? "Refresh catalog" : "Browse models"}</Button>
        <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Latest Qwen · Mistral · Kimi · Llama</span>
      </div>
      {err && <Pill tone="danger">✗ {err}</Pill>}

      {catalog.map((m) => {
        // Speed + context belong to the MODEL (both downloads run at the same
        // speed — the quant tradeoff is quality vs download size, not speed).
        const rec = m.quants[0];
        const perf = rec?.perf;
        const ctx = contextWords(m.context_tokens);
        return (
          <div key={m.repo} style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 6, paddingTop: 12, borderTop: "var(--border-width) solid var(--line)" }}>
            {/* MODEL HEADER: name + the two facts that matter (speed, memory) */}
            <div style={{ display: "flex", flexDirection: "column", gap: 3 }}>
              <div style={{ display: "flex", alignItems: "baseline", gap: 8, flexWrap: "wrap" }}>
                <span style={{ fontWeight: 800, fontSize: 15 }}>{m.name}</span>
                <span style={{ fontSize: 12, color: "var(--text-faint)" }}>{m.family_label} · {m.params_billions}B</span>
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
            {/* DOWNLOAD CHOICES: same model, different compression. Quality vs size. */}
            {m.quants.map((q) => {
              const pct = progress[q.filename];
              const downloading = pct !== undefined;
              return (
                <div key={q.filename} style={{ display: "flex", alignItems: "center", gap: 12, paddingLeft: 4 }}>
                  {(() => { const b = quantBlurb(q.quant); return (
                  <span style={{ display: "flex", flexDirection: "column", flex: 1, minWidth: 0 }}>
                    <span style={{ fontSize: 13, fontWeight: 700 }}>{b.title} <span style={{ fontWeight: 400, color: "var(--text-muted)" }}>— {b.sub}</span></span>
                    <span style={{ fontFamily: "ui-monospace, monospace", fontSize: 10, color: "var(--text-faint)" }}>~{q.size_gb.toFixed(1)}GB download · {q.quant}</span>
                  </span>
                  ); })()}
                  {isDown(q.filename)
                    ? <Pill tone="ok">installed ✓</Pill>
                    : downloading
                      ? <span style={{ fontSize: 12, fontFamily: "ui-monospace, monospace", width: 90, textAlign: "right" }}>{Math.round(pct * 100)}%</span>
                      : <Button variant="secondary" onClick={() => download(q)} disabled={!q.perf.fits}>Download</Button>}
                </div>
              );
            })}
          </div>
        );
      })}
      {catalog.length > 0 && (
        <div style={{ ...hint, fontSize: 12, color: "var(--text-faint)", marginTop: 8, display: "flex", flexDirection: "column", gap: 3 }}>
          <span><b>Which download should I pick?</b> Both are the exact same model at different compression. “Recommended” is nearly identical quality in a smaller file — pick it unless you have plenty of free memory and want the absolute best.</span>
          <span><b>Speed</b> (“tok/s” = tokens per second) is how fast the AI types. ~15+ feels quick; under ~8 feels sluggish. <b>Memory</b> is how much conversation the model can keep in mind at once. Estimates, not benchmarks.</span>
          <span>Local models run in chat mode; file tools are coming soon.</span>
        </div>
      )}
    </Card>
  );
}

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
      const models = provider === "anthropic"
        ? await invoke<string[]>("anthropic_models")
        : await invoke<string[]>("openai_models", { provider });
      setMsg(`✓ connected · ${models.length} models`);
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
