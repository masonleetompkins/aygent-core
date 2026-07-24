// Checkpoints — the rewind timeline. AYGENT snapshots the agent folder before
// every turn (Rust checkpoint module, git-backed per Contract C4). This screen
// lists those snapshots newest-first and lets you restore the folder to any of
// them with one click. Restoring first snapshots the current state, so a rewind
// is itself undoable.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Pill } from "../components/ui";

type Checkpoint = {
  id: string;
  message: string;
  timestamp: number; // unix seconds
  files: number;
  is_current: boolean;
};
type Timeline = { items: Checkpoint[]; can_undo: boolean; can_redo: boolean };

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

export function Checkpoints({ folder }: { folder: string | null }) {
  const [items, setItems] = useState<Checkpoint[]>([]);
  const [canUndo, setCanUndo] = useState(false);
  const [canRedo, setCanRedo] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [confirmId, setConfirmId] = useState<string | null>(null);

  async function refresh() {
    if (!folder) return;
    setErr(null);
    try {
      const t = await invoke<Timeline>("checkpoint_timeline");
      setItems(t.items); setCanUndo(t.can_undo); setCanRedo(t.can_redo);
    } catch (e) { setErr(String(e)); }
  }

  useEffect(() => { refresh(); /* eslint-disable-next-line */ }, [folder]);

  async function snapshotNow() {
    setBusy(true); setErr(null);
    try { await invoke("checkpoint_snapshot", { label: "manual checkpoint" }); await refresh(); }
    catch (e) { setErr(String(e)); }
    finally { setBusy(false); }
  }

  async function step(cmd: "checkpoint_undo" | "checkpoint_redo") {
    setBusy(true); setErr(null);
    try { await invoke(cmd); await refresh(); }
    catch (e) { setErr(String(e)); }
    finally { setBusy(false); }
  }

  async function rewind(id: string) {
    setBusy(true); setErr(null); setConfirmId(null);
    try { await invoke("checkpoint_rewind", { target: id }); await refresh(); }
    catch (e) { setErr(String(e)); }
    finally { setBusy(false); }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18, maxWidth: 680 }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: 12 }}>
        <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Checkpoints</h2>
        <span style={hint}>Rewind your folder to any earlier state.</span>
      </div>

      {!folder && <Pill tone="muted">Pick an Agent Folder in Settings first.</Pill>}

      {folder && (
        <>
          <div style={{ display: "flex", gap: 8 }}>
            <Button onClick={() => step("checkpoint_undo")} disabled={busy || !canUndo}>↶ Undo</Button>
            <Button onClick={() => step("checkpoint_redo")} disabled={busy || !canRedo}>↷ Redo</Button>
            <div style={{ width: 1, background: "var(--line)", margin: "2px 4px" }} />
            <Button variant="secondary" onClick={snapshotNow} disabled={busy}>{busy ? "…" : "Snapshot now"}</Button>
            <Button variant="secondary" onClick={refresh} disabled={busy}>Refresh</Button>
          </div>

          {err && <Pill tone="danger">✗ {err}</Pill>}

          {items.length === 0 && !err && (
            <Card>
              <p style={hint}>
                No checkpoints yet. One is taken automatically before each turn that changes
                files — or hit <b>Snapshot now</b> to make one.
              </p>
            </Card>
          )}

          {items.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              {items.map((c) => (
                <CheckpointRow
                  key={c.id}
                  c={c}
                  busy={busy}
                  confirming={confirmId === c.id}
                  onAskRewind={() => setConfirmId(c.id)}
                  onCancel={() => setConfirmId(null)}
                  onConfirm={() => rewind(c.id)}
                />
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}

function CheckpointRow({
  c, busy, confirming, onAskRewind, onCancel, onConfirm,
}: {
  c: Checkpoint; busy: boolean; confirming: boolean;
  onAskRewind: () => void; onCancel: () => void; onConfirm: () => void;
}) {
  return (
    <Card style={{ padding: "14px 18px", gap: 8 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
        <div style={{ display: "flex", flexDirection: "column", gap: 3, minWidth: 0, flex: 1 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <span style={{ fontWeight: 600, fontSize: 15, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
              {c.message || "checkpoint"}
            </span>
            {c.is_current && <Pill tone="ok">current</Pill>}
          </div>
          <div style={{ display: "flex", gap: 10, fontSize: 12.5, color: "var(--text-muted)", fontFamily: "ui-monospace, monospace" }}>
            <span>{c.id}</span>
            <span>·</span>
            <span>{relTime(c.timestamp)}</span>
            {c.files > 0 && <><span>·</span><span>{c.files} file{c.files === 1 ? "" : "s"}</span></>}
          </div>
        </div>

        {!c.is_current && !confirming && (
          <Button variant="secondary" onClick={onAskRewind} disabled={busy}>Rewind here</Button>
        )}
        {confirming && (
          <div style={{ display: "flex", gap: 6 }}>
            <Button onClick={onConfirm} disabled={busy}>{busy ? "…" : "Confirm rewind"}</Button>
            <Button variant="secondary" onClick={onCancel} disabled={busy}>Cancel</Button>
          </div>
        )}
      </div>
    </Card>
  );
}

function relTime(unixSec: number): string {
  if (!unixSec) return "";
  const d = new Date(unixSec * 1000);
  const diff = Date.now() - d.getTime();
  const min = Math.round(diff / 60000);
  if (min < 1) return "just now";
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  return d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}
