import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

// M0.1 proof-of-life: connect the UI to the daemon over the authenticated WS.
// Flow: ask Rust for {port, token} -> open ws -> send auth frame -> on auth:ok
// send a ping -> show live connection status. This proves the shell, daemon,
// and WS-auth (Atlas C6) all work end to end.

type Status =
  | { kind: "booting" }
  | { kind: "no-daemon" }
  | { kind: "connecting"; port: number }
  | { kind: "connected"; port: number; latency?: number }
  | { kind: "error"; msg: string };

export function App() {
  const [status, setStatus] = useState<Status>({ kind: "booting" });

  useEffect(() => {
    let ws: WebSocket | null = null;
    let cancelled = false;

    async function connect() {
      let info: { port: number | null; token: string };
      try {
        info = await invoke("daemon_info");
      } catch (e) {
        setStatus({ kind: "error", msg: `daemon_info failed: ${String(e)}` });
        return;
      }
      if (cancelled) return;
      if (!info.port) {
        // daemon may still be booting; retry shortly
        setStatus({ kind: "no-daemon" });
        setTimeout(connect, 600);
        return;
      }

      setStatus({ kind: "connecting", port: info.port });
      ws = new WebSocket(`ws://127.0.0.1:${info.port}`);
      let sentAt = 0;

      ws.onopen = () => ws!.send(JSON.stringify({ type: "auth", token: info.token }));
      ws.onmessage = (ev) => {
        let msg: any;
        try { msg = JSON.parse(ev.data); } catch { return; }
        if (msg.type === "auth:ok") {
          sentAt = Date.now();
          ws!.send(JSON.stringify({ type: "ping" }));
        } else if (msg.type === "pong") {
          setStatus({ kind: "connected", port: info.port!, latency: Date.now() - sentAt });
        }
      };
      ws.onerror = () => setStatus({ kind: "error", msg: "ws error" });
      ws.onclose = () => setStatus((s) => (s.kind === "connected" ? s : { kind: "error", msg: "ws closed (auth rejected?)" }));
    }

    connect();
    return () => { cancelled = true; ws?.close(); };
  }, []);

  const line = statusLine(status);
  const good = status.kind === "connected";

  return (
    <main
      style={{
        fontFamily: "-apple-system, system-ui, sans-serif",
        background: "#0a0f14",
        color: "#e8f4f8",
        height: "100vh",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: "0.75rem",
      }}
    >
      <h1 style={{ letterSpacing: "0.15em", color: "#2dd4bf", margin: 0 }}>AYGENT</h1>
      <p style={{ color: "#8fa9b6", margin: 0 }}>The AI agent you actually own.</p>
      <code
        style={{
          marginTop: "1.5rem",
          fontSize: "0.9rem",
          color: good ? "#2dd4bf" : "#8fa9b6",
          border: `1px solid ${good ? "#14b8a6" : "#1b2a35"}`,
          borderRadius: "999px",
          padding: "0.4rem 1rem",
        }}
      >
        {good ? "● " : "○ "}{line}
      </code>
    </main>
  );
}

function statusLine(s: Status): string {
  switch (s.kind) {
    case "booting": return "booting…";
    case "no-daemon": return "waiting for daemon…";
    case "connecting": return `connecting to daemon :${s.port}…`;
    case "connected": return `daemon connected ✓  (:${s.port}, ${s.latency ?? "?"}ms)`;
    case "error": return `not connected — ${s.msg}`;
  }
}
