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
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Input } from "../components/ui";
import { Icon } from "../components/Icon";

type Tab = { id: number; addr: string; title: string; editing: boolean };
let TAB_SEQ = 1;
// Width (CSS px) of the right-hand agent pane when handed off. syncBounds
// shrinks the native webview by this + a gap so the pane sits BESIDE the page.
const AGENT_PANE_W = 340;
// --radius-card. The native WKWebView's OWN CALayer is rounded to this in Rust
// (cornerRadius + masksToBounds) so the OS clips the page to a rounded rect —
// sizes match the pane EXACTLY (no inset), and the DOM frame overlaps only the
// corner pixels for the accent border. Sent to Rust so the radius is single-
// sourced from the theme.
const FRAME_RADIUS = 14;
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
  // Last rect we pushed to Rust — dedupe so we don't re-apply an unchanged rect.
  const lastBoundsRef = useRef<string>("");

  const active = tabs.find((t) => t.id === activeId) ?? tabs[0];
  function patch(id: number, p: Partial<Tab>) {
    setTabs((ts) => ts.map((t) => (t.id === id ? { ...t, ...p } : t)));
  }

  // Position the native webview EXACTLY over the page-area rect (CSS px). The
  // webview is a real OS layer that paints on top of our React chrome, so it
  // MUST be confined to the pane rect or it covers the tab/address bar (which it
  // was doing). Measure via rAF so we read post-layout numbers, and clamp to
  // non-negative sizes.
  // The webview tracks the LEFT region of the page area; in agent mode the
  // right pane takes AGENT_PANE_W, so we shrink the webview to leave room — the
  // agent prompt pane sits BESIDE the page, not over it.
  function syncBounds() {
    const el = paneRef.current;
    if (!el) return;
    // DOUBLE rAF: read AFTER React commit + browser layout/paint, so r.top and
    // parentHeight come from the SAME settled frame. wry Y-flips the child
    // against the parent NSView height at apply-time; sending the height we
    // measured against lets Rust flip deterministically (kills the resize race
    // where the child rides too high because Rust flipped against a stale
    // height). Diagnosis: Atlas, from wry wkwebview source.
    requestAnimationFrame(() => requestAnimationFrame(() => {
      const el2 = paneRef.current;
      if (!el2) return;
      const r = el2.getBoundingClientRect();
      if (r.width < 40 || r.height < 40) return;
      if (r.left <= 4 && r.top <= 4) return; // origin-hugging = not laid out yet
      // Main webview content-area logical height == the parent NSView height
      // wry flips against. Measured in the same frame as r. No hardcoding.
      const parentHeight = Math.round(document.documentElement.clientHeight);
      const clientWidth = Math.round(document.documentElement.clientWidth);
      const clientHeight = parentHeight;
      const rightPane = driver === "agent" ? AGENT_PANE_W + 10 : 0; // +gap
      // Webview rect == paneRef's BORDER-BOX rect (no inset). The rounded
      // outline is a pointer-events:none overlay rendered ON TOP of the webview,
      // so its ~radius corner arcs mask the webview's square corners while the
      // webview fills the whole box.
      const bounds = {
        x: Math.round(r.left), y: Math.round(r.top),
        width: Math.round(Math.max(r.width - rightPane, 1)),
        height: Math.round(r.height),
        radius: FRAME_RADIUS,
        // Content-area size the rect was measured against — Rust uses this to
        // compute the native titlebar inset at runtime (THE fix).
        clientWidth, clientHeight,
        parentHeight,
      };
      // Skip redundant calls — only push when the rect actually changed.
      const key = `${bounds.x},${bounds.y},${bounds.width},${bounds.height},${bounds.clientWidth},${bounds.clientHeight}`;
      if (key === lastBoundsRef.current) return;
      lastBoundsRef.current = key;
      invoke("webview_set_bounds", bounds).catch(() => {});
    }));
  }

  // Mount: on leaving the tab, hide the native webview so it doesn't float over
  // other screens. Track resize/scroll to keep it aligned.
  useEffect(() => {
    if (installed !== true) return;
    syncBounds();
    const onResize = () => syncBounds();
    window.addEventListener("resize", onResize);
    // Track the pane's own size changes (sidebar collapse, window resize, agent
    // pane toggle) precisely instead of blind-polling.
    let ro: ResizeObserver | undefined;
    if (paneRef.current && "ResizeObserver" in window) {
      ro = new ResizeObserver(() => syncBounds());
      ro.observe(paneRef.current);
    }
    // Native window resize + fullscreen↔windowed transitions: AppKit relays out
    // the parent AFTER the event, so re-sync on the event AND a beat later to
    // catch the settled parent height (Atlas: the settle covers wry's post-
    // transition re-flip).
    const winUn = getCurrentWindow().onResized(() => { syncBounds(); setTimeout(syncBounds, 60); });
    return () => {
      window.removeEventListener("resize", onResize);
      ro?.disconnect();
      winUn.then((f) => f()).catch(() => {});
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
    // Guard the OPEN rect too — the embed is BORN at this rect, so an origin-
    // hugging / degenerate measurement here creates the child covering the whole
    // window (the pop-out) and no later set_bounds fully recovers it. If the
    // pane isn't laid out yet, fall back to a safe inset and let syncBounds
    // correct it on the next frame.
    let ox = Math.round(r?.left ?? 0), oy = Math.round(r?.top ?? 0);
    let ow = Math.round(r?.width ?? 800), oh = Math.round(r?.height ?? 600);
    if (ow < 40 || oh < 40 || (ox <= 4 && oy <= 4)) {
      // Not laid out: use a conservative inset (right of sidebar, below tabs).
      ox = 300; oy = 100; ow = 600; oh = 500;
    }
    // eslint-disable-next-line no-console
    console.log("[browser] webview_open →", { x: ox, y: oy, width: ow, height: oh }, "raw:", { l: r?.left, t: r?.top, w: r?.width, h: r?.height });
    const clientWidth = Math.round(document.documentElement.clientWidth);
    const clientHeight = Math.round(document.documentElement.clientHeight);
    await invoke("webview_open", { url, x: ox, y: oy, width: ow, height: oh, clientWidth, clientHeight, radius: FRAME_RADIUS })
      .catch((e) => setAgentLog((l) => [...l, `open failed: ${e}`]));
    // Re-sync a beat later so the webview lands on the SETTLED rect (the agent
    // prompt bar toggling can shift the pane by a row).
    setTimeout(syncBounds, 120);
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

  // THE HAND-OFF. Flip to agent: the right prompt pane opens + the webview
  // shrinks to make room. Flip to human: pane closes, webview reclaims width.
  // Re-sync bounds a beat after the toggle so the webview resizes with it.
  useEffect(() => { syncBounds(); const t = setTimeout(syncBounds, 60); return () => clearTimeout(t); }, [driver]); // eslint-disable-line react-hooks/exhaustive-deps

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
    // Normal in-flow layout: fill the parent content area (App.tsx wraps every
    // screen in a flex:1 / minHeight:0 column with 28x32 padding). height:100%
    // + minHeight:0 lets this fill that box; overflow:hidden keeps paneRef from
    // spilling. Do NOT use position:absolute/inset:0 here — that escapes the
    // padded content wrapper and overlays the entire app (sidebar + logo).
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0, gap: 8, overflow: "hidden" }}>
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

      {/* PAGE (left) + AGENT PANE (right). The native webview tracks paneRef,
          which is the LEFT region; syncBounds shrinks it by AGENT_PANE_W when
          the agent pane is open, so the pane sits BESIDE the page. */}
      <div style={{ flex: 1, minHeight: 0, minWidth: 0, display: "flex", gap: 10, overflow: "hidden" }}>
        {/* THE PAGE AREA — the native webview is positioned over THIS div.
            The OS webview fills paneRef's FULL border-box (syncBounds sends the
            exact rect). So paneRef itself carries NO border — an opaque native
            layer would just cover it. Instead the accented rounded frame is a
            pointer-events:none OVERLAY rendered on top (below), whose rounded
            corners mask the webview's square corners. Both are driven by the
            same measured rect, so they stay locked at every window size. */}
        <div ref={paneRef} style={{
          flex: 1, minWidth: 0, minHeight: 0, height: "100%", position: "relative",
          borderRadius: "var(--radius-card)", background: "var(--bg)", overflow: "hidden",
        }}>
          <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", color: "var(--text-faint)", fontSize: 14, pointerEvents: "none" }}>
            Click the tab to type a URL, or search.
          </div>
          {/* ROUNDED FRAME OVERLAY — sits ON TOP of the native webview. Just an
              outline + rounded corners; transparent fill, no pointer capture,
              so clicks pass through to the page. Accent-highlighted in agent
              mode, subtle line otherwise. This is the trick that gives the
              square-cornered OS webview rounded corners: the arcs cover the
              webview's corners while the center stays click-through. */}
          <div style={{
            position: "absolute", inset: 0, pointerEvents: "none", borderRadius: "var(--radius-card)",
            border: driver === "agent"
              ? "2px solid var(--accent)"
              : "var(--border-width) solid var(--line)",
          }} />
        </div>

        {/* AGENT PANE (right) — appears on hand-off. You prompt here; the agent
            acts in the page to the left while you watch. */}
        {driver === "agent" && (
          <div style={{
            width: AGENT_PANE_W, flexShrink: 0, display: "flex", flexDirection: "column", gap: 8,
            border: "var(--border-width) solid var(--accent)", borderRadius: "var(--radius-card)",
            padding: 12, background: "var(--surface)", minHeight: 0,
          }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <Icon name="sparkles" size={15} />
              <span style={{ fontSize: 13, fontWeight: 700 }}>Agent</span>
              <span style={{ marginLeft: "auto", fontSize: 11, color: "var(--text-faint)" }}>acting in the page →</span>
            </div>
            <div style={{ fontSize: 12, color: "var(--text-muted)" }}>
              Tell the agent what to do on this page. You’re watching — hit “You” anytime to take back control.
            </div>

            {/* Conversation log fills the pane. */}
            <div style={{ flex: 1, minHeight: 0, overflow: "auto", display: "flex", flexDirection: "column", gap: 8, paddingRight: 2 }}>
              {agentLog.length === 0 ? (
                <div style={{ fontSize: 12, color: "var(--text-faint)", marginTop: 4 }}>
                  Try: “click the sign-in button”, “summarize this page”, “scroll down”.
                </div>
              ) : agentLog.map((l, i) => {
                const isUser = l.startsWith("▸ ");
                return (
                  <div key={i} style={{
                    alignSelf: isUser ? "flex-end" : "flex-start", maxWidth: "92%",
                    fontSize: 12.5, lineHeight: 1.4, padding: "7px 10px", borderRadius: 10,
                    background: isUser ? "var(--accent)" : "var(--bg)",
                    color: isUser ? "#fff" : "var(--text)",
                    border: isUser ? "none" : "var(--border-width) solid var(--line)",
                    whiteSpace: "pre-wrap", wordBreak: "break-word",
                  }}>{isUser ? l.slice(2) : l}</div>
                );
              })}
              {agentBusy && <div style={{ fontSize: 12, color: "var(--text-faint)" }}>working…</div>}
            </div>

            {/* Prompt input pinned to the bottom of the pane. */}
            <div style={{ display: "flex", gap: 6 }}>
              <div style={{ flex: 1 }}>
                <Input value={agentPrompt} onChange={(e: any) => setAgentPrompt(e.target.value)}
                  onKeyDown={(e: any) => { if (e.key === "Enter") runAgent(); }}
                  placeholder="Tell the agent…" />
              </div>
              <button onClick={runAgent} disabled={agentBusy || !agentPrompt.trim()}
                style={{ fontSize: 13, padding: "8px 14px", borderRadius: "var(--radius-control)", border: "none", background: "var(--accent)", color: "#fff", cursor: "pointer", opacity: agentBusy || !agentPrompt.trim() ? 0.5 : 1 }}>
                {agentBusy ? "…" : "Act"}
              </button>
            </div>
          </div>
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
