// AYGENT — task_continue TIMER PICKER (Mason 09-08).
//
// App-level modal: when any turn calls task_continue, the backend parks the
// turn and emits `task-continue-pick`. This overlay offers 1 / 3 / 5 / 10 /
// 15 minutes; the answer resolves the delay via `task_continue_answer` and
// the wake-up is enqueued exactly as before. No answer in 3 minutes → the
// agent's suggested delay stands (backend timeout), so unattended turns never
// wedge. Queued: picks stack, newest on top.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button } from "./ui";

type Pick = { id: string; agentId: string; agentName: string; note: string; requestedSecs: number; optionsMin: number[] };

const fmtDur = (secs: number) =>
  secs < 60 ? `${secs}s` : secs % 60 === 0 ? `${secs / 60} min` : `${Math.floor(secs / 60)}m ${secs % 60}s`;

export function ContinuePicker() {
  const [picks, setPicks] = useState<Pick[]>([]);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    let un: (() => void) | undefined;
    listen<Pick>("task-continue-pick", (ev) => {
      const p = ev.payload;
      if (!p?.id) return;
      setPicks((xs) => (xs.some((x) => x.id === p.id) ? xs : [...xs, p]));
    }).then((f) => { un = f; }).catch(() => {});
    return () => { un?.(); };
  }, []);

  async function answer(pick: Pick, delaySecs: number) {
    if (busy) return;
    setBusy(pick.id);
    try { await invoke("task_continue_answer", { id: pick.id, delaySecs }); }
    catch { /* timed out or turn moved on — backend falls back to the suggestion */ }
    finally { setPicks((xs) => xs.filter((x) => x.id !== pick.id)); setBusy(null); }
  }

  if (!picks.length) return null;
  return (
    <div style={{
      position: "fixed", inset: 0, zIndex: 100, display: "flex", alignItems: "center", justifyContent: "center",
      background: "rgba(0,0,0,0.45)", padding: 24,
    }}>
      <div style={{
        width: 420, maxWidth: "100%", maxHeight: "90vh", overflowY: "auto",
        background: "var(--surface)", color: "var(--text)",
        border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
        boxShadow: "var(--elevation)", padding: 20, display: "flex", flexDirection: "column", gap: 12,
      }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <span style={{ fontSize: 18 }}>⏰</span>
          <b style={{ fontSize: 15, flex: 1 }}>How long should {picks[picks.length - 1].agentName} wait?</b>
          {picks.length > 1 && <span style={{ fontSize: 12, color: "var(--text-faint)" }}>+{picks.length - 1} queued</span>}
        </div>
        <p style={{ margin: 0, fontSize: 13, color: "var(--text-muted)", lineHeight: 1.5 }}>
          {picks[picks.length - 1].agentName} wants to check back on a long-running task. Pick the wake-up timer —
          no answer and its suggestion ({fmtDur(picks[picks.length - 1].requestedSecs)}) stands.
        </p>
        {picks[picks.length - 1].note && (
          <div style={{
            fontSize: 12.5, fontFamily: "ui-monospace, monospace", lineHeight: 1.5,
            background: "var(--bg)", border: "var(--border-width) solid var(--line)",
            borderRadius: "var(--radius-control)", padding: "8px 10px",
            maxHeight: 120, overflowY: "auto", whiteSpace: "pre-wrap", wordBreak: "break-word",
          }}>{picks[picks.length - 1].note.slice(0, 600)}</div>
        )}
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          {(picks[picks.length - 1].optionsMin?.length ? picks[picks.length - 1].optionsMin : [1, 3, 5, 10, 15]).map((m) => (
            <Button key={m} onClick={() => void answer(picks[picks.length - 1], m * 60)} disabled={busy === picks[picks.length - 1].id}>
              {m} min
            </Button>
          ))}
          <Button variant="secondary" onClick={() => void answer(picks[picks.length - 1], picks[picks.length - 1].requestedSecs)} disabled={busy === picks[picks.length - 1].id}>
            Use suggestion ({fmtDur(picks[picks.length - 1].requestedSecs)})
          </Button>
        </div>
        <p style={{ margin: 0, fontSize: 11.5, color: "var(--text-faint)" }}>
          Suggested: {fmtDur(picks[picks.length - 1].requestedSecs)} · closes itself if you don't pick
        </p>
      </div>
    </div>
  );
}
