// AYGENT HyperFrames — the SINGLE card that owns the in-app video/graphics skill:
// install (provision a portable Node + FFmpeg + hyperframes into AYGENT's own
// space, nothing touches the system), turn it ON/OFF for this agent, and remove.
// It is deliberately NOT shown in the generic Skills list (filtered out there) so
// there's exactly one place to manage it. (Mason 08-06 — merged the duplicate.)
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card, Button, Pill } from "./ui";

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;
const SKILL_ID = "skill.hyperframes";

export function AygentHyperFrames({ agentId, folder }: { agentId: string | null; folder: string | null }) {
  type Status = { installed: boolean; runtime_dir: string | null };
  type Skill = { id: string; enabled: boolean };
  const [status, setStatus] = useState<Status | null>(null);
  const [skillOn, setSkillOn] = useState<boolean>(false);
  const [phase, setPhase] = useState<string | null>(null);
  const [note, setNote] = useState<string>("");
  const [pct, setPct] = useState<number | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [toggling, setToggling] = useState(false);
  const [proMode, setProMode] = useState(false);
  const unlistenRef = useRef<null | (() => void)>(null);

  async function refresh() {
    try { setStatus(await invoke<Status>("hyperframes_status")); } catch (e) { setErr(String(e)); }
    // Per-agent ON/OFF state for the skill (the card owns this toggle now).
    try {
      const skills = await invoke<Skill[]>("skills_list", { agentId, folder });
      setSkillOn(!!skills.find((k) => k.id === SKILL_ID)?.enabled);
    } catch { /* leave as-is */ }
  }
  useEffect(() => {
    refresh();
    invoke<boolean>("pro_mode_get", { folder }).then(setProMode).catch(() => setProMode(false));
    return () => { unlistenRef.current?.(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [folder, agentId]);

  async function enable() {
    setErr(null); setPhase("start"); setPct(0); setNote("Starting…");
    const channel = `hyperframes-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const un = await listen<{ phase: string; note: string; pct: number | null }>(channel, (e) => {
      const p = e.payload || ({} as any);
      if (p.phase) setPhase(p.phase);
      if (typeof p.note === "string") setNote(p.note);
      if (typeof p.pct === "number") setPct(p.pct);
    });
    unlistenRef.current = un;
    try {
      await invoke("hyperframes_provision", { channel, agentId, folder });
      await refresh();
      setPhase("done"); setNote("HyperFrames is ready."); setPct(1);
      window.dispatchEvent(new Event("aygent-skills-changed"));
    } catch (e) {
      setErr(String(e)); setPhase(null);
    } finally {
      un(); unlistenRef.current = null;
    }
  }

  async function toggleSkill() {
    if (!agentId) { setErr("Select an agent first — skills enable per agent."); return; }
    setToggling(true); setErr(null);
    try {
      await invoke("tools_set_enabled", { agentId, folder, id: SKILL_ID, on: !skillOn });
      setSkillOn((v) => !v);
      window.dispatchEvent(new Event("aygent-skills-changed"));
    } catch (e) { setErr(String(e)); }
    finally { setToggling(false); }
  }

  async function disable() {
    setErr(null); setRemoving(true);
    try {
      await invoke("hyperframes_remove", { keepToolchain: false });
      setConfirmRemove(false); setPhase(null); setPct(null); setNote(""); setSkillOn(false);
      await refresh();
      window.dispatchEvent(new Event("aygent-skills-changed"));
    } catch (e) { setErr(String(e)); }
    finally { setRemoving(false); }
  }

  const busy = phase !== null && phase !== "done";

  return (
    <Card title="HyperFrames — video & motion graphics">
      <p style={hint}>
        Let your agents create videos, animations, and motion graphics — from HTML, rendered to MP4.
        Click Enable and AYGENT installs everything it needs into its own space (a portable Node,
        FFmpeg, and the HyperFrames engine). Nothing touches your system, and you can remove it anytime.
      </p>

      {status?.installed && !busy ? (
        <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          {/* Row 1: the single meaningful control — is this skill ON for the agent. */}
          <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
            <Button variant={skillOn ? "primary" : "secondary"} onClick={() => void toggleSkill()} disabled={toggling}>
              {toggling ? "…" : skillOn ? "On" : "Off"}
            </Button>
            <span style={{ fontSize: 13, color: "var(--text-muted)" }}>
              {skillOn
                ? "This agent can create videos & motion graphics."
                : "Installed but off for this agent — turn On to give it the skill."}
            </span>
            <span style={{ flex: 1 }} />
            <Pill tone="ok">installed ✓</Pill>
          </div>

          {/* Row 2: Pro Mode dependency — only shown when it would actually block. */}
          {skillOn && !proMode && (
            <div style={{
              fontSize: 12.5, color: "var(--warn, #b7791f)",
              border: "var(--border-width) solid var(--warn, #b7791f)",
              background: "var(--warn-bg, rgba(234,179,8,.10))",
              borderRadius: 8, padding: "8px 12px",
            }}>
              ⚠️ Renders run shell commands, so this agent also needs <b>Pro Mode</b> ON — turn it on for
              this agent in the <b>Agents</b> tab. Until then it has the skill but can’t run the render.
            </div>
          )}

          {/* Row 3: install location + remove (secondary, tucked at the bottom). */}
          <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap", borderTop: "var(--border-width) solid var(--line)", paddingTop: 10 }}>
            {status.runtime_dir && (
              <span style={{ ...hint, fontSize: 11.5, color: "var(--text-faint)", flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                Installed in <code style={{ fontFamily: "ui-monospace, monospace" }}>{status.runtime_dir}</code>
              </span>
            )}
            {!confirmRemove ? (
              <Button variant="secondary" onClick={() => setConfirmRemove(true)} disabled={removing}>Disable &amp; remove</Button>
            ) : (
              <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{ fontSize: 12.5, color: "var(--text-muted)" }}>Remove it + free the space?</span>
                <Button variant="secondary" onClick={() => void disable()} disabled={removing}>{removing ? "Removing…" : "Yes, remove"}</Button>
                <Button variant="secondary" onClick={() => setConfirmRemove(false)} disabled={removing}>Cancel</Button>
              </span>
            )}
          </div>
        </div>
      ) : busy ? (
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span style={{ display: "inline-block", animation: "aygentSpin 1s linear infinite", fontSize: 14 }}>◐</span>
            <span style={{ fontSize: 13.5 }}>{note}</span>
            {pct !== null && (
              <span style={{ fontSize: 12, fontFamily: "ui-monospace, monospace", marginLeft: "auto" }}>{Math.round(pct * 100)}%</span>
            )}
          </div>
          <div style={{ height: 6, borderRadius: 3, background: "var(--surface)", border: "var(--border-width) solid var(--line)", overflow: "hidden" }}>
            <div style={{ height: "100%", width: `${Math.round((pct ?? 0) * 100)}%`, background: "var(--accent)", transition: "width .3s ease" }} />
          </div>
          <p style={{ ...hint, fontSize: 11.5, color: "var(--text-faint)" }}>
            Installing a portable Node + FFmpeg + HyperFrames into AYGENT’s own space. First time only.
          </p>
          <style>{`@keyframes aygentSpin { to { transform: rotate(360deg); } }`}</style>
        </div>
      ) : (
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <Button onClick={() => void enable()}>Enable HyperFrames</Button>
          <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>~120MB one-time download · no system changes</span>
        </div>
      )}
      {err && <Pill tone="danger">✗ {err}</Pill>}
    </Card>
  );
}
