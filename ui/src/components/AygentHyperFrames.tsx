// AYGENT HyperFrames — one-click enable for the in-app video/graphics skill.
// AYGENT provisions a PORTABLE Node + static FFmpeg + the hyperframes package
// into its OWN space (nothing touches the system), then registers a Skill the
// agent uses to render HTML->MP4. The card narrates every step + lists what's
// installed and where; Disable removes it and reclaims the disk. (Mason 08-06.)
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card, Button, Pill } from "./ui";

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

export function AygentHyperFrames() {
  type Status = { installed: boolean; runtime_dir: string | null };
  const [status, setStatus] = useState<Status | null>(null);
  const [phase, setPhase] = useState<string | null>(null);
  const [note, setNote] = useState<string>("");
  const [pct, setPct] = useState<number | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [removing, setRemoving] = useState(false);
  const unlistenRef = useRef<null | (() => void)>(null);

  async function refresh() {
    try { setStatus(await invoke<Status>("hyperframes_status")); } catch (e) { setErr(String(e)); }
  }
  useEffect(() => { refresh(); return () => { unlistenRef.current?.(); }; }, []);

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
      await invoke("hyperframes_provision", { channel });
      await refresh();
      setPhase("done"); setNote("HyperFrames is ready."); setPct(1);
      // Let the Skills list refresh so the new skill shows up immediately.
      window.dispatchEvent(new Event("aygent-skills-changed"));
    } catch (e) {
      setErr(String(e)); setPhase(null);
    } finally {
      un(); unlistenRef.current = null;
    }
  }

  async function disable() {
    setErr(null); setRemoving(true);
    try {
      await invoke("hyperframes_remove", { keepToolchain: false });
      setConfirmRemove(false); setPhase(null); setPct(null); setNote("");
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
        <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
            <Pill tone="ok">enabled ✓</Pill>
            <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>
              Your agents now have the <b>hyperframes</b> skill (see the Skills tab).
            </span>
            <span style={{ flex: 1 }} />
            {!confirmRemove ? (
              <Button variant="secondary" onClick={() => setConfirmRemove(true)} disabled={removing}>Disable &amp; remove</Button>
            ) : (
              <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{ fontSize: 12.5, color: "var(--text-muted)" }}>Remove HyperFrames + its toolchain?</span>
                <Button variant="secondary" onClick={() => void disable()} disabled={removing}>{removing ? "Removing…" : "Yes, remove"}</Button>
                <Button variant="secondary" onClick={() => setConfirmRemove(false)} disabled={removing}>Cancel</Button>
              </span>
            )}
          </div>
          {status.runtime_dir && (
            <p style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>
              Installed in <code style={{ fontFamily: "ui-monospace, monospace" }}>{status.runtime_dir}</code>. Removing frees the space.
            </p>
          )}
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
