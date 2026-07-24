// Settings — THE WEDGE. Where "no config files, ever" becomes visible and
// delightful. Appearance (theme lives here now, not a debug bar), Providers
// (BYO keys -> Keychain), and the Agent Folder scope. Themed via tokens.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";
import { saveTheme, type Mode } from "../lib/theme";

const ACCENT_SWATCHES = ["", "#2dd4bf", "#6366f1", "#e0533d", "#22c55e", "#eab308", "#ec4899"];
const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

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

// re-export so App can persist through the same helper
export { saveTheme };
