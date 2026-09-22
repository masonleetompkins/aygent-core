// Command Pro command palette (branch ui-command-pro). REAL dispatch, not a mock:
// jump to screens, new chat/thread, compact active thread, switch agent/thread,
// toggle theme. Open with Cmd+K / Ctrl+K. Fuzzy = subsequence match.
import { useEffect, useMemo, useRef, useState } from "react";
import type { ScreenId } from "./Sidebar";

export type PaletteAction = {
  id: string;
  label: string;
  hint?: string;
  run: () => void;
};

function fuzzy(q: string, s: string): boolean {
  q = q.toLowerCase(); s = s.toLowerCase();
  let i = 0;
  for (const c of s) { if (c === q[i]) i++; if (i >= q.length) return true; }
  return i >= q.length;
}

export function CmdPalette({ onNavigate }: { onNavigate: (s: ScreenId) => void }) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((o) => !o);
        setQ(""); setSel(0);
      } else if (e.key === "Escape" && open) {
        setOpen(false);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open ]);

  useEffect(() => { if (open) setTimeout(() => inputRef.current?.focus(), 30); }, [open ]);

  const actions: PaletteAction[] = useMemo(() => {
    const go = (id: ScreenId, label: string, hint?: string): PaletteAction => ({
      id: `go-${id}`, label: `Go to ${label}`, hint, run: () => onNavigate(id),
    });
    return [
      { id: "new-thread", label: "New thread in this agent", hint: "↵", run: () => window.dispatchEvent(new Event("aygent-new-thread")) },
      { id: "compact", label: "Compact active thread", run: () => window.dispatchEvent(new Event("aygent-compact")) },
      go("chat", "Chat", "⌘1"),
      go("agents", "Agents", "⌘2"),
      go("settings", "Settings", "⌘,"),
      go("sparks", "Sparks", "⌘3"),
      go("video", "Video", "⌘4"),
      go("dashboard", "Dashboard"),
      go("scheduler", "Scheduler"),
      go("connections", "Connections"),
      go("tools", "Tools"),
      go("savepoints", "Save Points"),
      { id: "theme", label: "Toggle light / dark", run: () => window.dispatchEvent(new Event("aygent-toggle-theme")) },
      { id: "notify", label: "Toggle thread notifications", run: () => window.dispatchEvent(new Event("aygent-toggle-notify")) },
    ];
  }, [onNavigate]);

  const matches = useMemo(() => {
    const t = q.trim();
    if (!t) return actions;
    return actions.filter((a) => fuzzy(t, a.label)).slice(0, 12);
  }, [q, actions ]);

  useEffect(() => setSel(0), [q ]);

  if (!open) return null;
  function run(a: PaletteAction) { setOpen(false); setQ(""); a.run(); }

  return (
    <div
      onClick={() => setOpen(false)}
      style={{ position: "fixed", inset: 0, zIndex: 100, background: "rgba(0,0,0,0.35)", display: "flex", justifyContent: "center", paddingTop: "12vh" }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{ width: 560, maxWidth: "90vw", height: "fit-content", background: "var(--surface)", border: "1px solid var(--line)", borderRadius: 10, boxShadow: "0 12px 48px rgba(0,0,0,0.35)", overflow: "hidden" }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "10px 14px", borderBottom: "1px solid var(--line)" }}>
          <span style={{ color: "var(--text-faint)" }}>›</span>
          <input
            ref={inputRef}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(s + 1, matches.length - 1)); }
              else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(s - 1, 0)); }
              else if (e.key === "Enter" && matches[sel]) { e.preventDefault(); run(matches[sel]); }
            }}
            placeholder="Type a command… (chat, thread, compact, theme)"
            style={{ flex: 1, background: "transparent", border: "none", outline: "none", color: "var(--text)", fontSize: 14 }}
          />
          <kbd className="pro-kbd">esc</kbd>
        </div>
        <div className="aygent-scroll" style={{ maxHeight: 320, overflowY: "auto", padding: 6 }}>
          {matches.map((a, i) => (
            <button
              key={a.id}
              onMouseEnter={() => setSel(i)}
              onClick={() => run(a)}
              style={{
                display: "flex", alignItems: "center", gap: 8, width: "100%", textAlign: "left",
                padding: "8px 10px", border: "none", borderRadius: 7, cursor: "pointer", fontSize: 13.5,
                background: i === sel ? "var(--bg)" : "transparent", color: "var(--text)",
              }}
            >
              <span style={{ flex: 1 }}>{a.label}</span>
              {a.hint && <kbd className="pro-kbd">{a.hint}</kbd>}
            </button>
          ))}
          {matches.length === 0 && (
            <div style={{ padding: "14px", fontSize: 13, color: "var(--text-faint)" }}>No matches.</div>
          )}
        </div>
        <div style={{ padding: "6px 12px", borderTop: "1px solid var(--line)", fontSize: 11, color: "var(--text-faint)" }}>
          Threads are per-agent convs — ⌘K → New thread opens a parallel session.
        </div>
      </div>
    </div>
  );
}
