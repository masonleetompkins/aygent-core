import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input } from "../components/ui";
import { AgentForm } from "./Agents";

// ONBOARDING (config relocation, 2026-07-31). First-launch wizard:
//   1) Provider key (or local model)   2) Pick the AYGENT root folder
//   3) Configure the first agent (name → home subfolder + model + soul)
// "Own your agent": all config lives in the root folder the user picks; only a
// one-line pointer sits in app-support, keys stay in macOS Keychain.
//
// If the picked folder is ALREADY an AYGENT root (has the manifest), we RESTORE
// it (adopt existing config) instead of running steps — the new-machine port
// path. onDone() → App re-checks onboarding_status and boots into the root.

type Step = "key" | "folder" | "agent";

export function Onboarding({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<Step>("key");

  // Step 1 — provider key
  const [provider, setProvider] = useState("anthropic");
  const [apiKey, setApiKey] = useState("");
  const [keyBusy, setKeyBusy] = useState(false);
  const [keyErr, setKeyErr] = useState<string | null>(null);

  // Step 2 — root folder
  const [root, setRoot] = useState<string | null>(null);
  const [rootBusy, setRootBusy] = useState(false);
  const [rootErr, setRootErr] = useState<string | null>(null);

  // --- Step 1: save the provider key to the keychain -----------------------
  async function saveKey() {
    if (!apiKey.trim()) { setStep("folder"); return; } // allow skip (e.g. local later)
    setKeyBusy(true); setKeyErr(null);
    try {
      await invoke("set_provider_key", { provider, key: apiKey.trim() });
      setStep("folder");
    } catch (e) { setKeyErr(String(e)); }
    finally { setKeyBusy(false); }
  }

  // --- Step 2: pick + commit the root folder -------------------------------
  async function pickRoot() {
    setRootBusy(true); setRootErr(null);
    try {
      const r = await invoke<{ cancelled: boolean; path?: string; existingRoot?: boolean; nonEmpty?: boolean }>(
        "onboarding_pick_root"
      );
      if (r.cancelled || !r.path) { setRootBusy(false); return; }
      // If it's already an AYGENT root, restore immediately + finish.
      if (r.existingRoot) {
        await invoke("onboarding_set_root", { folder: r.path });
        onDone(); // App re-checks status → boots into the restored root
        return;
      }
      setRoot(r.path);
    } catch (e) { setRootErr(String(e)); }
    finally { setRootBusy(false); }
  }

  async function commitRoot() {
    if (!root) return;
    setRootBusy(true); setRootErr(null);
    try {
      await invoke("onboarding_set_root", { folder: root });
      setStep("agent");
    } catch (e) { setRootErr(String(e)); }
    finally { setRootBusy(false); }
  }

  return (
    <div style={{ minHeight: "100vh", display: "flex", alignItems: "center", justifyContent: "center", padding: 32, background: "var(--bg)" }}>
      <div style={{ width: "100%", maxWidth: 560, display: "flex", flexDirection: "column", gap: 20 }}>
        {/* header + step dots */}
        <div style={{ textAlign: "center" }}>
          <div style={{ fontSize: 28, fontWeight: 800, letterSpacing: -0.5 }}>Welcome to AYGENT</div>
          <div style={{ color: "var(--text-muted)", marginTop: 6, fontSize: 14 }}>
            The AI agent you actually own. Let's set up your home.
          </div>
          <div style={{ display: "flex", gap: 8, justifyContent: "center", marginTop: 16 }}>
            {(["key", "folder", "agent"] as Step[]).map((s) => (
              <span key={s} style={{
                width: 8, height: 8, borderRadius: 4,
                background: step === s ? "var(--accent)" : "var(--line)",
              }} />
            ))}
          </div>
        </div>

        {step === "key" && (
          <Card title="1 · Connect a model">
            <p style={hint}>Add a provider API key — it's stored securely in your macOS Keychain, never in a file. You can add more later (or use a local model).</p>
            <label style={fieldLabel}>Provider
              <select value={provider} onChange={(e) => setProvider(e.target.value)} style={selectStyle}>
                <option value="anthropic">Anthropic</option>
                <option value="openai">OpenAI</option>
                <option value="openrouter">OpenRouter</option>
              </select>
            </label>
            <label style={fieldLabel}>API key
              <Input value={apiKey} onChange={(e) => setApiKey(e.target.value)} mono
                placeholder="sk-… (stored in Keychain)" type="password" />
            </label>
            {keyErr && <span style={errStyle}>{keyErr}</span>}
            <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
              <Button onClick={saveKey} disabled={keyBusy}>{keyBusy ? "Saving…" : (apiKey.trim() ? "Save & continue" : "Skip for now")}</Button>
            </div>
          </Card>
        )}

        {step === "folder" && (
          <Card title="2 · Choose your AYGENT folder">
            <p style={hint}>
              This folder is your AYGENT <b>home</b> — every agent, its memory, save points, and settings live here.
              It's yours: back it up, move it, inspect it. Point AYGENT at it on any machine to restore your whole setup.
            </p>
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <Input value={root ?? ""} readOnly mono placeholder="no folder chosen yet" />
              <Button variant="secondary" onClick={pickRoot} disabled={rootBusy}>{rootBusy ? "…" : "Pick…"}</Button>
            </div>
            {root && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>New home will be created here. (Pointing at an existing AYGENT folder restores it automatically.)</span>}
            {rootErr && <span style={errStyle}>{rootErr}</span>}
            <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
              <Button variant="secondary" onClick={() => setStep("key")}>Back</Button>
              <Button onClick={commitRoot} disabled={!root || rootBusy}>{rootBusy ? "Setting up…" : "Use this folder"}</Button>
            </div>
          </Card>
        )}

        {step === "agent" && (
          <Card title="3 · Create your first agent">
            <p style={hint}>
              Your agent gets its own folder under your home — its soul, memory, and files live there.
              This is the same setup you'll use in-app: generate a soul, attach context files, pick a model.
            </p>
            {/* Reuse the EXACT in-app agent form (1:1 parity). Its folder defaults
                to the agent's home under the root via onboarding_make_agent_home;
                on save it creates the profile + finishes onboarding (restart). */}
            <AgentForm
              initial={null}
              pendingFolder={null}
              onPickFolder={() => { /* onboarding derives the home folder from the name */ }}
              onDone={() => onDone()}
              onCancel={() => setStep("folder")}
            />
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
  width: "100%", boxSizing: "border-box",
} as const;

