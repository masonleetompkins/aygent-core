import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card, Button, Input } from "../components/ui";
import { Icon, AGENT_ICONS, type IconName } from "../components/Icon";

// Onboarding v2 (3-step wizard): Welcome -> Home -> Agent Setup -> Done
// Own your agent — one folder = your whole setup. No standalone provider-key step:
// the key lives inside the Agent Setup model sub-step (Cloud tab).
// Home is picked via single native picker `onboarding_pick_root` which returns
// { path, existingRoot, nonEmpty, agentCount, chatCount }.
// After `onboarding_set_root` the live DB is already re-pointed (no restart).
// Agent home is created via `onboarding_make_agent_home` (<root>/<Name>/).
//
// Agent Setup is itself a stepped wizard (3a-3d):
//   3a Identity (name + icon, live home preview)
//   3b Model    (unified Cloud vs Local picker — Cloud: provider+key+model,
//                Local: Hugging Face search via local_search/local_lookup with
//                perf badges + download; picking local sets provider="local")
//   3c Soul     (brief -> agent_generate_soul on a lazily-created draft agent)
//   3d Superpowers (Allow Shell Access consent, scary-honest, default off)

type Step = "welcome" | "home" | "agent" | "done";
type Intent = "create" | "restore" | null;

// Local model catalog shapes (mirror Settings.tsx / catalog.rs).
type Perf = { tier: string; badge: string; tokens_per_sec: string; note: string; fits: boolean };
type Quant = { tier: string; quant: string; filename: string; size_gb: number; download_url: string; perf: Perf };
type CatModel = { family: string; family_label: string; repo: string; name: string; params_billions: number; context_tokens: number; downloads: number; quants: Quant[] };
type Downloaded = { filename: string; path: string; size_gb: number };

const SUB_LABELS = ["Identity", "Model", "Soul", "Superpowers"] as const;

export function Onboarding({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<Step>("welcome");
  const [intent, setIntent] = useState<Intent>(null);

  // Home
  const [root, setRoot] = useState<string | null>(null);
  const [pickInfo, setPickInfo] = useState<{ existingRoot: boolean; nonEmpty: boolean; agentCount: number; chatCount: number } | null>(null);
  const [rootBusy, setRootBusy] = useState(false);
  const [rootErr, setRootErr] = useState<string | null>(null);
  const [confirmMix, setConfirmMix] = useState(false);

  // Agent Setup — sub-step wizard 3a-3d
  const [sub, setSub] = useState(0);
  const [agentName, setAgentName] = useState("");
  const [agentIcon, setAgentIcon] = useState<IconName>("sparkles");
  const [pendingHome, setPendingHome] = useState<string | null>(null);
  const homeNameRef = useRef(""); // name the current pendingHome was created for
  const [agentBusy, setAgentBusy] = useState(false);
  const [agentErr, setAgentErr] = useState<string | null>(null);

  // 3b Model — unified picker
  const [tab, setTab] = useState<"cloud" | "local">("cloud");
  const [provider, setProvider] = useState("anthropic");
  const [model, setModel] = useState("");
  const [cloudKey, setCloudKey] = useState("");
  const [keyBusy, setKeyBusy] = useState(false);
  const [keyMsg, setKeyMsg] = useState<string | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);

  // 3b Local tab — HF browser
  const [query, setQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const [results, setResults] = useState<CatModel[]>([]);
  const [selected, setSelected] = useState<CatModel | null>(null);
  const [downloaded, setDownloaded] = useState<Downloaded[]>([]);
  const [progress, setProgress] = useState<Record<string, number>>({});
  const [localErr, setLocalErr] = useState<string | null>(null);

  // 3c Soul
  const [soulBrief, setSoulBrief] = useState("");
  const [systemPrompt, setSystemPrompt] = useState("");
  const [soulBusy, setSoulBusy] = useState(false);
  const [soulErr, setSoulErr] = useState<string | null>(null);

  // 3d Allow Shell Access
  const [allowShellAccess, setAllowShellAccess] = useState(false);
  const [proBusy, setProBusy] = useState(false);

  // Draft agent — created lazily the first time an action needs an agent id
  // (Generate Soul), then UPDATED (not re-created) by the final Create Agent.
  const [draft, setDraft] = useState<any | null>(null);
  const draftRef = useRef<Promise<any | null> | null>(null);

  // ---- Home step ------------------------------------------------------------

  async function pickRoot() {
    setRootBusy(true); setRootErr(null);
    try {
      const r = await invoke<{ cancelled: boolean; path?: string; existingRoot?: boolean; nonEmpty?: boolean; agentCount?: number; chatCount?: number }>("onboarding_pick_root");
      if (r.cancelled || !r.path) return;
      setRoot(r.path);
      setPickInfo({
        existingRoot: !!r.existingRoot,
        nonEmpty: !!r.nonEmpty,
        agentCount: r.agentCount ?? 0,
        chatCount: r.chatCount ?? 0,
      });
      setConfirmMix(false);
    } catch (e) { setRootErr(String(e)); }
    finally { setRootBusy(false); }
  }

  async function commitAsNewHome() {
    if (!root) return;
    setRootBusy(true); setRootErr(null);
    try {
      await invoke("onboarding_set_root", { folder: root });
      setStep("agent");
    } catch (e) { setRootErr(String(e)); }
    finally { setRootBusy(false); }
  }

  async function doRestore() {
    if (!root) return;
    setRootBusy(true); setRootErr(null);
    try {
      await invoke("onboarding_set_root", { folder: root });
      onDone();
    } catch (e) { setRootErr(String(e)); }
    finally { setRootBusy(false); }
  }

  // ---- Agent step helpers ---------------------------------------------------

  // Create (or re-create after a rename) the agent's home folder skeleton.
  async function ensureHome(): Promise<string | null> {
    const n = agentName.trim();
    if (!n) return null;
    if (pendingHome && homeNameRef.current === n) return pendingHome;
    try {
      const home = await invoke<string>("onboarding_make_agent_home", { name: n });
      setPendingHome(home);
      homeNameRef.current = n;
      return home;
    } catch (e) { setAgentErr(String(e)); return null; }
  }

  // Idempotent draft creation (guarded so double-clicks can't create two).
  async function ensureDraft(): Promise<any | null> {
    if (draft) return draft;
    if (draftRef.current) return draftRef.current;
    const p = (async () => {
      try {
        const home = await ensureHome();
        if (!home) return null;
        const created = await invoke<any>("agents_create", {
          name: agentName.trim(), icon: agentIcon, color: "#5b8cff",
          folderPath: home, model, provider, contextMode: "isolated", systemPrompt,
        });
        setDraft(created);
        return created;
      } catch (e) { setAgentErr(String(e)); return null; }
      finally { draftRef.current = null; }
    })();
    draftRef.current = p;
    return p;
  }

  // 3b Cloud: load the provider's live model list (best effort — "Auto" always works).
  async function loadModels(prov: string) {
    setModelsLoading(true);
    try {
      const list = prov === "anthropic"
        ? await invoke<string[]>("anthropic_models")
        : await invoke<string[]>("openai_models", { provider: prov });
      setModels(list || []);
    } catch { setModels([]); }
    finally { setModelsLoading(false); }
  }
  useEffect(() => {
    if (step === "agent" && sub === 1 && tab === "cloud" && provider !== "local") void loadModels(provider);
    /* eslint-disable-next-line */
  }, [step, sub, tab, provider]);

  async function saveAndVerifyKey() {
    setKeyBusy(true); setKeyMsg(null);
    try {
      if (cloudKey.trim()) await invoke("set_provider_key", { provider, key: cloudKey.trim() });
      await invoke("provider_verify_key", { provider });
      setKeyMsg("✓ key verified — auth works");
      setCloudKey("");
      void loadModels(provider);
    } catch (e) { setKeyMsg("✗ " + String(e)); }
    finally { setKeyBusy(false); }
  }

  // 3b Local: debounced HF search; exact "author/repo" goes through local_lookup.
  useEffect(() => {
    if (step !== "agent" || sub !== 1 || tab !== "local") return;
    const q = query.trim();
    if (q.length < 2) { setResults([]); setSearching(false); return; }
    if (selected && q === selected.repo) { setResults([]); return; }
    setSearching(true);
    const t = setTimeout(async () => {
      try {
        if (/^[^/\s]+\/[^/\s]+$/.test(q)) {
          const raw = await invoke<any>("local_lookup", { repoId: q });
          const m: CatModel | null = raw?.repo ? raw : raw?.model?.repo ? raw.model : null;
          if (m) { setResults([m]); setLocalErr(null); setSearching(false); return; }
        }
        const res = await invoke<{ models: CatModel[] }>("local_search", { query: q, limit: 6 });
        setResults(res.models || []);
        setLocalErr(null);
      } catch (e) { setLocalErr(String(e)); setResults([]); }
      finally { setSearching(false); }
    }, 350);
    return () => clearTimeout(t);
    /* eslint-disable-next-line */
  }, [query, tab, sub, step]);

  async function refreshDownloaded() {
    try { setDownloaded((await invoke<Downloaded[]>("local_downloaded")) || []); } catch { /* ignore */ }
  }
  useEffect(() => {
    if (step === "agent" && sub === 1 && tab === "local") void refreshDownloaded();
  }, [step, sub, tab]);

  async function download(q: Quant) {
    const channel = `dl-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    setLocalErr(null);
    setProgress((p) => ({ ...p, [q.filename]: 0 }));
    const un = await listen<any>(channel, (e) => {
      const { got, total, done } = e.payload || {};
      setProgress((p) => ({ ...p, [q.filename]: done ? 1 : (total ? got / total : 0) }));
    });
    try {
      await invoke("local_download", { channel, url: q.download_url, filename: q.filename });
      await refreshDownloaded();
    } catch (e) { setLocalErr(`Download failed: ${String(e)}`); }
    finally { un(); setProgress((p) => { const n = { ...p }; delete n[q.filename]; return n; }); }
  }

  function useLocal(path: string) {
    setProvider("local");
    setModel(path);
  }

  // 3c Soul
  async function generateSoul() {
    setSoulBusy(true); setSoulErr(null);
    try {
      const d = await ensureDraft();
      if (!d) { setSoulErr("Couldn't create the agent draft — check the name."); return; }
      const soul = await invoke<string>("agent_generate_soul", { agentId: d.id, brief: soulBrief });
      setSystemPrompt(soul);
    } catch (e) { setSoulErr(String(e)); }
    finally { setSoulBusy(false); }
  }

  // 3d Allow Shell Access — keyed on the agent's home folder (matches the jail).
  useEffect(() => {
    if (step !== "agent" || sub !== 3 || !pendingHome) return;
    invoke<boolean>("allow_shell_access_get", { folder: pendingHome }).then(setAllowShellAccess).catch(() => setAllowShellAccess(false));
  }, [step, sub, pendingHome]);

  async function toggleAllowShellAccess(next: boolean) {
    if (!pendingHome) return;
    if (next) {
      const ok = window.confirm(
        "Enable Allow Shell Access for this agent?\n\n" +
        "This agent will be able to RUN PROGRAMS on your Mac — including build tools, " +
        "git, and anything on your PATH — rooted in its folder. Commands run from the " +
        "folder and cannot leave it, secrets are never shared with them, and you can kill " +
        "any process at any time. But this relaxes the zero-shell guarantee.\n\n" +
        "Only enable this for an agent you're using to build or run code."
      );
      if (!ok) return;
    }
    setProBusy(true);
    try { setAllowShellAccess(await invoke<boolean>("allow_shell_access_set", { folder: pendingHome, enabled: next })); }
    catch { /* ignore */ }
    finally { setProBusy(false); }
  }

  // Finish: create (or update the draft) with everything gathered, then done.
  async function createAgent() {
    const n = agentName.trim();
    if (!n) { setAgentErr("Enter an agent name."); return; }
    setAgentBusy(true); setAgentErr(null);
    try {
      if (draft) {
        const updated = {
          ...draft, name: n, icon: agentIcon,
          folder_path: pendingHome ?? draft.folder_path,
          model, provider, system_prompt: systemPrompt,
        };
        await invoke("agents_update", { profile: updated });
      } else {
        const home = await ensureHome();
        if (!home) { setAgentErr("Couldn't create the agent's folder."); return; }
        await invoke("agents_create", {
          name: n, icon: agentIcon, color: "#5b8cff", folderPath: home,
          model, provider, contextMode: "isolated", systemPrompt,
        });
      }
      await invoke("onboarding_finish");
      setStep("done");
    } catch (e) { setAgentErr(String(e)); }
    finally { setAgentBusy(false); }
  }

  async function nextFromIdentity() {
    setAgentBusy(true); setAgentErr(null);
    try {
      const home = await ensureHome();
      if (home) setSub(1);
    } finally { setAgentBusy(false); }
  }

  const isExisting = !!pickInfo?.existingRoot;
  const isDirty = !!pickInfo?.nonEmpty && !pickInfo?.existingRoot;
  const isDownloaded = (fname: string) => downloaded.some((d) => d.filename === fname);

  return (
    <div style={{ minHeight: "100vh", display: "flex", alignItems: "center", justifyContent: "center", padding: 32, background: "var(--bg)" }}>
      <div style={{ width: "100%", maxWidth: 620, display: "flex", flexDirection: "column", gap: 20 }}>
        <div style={{ textAlign: "center" }}>
          <div style={{ fontSize: 28, fontWeight: 800, letterSpacing: -0.5 }}>Welcome to AYGENT</div>
          <div style={{ color: "var(--text-muted)", marginTop: 6, fontSize: 14 }}>Own your agent — one folder = your whole setup.</div>
          <div style={{ display: "flex", gap: 8, justifyContent: "center", marginTop: 16 }}>
            {(["welcome", "home", "agent"] as Step[]).map((s) => (
              <span key={s} style={{ width: 8, height: 8, borderRadius: 4, background: step === s || (step === "done" && s === "agent") ? "var(--accent)" : "var(--line)" }} />
            ))}
          </div>
        </div>

        {step === "welcome" && (
          <Card title="Own your agent — one folder = your whole setup">
            <p style={hint}>Your AYGENT home holds every agent, its memory, save points, and settings. Back it up, move it, point AYGENT at it on any machine to restore. Only a one-line pointer lives outside it.</p>
            <div style={{ display: "flex", gap: 8, marginTop: 8, flexWrap: "wrap" }}>
              <Button onClick={() => { setIntent("create"); setStep("home"); }}>Create new home</Button>
              <Button variant="secondary" onClick={() => { setIntent("restore"); setStep("home"); }}>Restore existing</Button>
            </div>
            <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Both start by picking your AYGENT folder — we detect whether it is new or an existing home.</span>
          </Card>
        )}

        {step === "home" && (
          <Card title={intent === "restore" ? "Restore your AYGENT home" : "Choose your AYGENT folder"}>
            <p style={hint}>
              {intent === "restore"
                ? "Pick the folder that already holds your AYGENT home (the one with aygent-root.json). We will restore it in place."
                : "Pick where your new AYGENT home should live. An empty folder is ideal — we will create your home there."}
            </p>

            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <Input value={root ?? ""} readOnly mono placeholder="no folder chosen yet" />
              <Button variant="secondary" onClick={pickRoot} disabled={rootBusy}>{rootBusy ? "…" : "Pick…"}</Button>
            </div>

            {root && pickInfo && (
              <div style={{ marginTop: 8, display: "flex", flexDirection: "column", gap: 8 }}>
                <span style={{ fontSize: 12, color: "var(--text-faint)", fontFamily: "ui-monospace, monospace", wordBreak: "break-all" }}>{root}</span>

                {isExisting && (
                  <div style={{ padding: "10px 12px", borderRadius: "var(--radius-control)", background: "color-mix(in srgb, var(--ok, #16a34a) 12%, var(--surface))", border: "1px solid color-mix(in srgb, var(--ok, #16a34a) 28%, transparent)", fontSize: 13 }}>
                    <span style={{ fontWeight: 700, color: "var(--ok, #16a34a)" }}>✓ Found an AYGENT home</span>
                    <span style={{ color: "var(--text-muted)" }}> — Restore — found {pickInfo.agentCount} agents, {pickInfo.chatCount} chats.</span>
                  </div>
                )}

                {isDirty && (
                  <div style={{ padding: "10px 12px", borderRadius: "var(--radius-control)", background: "color-mix(in srgb, #eab308 14%, var(--surface))", border: "1px solid color-mix(in srgb, #eab308 36%, transparent)", fontSize: 13 }}>
                    <span style={{ fontWeight: 700, color: "#a16207" }}>⚠ Folder is not empty and is not an AYGENT home</span>
                    <div style={{ color: "var(--text-muted)", marginTop: 2 }}>Use anyway? (may mix files) — your existing files will stay, but AYGENT will create its own files alongside them.</div>
                    <label style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 8, cursor: "pointer", fontSize: 13 }}>
                      <input type="checkbox" checked={confirmMix} onChange={(e) => setConfirmMix(e.target.checked)} />
                      I understand — use this folder anyway
                    </label>
                  </div>
                )}

                {!isExisting && !isDirty && (
                  <span style={{ fontSize: 12, color: "var(--text-faint)" }}>Empty folder — ready to create your home here.</span>
                )}
              </div>
            )}

            {rootErr && <span style={errStyle}>{rootErr}</span>}

            <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
              <Button variant="secondary" onClick={() => setStep("welcome")}>Back</Button>
              {!root && <span style={{ ...hint, fontSize: 12, alignSelf: "center" }}>Pick a folder to continue.</span>}
              {root && isExisting && <Button onClick={doRestore} disabled={rootBusy}>{rootBusy ? "Restoring…" : `Restore — ${pickInfo?.agentCount ?? 0} agents, ${pickInfo?.chatCount ?? 0} chats`}</Button>}
              {root && isDirty && <Button onClick={commitAsNewHome} disabled={rootBusy || !confirmMix}>{rootBusy ? "Setting up…" : "Use anyway"}</Button>}
              {root && !isExisting && !isDirty && <Button onClick={commitAsNewHome} disabled={rootBusy}>{rootBusy ? "Setting up…" : "Create"}</Button>}
            </div>
          </Card>
        )}

        {step === "agent" && (
          <Card title={`Agent Setup — ${SUB_LABELS[sub]}`}>
            {/* sub-step progress */}
            <div style={{ display: "flex", gap: 6, alignItems: "center", marginBottom: 4 }}>
              {SUB_LABELS.map((l, i) => (
                <span key={l} style={{
                  fontSize: 11, fontWeight: i === sub ? 700 : 500, padding: "3px 8px", borderRadius: 999,
                  color: i === sub ? "var(--accent)" : i < sub ? "var(--text-muted)" : "var(--text-faint)",
                  background: i === sub ? "color-mix(in srgb, var(--accent) 10%, transparent)" : "transparent",
                  border: i === sub ? "1px solid color-mix(in srgb, var(--accent) 30%, transparent)" : "1px solid transparent",
                }}>{i < sub ? "✓ " : ""}{l}</span>
              ))}
            </div>

            {/* 3a Identity */}
            {sub === 0 && (
              <>
                <p style={hint}>Your agent gets its own folder under your home for its soul, memory, and files.</p>
                <label style={fieldLabel}>Agent name
                  <Input value={agentName} onChange={(e) => setAgentName(e.target.value)} placeholder="e.g. Atlas, Journal, Research" />
                </label>
                <label style={fieldLabel}>Icon
                  <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginTop: 4 }}>
                    {AGENT_ICONS.map((ic) => {
                      const sel = agentIcon === ic;
                      return (
                        <button key={ic} onClick={() => setAgentIcon(ic as IconName)} title={ic} style={{
                          width: 40, height: 40, borderRadius: "var(--radius-control)", cursor: "pointer",
                          display: "flex", alignItems: "center", justifyContent: "center",
                          color: sel ? "var(--accent)" : "var(--text-muted)",
                          border: sel ? "var(--border-width) solid var(--accent)" : "var(--border-width) solid var(--line)",
                          background: sel ? "color-mix(in srgb, var(--accent) 10%, var(--bg))" : "var(--bg)",
                        }}><Icon name={ic as IconName} size={20} /></button>
                      );
                    })}
                  </div>
                </label>
                {root && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Home: <span style={{ fontFamily: "ui-monospace, monospace" }}>{root}/{agentName.trim() || "…"}/</span></span>}
              </>
            )}

            {/* 3b Model — unified Cloud vs Local */}
            {sub === 1 && (
              <>
                <p style={hint}>Pick the model that powers this agent. Cloud needs an API key; Local runs entirely on this Mac.</p>
                <div style={{ display: "flex", gap: 0, border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)", overflow: "hidden", width: "fit-content" }}>
                  {(["cloud", "local"] as const).map((t) => (
                    <button key={t} onClick={() => setTab(t)} style={{
                      padding: "8px 20px", cursor: "pointer", fontSize: 13, fontWeight: 700, border: "none",
                      background: tab === t ? "var(--accent)" : "var(--bg)",
                      color: tab === t ? "#fff" : "var(--text-muted)",
                    }}>{t === "cloud" ? "☁️ Cloud" : "💻 Local"}</button>
                  ))}
                </div>

                {tab === "cloud" && (
                  <>
                    <div style={{ display: "flex", gap: 12 }}>
                      <label style={fieldLabel}>Provider
                        <select value={provider === "local" ? "anthropic" : provider} onChange={(e) => { setProvider(e.target.value); setModel(""); setKeyMsg(null); }} style={selectStyle}>
                          <option value="anthropic">Anthropic</option>
                          <option value="openai">OpenAI</option>
                          <option value="openrouter">OpenRouter</option>
                          <option value="meta">Muse (Meta)</option>
                        </select>
                      </label>
                      <label style={fieldLabel}>Model
                        {provider === "openrouter" ? (
                          <>
                            <Input value={model} onChange={(e) => setModel(e.target.value)} mono list="ob-openrouter-models"
                              placeholder="paste a model id, e.g. anthropic/claude-3.5-sonnet" />
                            <datalist id="ob-openrouter-models">
                              {models.map((m) => <option key={m} value={m} />)}
                            </datalist>
                          </>
                        ) : (
                          <select value={model} onChange={(e) => setModel(e.target.value)} style={selectStyle}>
                            <option value="">{modelsLoading ? "loading models…" : "Auto (recommended)"}</option>
                            {models.map((m) => <option key={m} value={m}>{m}</option>)}
                            {model && !models.includes(model) && <option value={model}>{model}</option>}
                          </select>
                        )}
                      </label>
                    </div>
                    <label style={fieldLabel}>API key
                      <div style={{ display: "flex", gap: 8 }}>
                        <Input type="password" mono value={cloudKey} onChange={(e) => setCloudKey(e.target.value)} placeholder="paste key (or leave empty if already saved)" />
                        <Button variant="secondary" onClick={saveAndVerifyKey} disabled={keyBusy}>{keyBusy ? "Verifying…" : "Verify"}</Button>
                      </div>
                      <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Stored in your macOS Keychain — never in files. Verify round-trips real auth.</span>
                    </label>
                    {keyMsg && <span style={{ fontSize: 12, color: keyMsg.startsWith("✓") ? "var(--ok, #16a34a)" : "var(--danger, #ef4444)" }}>{keyMsg}</span>}
                  </>
                )}

                {tab === "local" && (
                  <>
                    <label style={fieldLabel}>Search Hugging Face
                      <Input value={query} onChange={(e) => { setQuery(e.target.value); setSelected(null); }} mono
                        placeholder="search models (e.g. qwen 7b) or paste a repo id (author/repo)" />
                      {searching && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>searching…</span>}
                    </label>

                    {results.length > 0 && !selected && (
                      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                        {results.map((m) => (
                          <button key={m.repo} onClick={() => { setSelected(m); setQuery(m.repo); setResults([]); }} style={{
                            display: "flex", alignItems: "baseline", gap: 8, textAlign: "left", cursor: "pointer",
                            padding: "8px 10px", borderRadius: "var(--radius-control)",
                            border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)", font: "inherit",
                          }}>
                            <span style={{ fontWeight: 700, fontSize: 14 }}>{m.name}</span>
                            <span style={{ fontSize: 12, color: "var(--text-faint)" }}>{m.family_label} · {m.params_billions}B</span>
                            <span style={{ fontFamily: "ui-monospace, monospace", fontSize: 11, color: "var(--text-faint)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1 }}>{m.repo}</span>
                          </button>
                        ))}
                      </div>
                    )}

                    {selected && (
                      <div style={{ display: "flex", flexDirection: "column", gap: 8, background: "var(--surface)", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)", padding: "12px 14px" }}>
                        <div style={{ display: "flex", alignItems: "baseline", gap: 8, flexWrap: "wrap" }}>
                          <span style={{ fontWeight: 800, fontSize: 15 }}>{selected.name}</span>
                          <span style={{ fontSize: 12, color: "var(--text-faint)" }}>{selected.family_label} · {selected.params_billions}B</span>
                        </div>
                        {selected.quants.map((q) => {
                          const pct = progress[q.filename];
                          const have = isDownloaded(q.filename);
                          const path = downloaded.find((d) => d.filename === q.filename)?.path;
                          return (
                            <div key={q.filename} style={{ display: "flex", alignItems: "center", gap: 10, fontSize: 13 }}>
                              <span title={q.perf.note}>{q.perf.badge}</span>
                              <span style={{ fontWeight: 600 }}>{q.quant}</span>
                              <span style={{ color: "var(--text-faint)", fontSize: 12 }}>{q.size_gb.toFixed(1)} GB</span>
                              <span style={{ flex: 1 }} />
                              {pct !== undefined ? (
                                <span style={{ fontSize: 12, color: "var(--accent)" }}>{Math.round(pct * 100)}%…</span>
                              ) : have && path ? (
                                model === path ? <span style={{ fontSize: 12, color: "var(--ok, #16a34a)", fontWeight: 700 }}>✓ selected</span>
                                  : <Button variant="secondary" onClick={() => useLocal(path)}>Use</Button>
                              ) : (
                                <Button variant="secondary" onClick={() => download(q)}>Download</Button>
                              )}
                            </div>
                          );
                        })}
                      </div>
                    )}

                    {downloaded.length > 0 && (
                      <label style={fieldLabel}>Already on this Mac
                        <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                          {downloaded.map((d) => (
                            <div key={d.path} style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13 }}>
                              <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontFamily: "ui-monospace, monospace", fontSize: 12 }}>{d.filename}</span>
                              <span style={{ color: "var(--text-faint)", fontSize: 12 }}>{d.size_gb.toFixed(1)} GB</span>
                              {model === d.path
                                ? <span style={{ fontSize: 12, color: "var(--ok, #16a34a)", fontWeight: 700 }}>✓ selected</span>
                                : <Button variant="secondary" onClick={() => useLocal(d.path)}>Use</Button>}
                            </div>
                          ))}
                        </div>
                      </label>
                    )}

                    {provider === "local" && model && (
                      <span style={{ fontSize: 12, color: "var(--ok, #16a34a)" }}>✓ Local model selected — this agent runs fully on-device.</span>
                    )}
                    {localErr && <span style={errStyle}>{localErr}</span>}
                  </>
                )}
              </>
            )}

            {/* 3c Soul */}
            {sub === 2 && (
              <>
                <p style={hint}>Give your agent a personality — or let it write its own Soul from a one-line vibe. You can edit everything before finishing.</p>
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <Input value={soulBrief} onChange={(e) => setSoulBrief(e.target.value)}
                    placeholder="optional vibe: 'warm, rigorous research partner'…" />
                  <Button variant="secondary" onClick={generateSoul} disabled={soulBusy}>{soulBusy ? "Writing…" : "✨ Generate Soul"}</Button>
                </div>
                {soulErr && <span style={errStyle}>{soulErr}</span>}
                <label style={fieldLabel}>Custom instructions / Soul
                  <textarea value={systemPrompt} onChange={(e) => setSystemPrompt(e.target.value)}
                    rows={8} placeholder="This agent's personality, values & instructions… or generate a Soul above. Leave empty to skip."
                    style={{
                      background: "var(--bg)", border: "var(--border-width) solid var(--line)",
                      borderRadius: "var(--radius-control)", color: "var(--text)", padding: "9px 12px",
                      fontSize: 14, resize: "vertical", fontFamily: "inherit",
                    }} />
                </label>
              </>
            )}

            {/* 3d Superpowers */}
            {sub === 3 && (
              <>
                <p style={hint}>One last choice — how much power this agent gets. You can change this any time in the agent's settings.</p>
                <div style={{
                  display: "flex", flexDirection: "column", gap: 8,
                  background: allowShellAccess ? "color-mix(in srgb, var(--danger, #ef4444) 8%, var(--surface))" : "var(--surface)",
                  border: `var(--border-width) solid ${allowShellAccess ? "color-mix(in srgb, var(--danger, #ef4444) 40%, transparent)" : "var(--line)"}`,
                  borderRadius: "var(--radius-control)", padding: "12px 14px",
                }}>
                  <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
                    <div style={{ fontSize: 13, fontWeight: 600 }}>⚡ Allow Shell Access — run shell commands</div>
                    <label style={{ display: "inline-flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
                      <input type="checkbox" checked={allowShellAccess} disabled={proBusy || !pendingHome}
                        onChange={(e) => toggleAllowShellAccess(e.target.checked)} />
                      <span style={{ fontSize: 13 }}>{allowShellAccess ? "Enabled" : "Off"}</span>
                    </label>
                  </div>
                  <span style={{ ...hint, fontSize: 12 }}>
                    Lets this agent run programs on your Mac (build tools, git, anything on your PATH),
                    rooted in its folder. Commands can't leave the folder and your API keys are never
                    shared with them — but this relaxes the zero-shell guarantee. Default off; most
                    agents never need it. Turn it on only for an agent that builds or runs code.
                  </span>
                </div>
              </>
            )}

            {agentErr && <span style={errStyle}>{agentErr}</span>}

            <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
              <Button variant="secondary" onClick={() => (sub === 0 ? setStep("home") : setSub(sub - 1))}>Back</Button>
              {sub === 0 && <Button onClick={nextFromIdentity} disabled={agentBusy || !agentName.trim()}>{agentBusy ? "…" : "Next"}</Button>}
              {(sub === 1 || sub === 2) && <Button onClick={() => setSub(sub + 1)}>Next</Button>}
              {sub === 3 && <Button onClick={createAgent} disabled={agentBusy}>{agentBusy ? "Creating…" : "Create Agent"}</Button>}
              {sub === 1 && !model && <span style={{ ...hint, fontSize: 12, alignSelf: "center", color: "var(--text-faint)" }}>No model picked = Auto (cloud default).</span>}
            </div>
          </Card>
        )}

        {step === "done" && (
          <Card title="You are all set">
            <p style={hint}>Your AYGENT home and first agent are ready. Your folder is yours — back it up, move it, restore it on any machine.</p>
            <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
              <Button onClick={onDone}>Enter AYGENT</Button>
            </div>
          </Card>
        )}
      </div>
    </div>
  );
}

const hint = { color: "var(--text-muted)", fontSize: 14, margin: "0 0 4px" } as const;
const fieldLabel = { display: "flex", flexDirection: "column", gap: 6, fontSize: 13, fontWeight: 600, flex: 1, marginTop: 8 } as const;
const errStyle = { fontSize: 12, color: "var(--danger, #ef4444)" } as const;
const selectStyle = {
  padding: "9px 12px", borderRadius: "var(--radius-control)",
  border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)",
  width: "100%", maxWidth: "100%", boxSizing: "border-box",
  textOverflow: "ellipsis", overflow: "hidden", whiteSpace: "nowrap",
} as const;
