import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input } from "../components/ui";
import { Icon, AGENT_ICONS, type IconName } from "../components/Icon";

// Onboarding v2 (3-step wizard): Welcome -> Home -> Agent Setup -> Done
// Own your agent — one folder = your whole setup. No provider key step.
// Home is picked via single native picker `onboarding_pick_root` which returns
// { path, existingRoot, nonEmpty, agentCount, chatCount }.
// After `onboarding_set_root` the live DB is already re-pointed (no restart).
// Agent home is created via `onboarding_make_agent_home` (<root>/<Name>/).

type Step = "welcome" | "home" | "agent" | "done";
type Intent = "create" | "restore" | null;

export function Onboarding({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<Step>("welcome");
  const [intent, setIntent] = useState<Intent>(null);

  // Home
  const [root, setRoot] = useState<string | null>(null);
  const [pickInfo, setPickInfo] = useState<{ existingRoot: boolean; nonEmpty: boolean; agentCount: number; chatCount: number } | null>(null);
  const [rootBusy, setRootBusy] = useState(false);
  const [rootErr, setRootErr] = useState<string | null>(null);
  const [confirmMix, setConfirmMix] = useState(false);

  // Agent Setup
  const [agentName, setAgentName] = useState("");
  const [agentIcon, setAgentIcon] = useState<IconName>("sparkles");
  const [agentBusy, setAgentBusy] = useState(false);
  const [agentErr, setAgentErr] = useState<string | null>(null);

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

  async function createFirstAgent() {
    const n = agentName.trim();
    if (!n) { setAgentErr("Enter an agent name."); return; }
    setAgentBusy(true); setAgentErr(null);
    try {
      const home = await invoke<string>("onboarding_make_agent_home", { name: n });
      await invoke("agents_create", {
        name: n,
        icon: agentIcon,
        color: "#5b8cff",
        folderPath: home,
        model: "",
        provider: "anthropic",
        contextMode: "isolated",
        systemPrompt: "",
      });
      setStep("done");
    } catch (e) { setAgentErr(String(e)); }
    finally { setAgentBusy(false); }
  }

  const isExisting = !!pickInfo?.existingRoot;
  const isDirty = !!pickInfo?.nonEmpty && !pickInfo?.existingRoot;

  return (
    <div style={{ minHeight: "100vh", display: "flex", alignItems: "center", justifyContent: "center", padding: 32, background: "var(--bg)" }}>
      <div style={{ width: "100%", maxWidth: 560, display: "flex", flexDirection: "column", gap: 20 }}>
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
          <Card title="Create your first agent">
            <p style={hint}>Your agent gets its own folder under your home (&lt;home&gt;/&lt;Name&gt;/) for its soul, memory, and files.</p>

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

            {root && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Home: <span style={{ fontFamily: "ui-monospace, monospace" }}>{root}/{agentName.trim() || "…"} /</span></span>}

            {agentErr && <span style={errStyle}>{agentErr}</span>}

            <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
              <Button variant="secondary" onClick={() => setStep("home")}>Back</Button>
              <Button onClick={createFirstAgent} disabled={agentBusy || !agentName.trim()}>{agentBusy ? "Creating…" : "Create agent"}</Button>
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
