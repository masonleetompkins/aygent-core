// AYGENT Browser — OUR in-app browser (Chromium provisioned into the app's own
// space; agents can drive it, the human can take the wheel). Enable/disable
// lives in TOOLS (Mason 08-01, UI task #4) — it IS a tool, not a setting.
// More AYGENT-branded tools will join it here.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card, Button, Pill } from "./ui";

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

export function AygentBrowser() {
  type BStatus = { installed: boolean; version: string; platform: string; executable: string | null };
  const [status, setStatus] = useState<BStatus | null>(null);
  const [phase, setPhase] = useState<string | null>(null); // resolve|download|extract|unquarantine|done
  const [pct, setPct] = useState<number | null>(null);      // 0..1 during download
  const [probe, setProbe] = useState<string | null>(null);  // reported chromium version
  const [err, setErr] = useState<string | null>(null);
  const unlistenRef = useRef<null | (() => void)>(null);

  async function refresh() {
    try { setStatus(await invoke<BStatus>("browser_status")); } catch (e) { setErr(String(e)); }
  }
  useEffect(() => { refresh(); return () => { unlistenRef.current?.(); }; }, []);

  async function enable() {
    setErr(null); setProbe(null); setPhase("resolve"); setPct(0);
    const channel = `browser-dl-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const un = await listen<any>(channel, (e) => {
      const p = e.payload || {};
      if (p.phase) setPhase(p.phase);
      const { got, total, done } = p;
      if (done) { setPct(1); }
      else if (typeof got === "number" && total) { setPct(got / total); }
    });
    unlistenRef.current = un;
    try {
      await invoke<string>("browser_install", { channel });
      await refresh();
      // Tell the app shell the sidebar entry can appear now (UI task #4).
      window.dispatchEvent(new Event("aygent-browser-changed"));
      // GATE: launch the provisioned Chromium and confirm it actually runs.
      setPhase("probe");
      const ver = await invoke<string>("browser_launch_probe");
      setProbe(ver);
      setPhase("done");
    } catch (e) {
      setErr(String(e));
      setPhase(null);
    } finally {
      un(); unlistenRef.current = null;
    }
  }

  const busy = phase !== null && phase !== "done";
  const phaseLabel: Record<string, string> = {
    resolve: "Finding the latest build…",
    download: "Downloading Chromium…",
    extract: "Unpacking…",
    unquarantine: "Finalizing…",
    probe: "Verifying it launches…",
    done: "Ready",
  };

  return (
    <Card title="AYGENT Browser">
      <p style={hint}>
        A real browser that lives inside AYGENT — your agents can see and use it, and so can you.
        Nothing to install: click Enable and AYGENT downloads Chromium into its own space, just like a local model.
      </p>
      {status?.installed ? (
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <Pill tone="ok">installed ✓</Pill>
          <span style={{ ...hint, fontFamily: "ui-monospace, monospace", fontSize: 12 }}>
            Chromium {status.version} · {status.platform}
          </span>
          {probe && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>launch OK — {probe}</span>}
        </div>
      ) : busy ? (
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <span style={{ fontSize: 13 }}>{phaseLabel[phase!] ?? phase}</span>
          {phase === "download" && pct !== null && (
            <span style={{ fontSize: 12, fontFamily: "ui-monospace, monospace", width: 60, textAlign: "right" }}>
              {Math.round(pct * 100)}%
            </span>
          )}
        </div>
      ) : (
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <Button onClick={enable}>Enable Browser</Button>
          <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>~150MB · one-time download</span>
        </div>
      )}
      {err && <Pill tone="danger">✗ {err}</Pill>}
    </Card>
  );
}
