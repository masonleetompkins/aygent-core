// AYGENT — Browser screen: REAL embedded webview + seamless agent hand-off.
//
// The page is a REAL native webview (WKWebView) rendered by Tauri, floated over
// this pane's content area. Crisp text, native selection, hover, scroll — you
// browse normally. This React layer draws the CHROME around it: tabs, address
// bar, the who's-driving HUD, and THE MAGIC — a hand-off toggle. Flip to "Agent"
// and a prompt bar appears; you tell the agent what to do and it acts in the
// SAME window you're looking at (via eval into the webview), while you watch.
// Flip back instantly — same page, same session.
//
// Because the native webview floats OVER this pane, we measure the page-area
// rect and tell Rust to position the webview there; we re-measure on layout/
// resize + hide it when you leave the tab.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Input, Pill } from "../components/ui";
import { Icon } from "../components/Icon";

type Tab = { id: number; addr: string; title: string; editing: boolean };
let TAB_SEQ = 1;
const newTab = (): Tab => ({ id: TAB_SEQ++, addr: "", title: "New Tab", editing: true });

export function Browser() {
  const [installed] = useInstalled();
  const [tabs, setTabs] = useState<Tab[]>([newTab()]);
  const [activeId, setActiveId] = useState<number>(() => tabs[0].id);
  const [driver, setDriver] = useState<"human" | "agent">("human");
  const [agentPrompt, setAgentPrompt] = useState("");
  const [agentBusy, setAgentBusy] = useState(false);
  const [agentLog, setAgentLog] = useState<string[]>([]);
  const editRef = useRef<HTMLInputElement>(null);
  // The div whose rect the native webview is positioned over.
  const paneRef = useRef<HTMLDivElement>(null);

  const active = tabs.find((t) => t.id === activeId) ?? tabs[0];
  function patch(id: number, p: Partial<Tab>) {
    setTabs((ts) => ts.map((t) => (t.id === id ? { ...t, ...p } : t)));
  }

  // Position the native webview over the pane rect (device-independent CSS px).
  function syncBounds() {
    const el = paneRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    invoke("webview_set_bounds", { x: r.left, y: r.top, width: r.width, height: r.height }).catch(() => {});
  }

  // Mount: on leaving the tab, hide the native webview so it doesn't float over
  // other screens. Track resize/scroll to keep it aligned.
  useEffect(() => {
    if (installed !== true) return;
    syncBounds();
    const onResize = () => syncBounds();
    window.addEventListener("resize", onResize);
    const iv = setInterval(syncBounds, 500); // catch layout shifts cheaply
    return () => {
      window.removeEventListener("resize", onResize);
      clearInterval(iv);
      invoke("webview_hide").catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [installed]);

  async function go(id: number) {
    const tab = tabs.find((t) => t.id === id);
    if (!tab) return;
    const url = tab.addr.trim();
    if (!url) { patch(id, { editing: false }); return; }
    patch(id, { editing: false });
    const el = paneRef.current;
    const r = el?.getBoundingClientRect();
    // First navigation for the pane opens/positions the webview; later ones reuse.
    await invoke("webview_open", {
      url, x: r?.left ?? 0, y: r?.top ?? 0, width: r?.width ?? 800, height: r?.height ?? 600,
    }).catch((e) => setAgentLog((l) => [...l, `open failed: ${e}`]));
  }

  function clickTab(id: number) {
    if (id === activeId) { patch(id, { editing: true }); setTimeout(() => editRef.current?.select(), 0); }
    else setActiveId(id);
  }
  function addTab() { const t = newTab(); setTabs((ts) => [...ts, t]); setActiveId(t.id); }
  function closeTab(id: number) {
    setTabs((ts) => {
      const next = ts.filter((t) => t.id !== id);
      if (next.length === 0) { const t = newTab(); setActiveId(t.id); return [t]; }
      if (id === activeId) setActiveId(next[next.length - 1].id);
      return next;
    });
  }

  // THE HAND-OFF. Flip to agent: the prompt bar appears. Flip to human: you drive.
  function toggleDriver() {
    setDriver((d) => (d === "human" ? "agent" : "human"));
  }

  async function runAgent() {
    const task = agentPrompt.trim();
    if (!task || agentBusy) return;
    setAgentBusy(true);
    setAgentLog((l) => [...l, `▸ ${task}`]);
    try {
      const res = await invoke<string>("webview_agent_act", { task });
      setAgentLog((l) => [...l, res]);
    } catch (e) {
      setAgentLog((l) => [...l, `✗ ${e}`]);
    } finally {
      setAgentBusy(false);
      setAgentPrompt("");
    }
  }

  if (installed === false) {
    return (
      <div style={{ padding: 4 }}>
        <div style={{ fontSize: "var(--text-h1)", fontWeight: 800 }}>Browser</div>
        <p style={{ color: "var(--text-muted)", fontSize: 14, marginTop: 8 }}>
          The in-app browser isn’t enabled yet. Turn it on in Settings — AYGENT downloads Chromium
          into its own space, just like a local model. Then come back here to browse.
        </p>
      </div>
    );
  }

  const label = (t: Tab) => t.title || (t.addr.trim() ? t.addr : "New Tab");

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0, gap: 8 }}>
      {/* TAB ROW — active tab is the inline address field. */}
      <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
        {tabs.map((t) => {
          const on = t.id === activeId;
          const editing = on && t.editing;
          return (
            <div key={t.id} onClick={() => clickTab(t.id)}
              style={{
                display: "flex", alignItems: "center", gap: 7,
                width: editing ? 360 : "auto", maxWidth: editing ? 360 : 240,
                padding: "7px 11px", cursor: on ? "text" : "pointer",
                borderRadius: "var(--radius-control)",
                border: `var(--border-width) solid ${on ? "var(--accent)" : "var(--line)"}`,
                background: on ? "var(--surface)" : "var(--bg)",
                color: on ? "var(--text)" : "var(--text-muted)",
                boxShadow: on ? "var(--elevation)" : "none",
              }}>
              <Icon name="globe" size={14} />
              {editing ? (
                <input ref={editRef} autoFocus value={t.addr}
                  onChange={(e) => patch(t.id, { addr: e.target.value })}
                  onKeyDown={(e) => { if (e.key === "Enter") go(t.id); if (e.key === "Escape") patch(t.id, { editing: false }); }}
                  onBlur={() => patch(t.id, { editing: false })}
                  onClick={(e) => e.stopPropagation()}
                  placeholder="Enter a URL or search…"
                  style={{ flex: 1, minWidth: 0, border: "none", outline: "none", background: "transparent", color: "var(--text)", fontSize: 13, fontFamily: "inherit" }} />
              ) : (
                <span style={{ flex: 1, fontSize: 13, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{label(t)}</span>
              )}
              <span onClick={(e) => { e.stopPropagation(); closeTab(t.id); }}
                style={{ fontSize: 15, lineHeight: 1, opacity: 0.55, paddingLeft: 2 }} title="Close tab">×</span>
            </div>
          );
        })}
        <button onClick={addTab} title="New tab"
          style={{ display: "flex", alignItems: "center", justifyContent: "center", width: 32, height: 32, borderRadius: "var(--radius-control)", border: "var(--border-width) dashed var(--line)", background: "transparent", color: "var(--text-muted)", cursor: "pointer", fontSize: 18, lineHeight: 1, flexShrink: 0 }}>+</button>

        {/* THE HAND-OFF TOGGLE — the centerpiece. */}
        <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 8 }}>
          <div style={{ display: "flex", borderRadius: "var(--radius-control)", overflow: "hidden", border: "var(--border-width) solid var(--line)" }}>
            <button onClick={() => setDriver("human")}
              style={segStyle(driver === "human")}>You</button>
            <button onClick={() => setDriver("agent")}
              style={segStyle(driver === "agent")}>Agent</button>
          </div>
        </div>
      </div>

      {/* AGENT PROMPT BAR — appears when you hand off. You tell the agent what to
          do; it acts in the SAME window you're looking at. */}
      {driver === "agent" && (
        <div style={{ display: "flex", flexDirection: "column", gap: 6, border: "var(--border-width) solid var(--accent)", borderRadius: "var(--radius-card)", padding: 10, background: "var(--surface)" }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <Icon name="sparkles" size={15} />
            <span style={{ fontSize: 13, fontWeight: 700 }}>Tell the agent what to do in this page</span>
            <span style={{ marginLeft: "auto", fontSize: 12, color: "var(--text-faint)" }}>You’re watching — take back control anytime with “You”.</span>
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            <div style={{ flex: 1 }}>
              <Input value={agentPrompt} onChange={(e: any) => setAgentPrompt(e.target.value)}
                onKeyDown={(e: any) => { if (e.key === "Enter") runAgent(); }}
                placeholder='e.g. "click the login button" or "summarize this page"' />
            </div>
            <button onClick={runAgent} disabled={agentBusy || !agentPrompt.trim()}
              style={{ fontSize: 13, padding: "8px 16px", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--accent)", background: "var(--accent)", color: "#fff", cursor: "pointer", opacity: agentBusy || !agentPrompt.trim() ? 0.5 : 1 }}>
              {agentBusy ? "Working…" : "Act"}
            </button>
          </div>
          {agentLog.length > 0 && (
            <div style={{ maxHeight: 96, overflow: "auto", fontSize: 12, fontFamily: "ui-monospace, monospace", color: "var(--text-muted)", display: "flex", flexDirection: "column", gap: 2 }}>
              {agentLog.slice(-6).map((l, i) => <div key={i}>{l}</div>)}
            </div>
          )}
        </div>
      )}

      {/* THE PAGE AREA — the native webview floats over THIS div. When the agent
          is driving we dim + block pointer events so you don't fight it. */}
      <div ref={paneRef} style={{
        flex: 1, minHeight: 0, position: "relative",
        border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
        background: "var(--bg)", overflow: "hidden",
      }}>
        {/* Placeholder shown only before first navigation (webview not yet over it). */}
        <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", color: "var(--text-faint)", fontSize: 14, pointerEvents: "none" }}>
          Click the tab to type a URL, or search.
        </div>
        {driver === "agent" && (
          <div style={{ position: "absolute", inset: 0, background: "rgba(0,0,0,0.04)", pointerEvents: "none", boxShadow: "inset 0 0 0 2px var(--accent)" }} />
        )}
      </div>
    </div>
  );
}

function segStyle(on: boolean): React.CSSProperties {
  return {
    fontSize: 12, fontWeight: 700, padding: "6px 14px", border: "none", cursor: "pointer",
    background: on ? "var(--accent)" : "transparent",
    color: on ? "#fff" : "var(--text-muted)",
  };
}

function useInstalled(): [boolean | null] {
  const [installed, setInstalled] = useState<boolean | null>(null);
  useEffect(() => {
    invoke<{ installed: boolean }>("browser_status")
      .then((s) => setInstalled(s.installed))
      .catch(() => setInstalled(false));
  }, []);
  return [installed];
}
