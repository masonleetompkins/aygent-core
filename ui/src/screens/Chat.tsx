// Chat — the primary agent surface. Streaming-first: tokens render live, tool
// calls appear as inline cards as they fire, multi-turn history persists.
// Consumes normalized StreamEvents from the Rust streaming agent loop over a
// Tauri event channel. Falls back to a thinking animation if no text streams.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button, Input } from "../components/ui";

type ToolLine = { name: string; path: string; ok?: boolean; detail?: string };
type Msg =
  | { role: "user"; text: string }
  | { role: "assistant"; text: string; tools: ToolLine[]; streaming?: boolean };

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

export function Chat({ folder, keySet }: { folder: string | null; keySet: boolean }) {
  const [msgs, setMsgs] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const historyRef = useRef<any>([]); // provider-format running history
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => { scrollRef.current?.scrollTo({ top: 1e9, behavior: "smooth" }); }, [msgs]);

  async function send() {
    const prompt = input.trim();
    if (!prompt || busy) return;
    setInput(""); setBusy(true);

    setMsgs((m) => [...m, { role: "user", text: prompt },
      { role: "assistant", text: "", tools: [], streaming: true }]);

    const channel = `agent://${Date.now()}`;

    // Accumulate into a REF (source of truth for THIS turn), then mirror into
    // state for rendering. With StrictMode removed there is exactly ONE listener
    // per turn, so no id-dedupe games are needed — every event is appended once.
    const acc = { text: "", tools: [] as ToolLine[] };

    const mirror = () => setMsgs((m) => {
      const copy = [...m];
      const last = copy[copy.length - 1];
      if (last?.role === "assistant") {
        last.text = acc.text;
        last.tools = acc.tools.map((t) => ({ ...t }));
      }
      return copy;
    });

    const unlisten = await listen<any>(channel, (e) => {
      const ev = e.payload;
      switch (ev.kind) {
        case "TextDelta": acc.text += ev.text; break;
        case "ToolUse": acc.tools.push({ name: ev.name, path: ev.input?.path ?? "" }); break;
        case "ToolResult": {
          for (let i = acc.tools.length - 1; i >= 0; i--) {
            if (acc.tools[i].name === ev.name && acc.tools[i].ok === undefined) {
              acc.tools[i] = { ...acc.tools[i], ok: ev.ok, detail: ev.detail }; break;
            }
          }
          break;
        }
        case "Error": acc.text += `\n✗ ${ev.text}`; break;
        case "Info": case "Done": break;
      }
      mirror();
    });

    try {
      const updated = await invoke<any>("agent_stream", {
        channel, prompt, history: historyRef.current,
      });
      historyRef.current = updated;
    } catch (err) {
      acc.text += `\n✗ ${String(err)}`;
      mirror();
    } finally {
      unlisten();
      setMsgs((m) => { const c = [...m]; const l = c[c.length - 1]; if (l?.role === "assistant") l.streaming = false; return c; });
      setBusy(false);
    }
  }

  const blocked = !folder || !keySet;

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "calc(100vh - 130px)", maxWidth: 720 }}>
      <h2 style={{ fontSize: 22, fontWeight: 800, margin: "0 0 12px" }}>Chat</h2>

      {blocked && (
        <p style={{ ...hint, marginBottom: 12 }}>
          {!folder ? "Pick an Agent Folder in Settings, " : ""}{!keySet ? "add an Anthropic key in Settings" : ""} to start.
        </p>
      )}

      <div ref={scrollRef} style={{ flex: 1, overflowY: "auto", display: "flex", flexDirection: "column", gap: 14, paddingRight: 6 }}>
        {msgs.length === 0 && !blocked && (
          <p style={hint}>Say hello, or ask your agent to work with files in your folder.</p>
        )}
        {msgs.map((m, i) => <Bubble key={i} m={m} />)}
      </div>

      <div style={{ display: "flex", gap: 8, marginTop: 14 }}>
        <Input value={input} disabled={blocked || busy}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") send(); }}
          placeholder={blocked ? "Set up folder + key in Settings first…" : "Message your agent…"} />
        <Button onClick={send} disabled={blocked || busy}>{busy ? "…" : "Send"}</Button>
      </div>
    </div>
  );
}

function Bubble({ m }: { m: Msg }) {
  const isUser = m.role === "user";
  return (
    <div style={{ display: "flex", justifyContent: isUser ? "flex-end" : "flex-start" }}>
      <div style={{
        maxWidth: "82%",
        background: isUser ? "var(--accent)" : "var(--surface)",
        color: isUser ? "var(--bg)" : "var(--text)",
        border: "var(--border-width) solid var(--line)",
        borderRadius: "var(--radius-card)",
        boxShadow: "var(--elevation)",
        padding: "12px 15px",
        display: "flex", flexDirection: "column", gap: 8,
      }}>
        {!isUser && m.role === "assistant" && m.tools.map((t, i) => <ToolCard key={i} t={t} />)}
        {m.text && <span style={{ whiteSpace: "pre-wrap", lineHeight: 1.55, fontSize: 15 }}>{m.text}</span>}
        {!isUser && m.role === "assistant" && m.streaming && !m.text && <Thinking />}
      </div>
    </div>
  );
}

function ToolCard({ t }: { t: ToolLine }) {
  const pending = t.ok === undefined;
  const color = pending ? "var(--text-muted)" : t.ok ? "var(--ok)" : "var(--danger)";
  // A path is "revealable" once the call succeeded and points at a real file
  // (list_files on '.' or a refused call has nothing useful to reveal).
  const revealable = t.ok === true && !!t.path && t.path !== ".";

  async function reveal() {
    try { await invoke("reveal_in_finder", { path: t.path }); }
    catch (err) { console.warn("reveal failed:", err); }
  }

  return (
    <div style={{
      display: "flex", alignItems: "center", gap: 8,
      fontFamily: "ui-monospace, monospace", fontSize: 12.5,
      border: `var(--border-width) solid ${color}`, color,
      borderRadius: "var(--radius-control)", padding: "6px 10px",
      background: "var(--bg)",
    }}>
      <span>
        ⚙ {t.name}(
        {revealable ? (
          <button
            onClick={reveal}
            title="Reveal in Finder"
            style={{
              font: "inherit", color: "inherit", background: "none", border: "none",
              padding: 0, cursor: "pointer", textDecoration: "underline",
              textUnderlineOffset: 2,
            }}
          >{t.path}</button>
        ) : t.path}
        )
      </span>
      <span style={{ marginLeft: "auto" }}>{pending ? "…" : t.ok ? "✓" : `✗ ${t.detail || "refused"}`}</span>
    </div>
  );
}

function Thinking() {
  return (
    <span style={{ display: "inline-flex", gap: 4, alignItems: "center", color: "var(--text-muted)" }}>
      {[0, 1, 2].map((i) => (
        <span key={i} style={{
          width: 6, height: 6, borderRadius: 999, background: "currentColor",
          animation: `aygentPulse 1s ${i * 0.15}s infinite ease-in-out`,
        }} />
      ))}
      <style>{`@keyframes aygentPulse{0%,100%{opacity:.25}50%{opacity:1}}`}</style>
    </span>
  );
}
