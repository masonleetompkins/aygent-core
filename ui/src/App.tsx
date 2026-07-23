import { useEffect, useState } from "react";

// Phase 0 (M0.1) proof-of-life UI. Confirms: the window opens, the WebView
// runs React, and (once M0.1 wiring lands) it can auth to the daemon WS with
// the injected per-session token and round-trip a message.
export function App() {
  const [status, setStatus] = useState("booting…");

  useEffect(() => {
    // M0.1: read the WS port + token injected by Rust (Tauri state / IPC),
    // connect, send the auth frame, show connection status. Stubbed for now.
    setStatus("UI up — daemon WS wiring lands in M0.1");
  }, []);

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
      <h1 style={{ letterSpacing: "0.15em", color: "#2dd4bf" }}>AYGENT</h1>
      <p style={{ color: "#8fa9b6" }}>The AI agent you actually own.</p>
      <code style={{ color: "#8fa9b6", fontSize: "0.85rem" }}>{status}</code>
    </main>
  );
}
