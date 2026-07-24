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
type ModelInfo = { label: string; inPrice: number; outPrice: number; context: string; blurb: string };

// Ordered rules: FIRST match wins, so more specific ids come before general
// family fallbacks. Context values are the real published windows.
function modelInfo(id: string): ModelInfo {
  const has = (s: string) => id.includes(s);

  // ---- 1M-token context models (from Anthropic docs) ----
  // Fable 5 / Mythos 5 — top tier.
  if (has("fable") || has("mythos"))
    return { label: "Fable", inPrice: 10, outPrice: 50, context: "1M", blurb: "next-gen — most capable, for long-running agents" };

  // Opus: 5 and 4.6/4.7/4.8 are 1M; older Opus (4.5/4.1/4) are 200k.
  if (has("opus")) {
    const oneM = has("opus-5") || has("opus-4-6") || has("opus-4-7") || has("opus-4-8");
    // Opus 5 pricing dropped to $5/$25; older Opus were $15/$75.
    const price = has("opus-5") ? { i: 5, o: 25 } : { i: 15, o: 75 };
    return { label: "Opus", inPrice: price.i, outPrice: price.o, context: oneM ? "1M" : "200k", blurb: "deepest reasoning — best for hard problems" };
  }

  // Sonnet: 5 and 4.6 are 1M; 4.5 and earlier are 200k.
  if (has("sonnet")) {
    const oneM = has("sonnet-5") || has("sonnet-4-6");
    return { label: "Sonnet", inPrice: 3, outPrice: 15, context: oneM ? "1M" : "200k", blurb: "balanced — great default for real work" };
  }

  // ---- 200k-token context models ----
  // Haiku is always 200k. 4.5 is $1/$5; older 3.5/3 were cheaper.
  if (has("haiku")) {
    const price = has("haiku-4-5") ? { i: 1, o: 5 } : { i: 0.8, o: 4 };
    return { label: "Haiku", inPrice: price.i, outPrice: price.o, context: "200k", blurb: "fast + cheap — everyday tasks" };
  }

  // Legacy Claude 2.x — 100k.
  if (has("claude-2")) return { label: id, inPrice: 8, outPrice: 24, context: "100k", blurb: "legacy model" };

  // Unknown/new id: default to 200k (the conservative, most-common window)
  // rather than over-claiming 1M.
  return { label: id, inPrice: 0, outPrice: 0, context: "200k", blurb: "" };
}
function fmtPrice(n: number) { return n === 0 ? "?" : (n < 1 ? `$${n.toFixed(2)}` : `$${n}`); }

export function Settings({
  mode, accent, onTheme, folder, onPickFolder,
}: {
  mode: Mode; accent: string; onTheme: (m: Mode, a: string) => void;
  folder: string | null; onPickFolder: () => void;
}) {
  const [apiKey, setApiKey] = useState("");
  const [keySet, setKeySet] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);
  const [retention, setRetention] = useState(30);
  const [cpMsg, setCpMsg] = useState<string | null>(null);
  const [confirmPurge, setConfirmPurge] = useState(false);
  const [models, setModels] = useState<string[]>([]);
  const [selModel, setSelModel] = useState(""); // "" = auto (haiku)
  const [selProvider, setSelProvider] = useState(""); // "" = anthropic, "local"
  const [modelMsg, setModelMsg] = useState<string | null>(null);

  useEffect(() => { invoke<boolean>("has_provider_key", { provider: "anthropic" }).then(setKeySet).catch(() => {}); }, []);
  useEffect(() => {
    if (!folder) return;
    invoke<number>("checkpoint_get_retention").then(setRetention).catch(() => {});
    invoke<{ provider: string; model: string }>("get_selection", { folder })
      .then((s) => { setSelProvider(s.provider); setSelModel(s.model); }).catch(() => {});
  }, [folder]);

  // Load the live model list once a key is set (real models this key can use).
  useEffect(() => {
    if (!keySet) return;
    invoke<string[]>("anthropic_models").then(setModels).catch(() => {});
  }, [keySet]);

  async function chooseModel(model: string) {
    if (!folder) return;
    setSelProvider(""); setSelModel(model); setModelMsg(null);
    try {
      await invoke("set_selection", { folder, provider: "", model });
      setModelMsg(model === "" ? "✓ auto (Haiku — fast + cheap)" : `✓ using ${modelInfo(model).label}`);
    } catch (e) { setModelMsg("✗ " + String(e)); }
  }

  // Choose a downloaded LOCAL model (provider="local", model=absolute gguf path).
  async function chooseLocalModel(path: string, name: string) {
    if (!folder) return;
    setSelProvider("local"); setSelModel(path); setModelMsg(null);
    try {
      await invoke("set_selection", { folder, provider: "local", model: path });
      setModelMsg(`✓ using ${name} (local)`);
    } catch (e) { setModelMsg("✗ " + String(e)); }
  }

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

      {/* APPEARANCE */}
      <Card title="Appearance">
        <p style={hint}>The theme controls the lighting — light casts soft shadows, dark emits glows.</p>
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
        <p style={{ ...hint, color: "var(--text-faint)", fontSize: 12 }}>The accent tints outlines and the shadow/glow — not just buttons.</p>
      </Card>

      {/* PROVIDERS */}
      <Card title="Providers">
        <p style={hint}>Bring your own keys. They go straight to the macOS Keychain — the UI never keeps them.</p>
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <span style={{ fontSize: 14, fontWeight: 700, width: 90 }}>Anthropic</span>
          {keySet ? <Pill tone="ok">key set ✓</Pill> : <Pill tone="muted">no key</Pill>}
        </div>
        <div style={{ display: "flex", gap: 8 }}>
          <Input type="password" mono value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder={keySet ? "replace key…" : "sk-ant-…"} />
          <Button onClick={saveKey}>{keySet ? "Replace" : "Save"}</Button>
          {keySet && <Button variant="secondary" onClick={testKey}>Test</Button>}
        </div>
        {testResult && <Pill tone={testResult.startsWith("✗") ? "danger" : "ok"}>{testResult}</Pill>}
        <p style={{ ...hint, color: "var(--text-faint)", fontSize: 12 }}>OpenAI · OpenRouter · local Ollama — coming in this build.</p>
      </Card>

      {/* MODEL */}
      <Card title="Model">
        <p style={hint}>Which brain your agent runs on. Saved per folder — a serious project can run Opus while a scratch folder stays on Haiku.</p>
        {!folder ? (
          <p style={{ ...hint, color: "var(--text-faint)" }}>Pick an Agent Folder below to choose a model.</p>
        ) : !keySet ? (
          <p style={{ ...hint, color: "var(--text-faint)" }}>Add an Anthropic key above to load your available models.</p>
        ) : models.length === 0 ? (
          <p style={{ ...hint, color: "var(--text-faint)" }}>Loading models…</p>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            {/* AUTO option */}
            <ModelRow
              active={selModel === ""}
              onClick={() => chooseModel("")}
              title="Auto"
              sub="picks Haiku — fast + cheap — automatically"
              meta="default"
            />
            {models.map((m) => {
              const info = modelInfo(m);
              return (
                <ModelRow
                  key={m}
                  active={selModel === m}
                  onClick={() => chooseModel(m)}
                  title={info.label}
                  sub={info.blurb || m}
                  meta={`${fmtPrice(info.inPrice)} in · ${fmtPrice(info.outPrice)} out / 1M tok · ${info.context} ctx`}
                  mono={m}
                />
              );
            })}
            {modelMsg && <Pill tone={modelMsg.startsWith("✗") ? "danger" : "ok"}>{modelMsg}</Pill>}
            <p style={{ ...hint, color: "var(--text-faint)", fontSize: 12 }}>Prices are per million tokens (input · output). Context = how much the model can hold in one conversation.</p>
          </div>
        )}
      </Card>

      {/* LOCAL MODELS — download + run GGUF models entirely in-app, no external tools */}
      <LocalModels
        folder={folder}
        activePath={selProvider === "local" ? selModel : ""}
        onChoose={chooseLocalModel}
      />

      {/* CHECKPOINTS */}
      <Card title="Checkpoints">
        <p style={hint}>Every change your agent makes is snapshotted so you can rewind. Keep history for a window, then it prunes automatically.</p>
        {!folder ? (
          <p style={{ ...hint, color: "var(--text-faint)" }}>Pick an Agent Folder below to configure checkpoints.</p>
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
                  <span style={{ ...hint, color: "var(--danger)", fontSize: 13 }}>Deletes all checkpoints (your files are untouched).</span>
                </>
              )}
            </div>
            {cpMsg && <Pill tone={cpMsg.startsWith("✗") ? "danger" : "ok"}>{cpMsg}</Pill>}
          </>
        )}
      </Card>

      {/* AGENT FOLDER */}
      <Card title="Agent Folder">
        <p style={hint}>The one folder your agent can touch. Everything else on your Mac is invisible to it.</p>
        {folder
          ? <code style={{ fontSize: 13, wordBreak: "break-all", fontFamily: "ui-monospace, monospace" }}>🔒 {folder}</code>
          : <p style={{ ...hint, color: "var(--text-faint)" }}>No folder chosen — the agent can touch nothing yet.</p>}
        <div><Button onClick={onPickFolder}>{folder ? "Change folder…" : "Choose folder…"}</Button></div>
      </Card>
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
              <ModelRow
                active={activePath === d.path}
                onClick={() => folder && onChoose(d.path, d.filename.replace(/\.gguf$/i, ""))}
                title={d.filename.replace(/\.gguf$/i, "")}
                sub={toolable === undefined
                  ? (folder ? "click to use this model" : "pick an Agent Folder to use")
                  : (toolable ? "✓ works with file tools" : "chat only — no file tools")}
                meta={`${d.size_gb.toFixed(1)}GB · local`}
              />
              <Button variant="secondary" onClick={() => del(d)}>Delete</Button>
            </div>
            );
          })}
        </div>
      )}
      {downloaded.length > 0 && (
        <p style={{ ...hint, color: "var(--text-faint)", fontSize: 12 }}>
          Models marked “works with file tools” can read &amp; write files in your Agent Folder. Smaller
          models are less reliable at it than cloud models — and every change is checkpointed, so you can
          always rewind.
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

// re-export so App can persist through the same helper
export { saveTheme };
