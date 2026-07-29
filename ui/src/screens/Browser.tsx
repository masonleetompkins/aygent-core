// AYGENT — Browser screen (BROWSER-ARCH Slice 1 + inline-tab addressing).
// ONE row of tabs. The ACTIVE tab IS the address field: click it (when already
// active) to edit its URL inline; type + Enter to navigate. Inactive tabs show
// their page title — click to switch. + adds a tab, x closes. Plain text (no
// domain) routes to a Google search. The rendered page below is a settled
// screenshot for now (Slice 2 = live screencast; Slice 3 = click-into-it).
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Pill } from "../components/ui";
import { Icon } from "../components/Icon";

type NavResult = { screenshot: string; url: string; title: string };
type Tab = {
  id: number;
  addr: string;                 // URL text for this tab (also the edit buffer)
  page: NavResult | null;
  loading: boolean;
  err: string | null;
  editing: boolean;             // active tab in URL-edit mode
};

let TAB_SEQ = 1;
const newTab = (): Tab => ({ id: TAB_SEQ++, addr: "", page: null, loading: false, err: null, editing: true });

export function Browser() {
  const [installed] = useInstalled();
  const [tabs, setTabs] = useState<Tab[]>([newTab()]);
  const [activeId, setActiveId] = useState<number>(() => tabs[0].id);
  const editRef = useRef<HTMLInputElement>(null);
  // Slice 2: the live frame streamed from Chromium (base64 JPEG data URL).
  const [frame, setFrame] = useState<string | null>(null);
  // Slice 3: the frame element, so clicks/keys map to page coords + forward in.
  const frameRef = useRef<HTMLImageElement>(null);
  // Slice 5: shared-control state (who's driving + hand-off note).
  const [control, setControl] = useState<{ driver: string; note: string; agent_active: boolean }>(
    { driver: "idle", note: "", agent_active: false }
  );
  const interactive = control.driver === "human";

  const active = tabs.find((t) => t.id === activeId) ?? tabs[0];

  // Slice 2: start the live view + subscribe to streamed frames when the tab
  // mounts (only once the browser is installed).
  useEffect(() => {
    if (installed !== true) return;
    let un: undefined | (() => void);
    let alive = true;
    let unCtl: undefined | (() => void);
    (async () => {
      un = await listen<{ data: string }>("browser:frame", (e) => {
        if (alive && e.payload?.data) setFrame(e.payload.data);
      });
      unCtl = await listen<any>("browser:control", (e) => {
        if (alive && e.payload) setControl(e.payload);
      });
      invoke("browser_start_view").catch(() => {});
      invoke<any>("browser_control_status").then((c) => alive && c && setControl(c)).catch(() => {});
    })();
    return () => { alive = false; un?.(); unCtl?.(); };
  }, [installed]);

  function patch(id: number, p: Partial<Tab>) {
    setTabs((ts) => ts.map((t) => (t.id === id ? { ...t, ...p } : t)));
  }

  function addTab() {
    const t = newTab();
    setTabs((ts) => [...ts, t]);
    setActiveId(t.id);
  }

  function closeTab(id: number) {
    setTabs((ts) => {
      const next = ts.filter((t) => t.id !== id);
      if (next.length === 0) { const t = newTab(); setActiveId(t.id); return [t]; }
      if (id === activeId) setActiveId(next[next.length - 1].id);
      return next;
    });
  }

  // --- Slice 3: forward human input on the live frame into Chromium ---------
  // Map a pointer event to normalized 0..1 coords over the frame image.
  function normCoords(e: React.PointerEvent | React.WheelEvent | React.MouseEvent) {
    const el = frameRef.current;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    const fx = (e.clientX - r.left) / r.width;
    const fy = (e.clientY - r.top) / r.height;
    return { fx: Math.min(Math.max(fx, 0), 1), fy: Math.min(Math.max(fy, 0), 1) };
  }
  function onFrameClick(e: React.MouseEvent) {
    const c = normCoords(e);
    if (!c) return;
    // Slice 5: interacting = the human takes the wheel (preempts the agent).
    if (control.driver !== "human") invoke("browser_take_wheel").catch(() => {});
    frameRef.current?.focus();
    invoke("browser_click", { fx: c.fx, fy: c.fy, button: "left" }).catch(() => {});
  }
  function onFrameWheel(e: React.WheelEvent) {
    const c = normCoords(e);
    if (!c) return;
    invoke("browser_scroll", { fx: c.fx, fy: c.fy, dx: e.deltaX, dy: e.deltaY }).catch(() => {});
  }
  function onFrameKeyDown(e: React.KeyboardEvent) {
    // Special keys go via browser_key; printable chars via browser_type.
    const special = ["Enter", "Backspace", "Tab", "Escape", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Delete"];
    if (special.includes(e.key)) {
      e.preventDefault();
      invoke("browser_key", { key: e.key }).catch(() => {});
    } else if (e.key.length === 1 && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      invoke("browser_type", { text: e.key }).catch(() => {});
    }
  }

  // Click a tab: switch to it. If it's ALREADY active, enter URL-edit mode.
  function clickTab(id: number) {
    if (id === activeId) { patch(id, { editing: true }); setTimeout(() => editRef.current?.select(), 0); }
    else setActiveId(id);
  }

  async function go(id: number) {
    const tab = tabs.find((t) => t.id === id);
    if (!tab) return;
    const url = tab.addr.trim();
    if (!url) { patch(id, { editing: false }); return; }
    patch(id, { loading: true, err: null, editing: false });
    try {
      const res = await invoke<NavResult>("browser_navigate", { url });
      patch(id, { page: res, addr: res.url, loading: false });
    } catch (e) {
      patch(id, { err: String(e), loading: false });
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

  const label = (t: Tab) => t.page?.title || t.page?.url || "New Tab";

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0, gap: 10 }}>
      {/* ONE ROW: tabs. Active tab is editable inline = the address field. */}
      <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
        {tabs.map((t) => {
          const on = t.id === activeId;
          const editing = on && t.editing;
          return (
            <div
              key={t.id}
              onClick={() => clickTab(t.id)}
              style={{
                display: "flex", alignItems: "center", gap: 7,
                width: editing ? 340 : "auto", maxWidth: editing ? 340 : 240,
                padding: "7px 11px", cursor: on ? "text" : "pointer",
                borderRadius: "var(--radius-control)",
                border: `var(--border-width) solid ${on ? "var(--accent)" : "var(--line)"}`,
                background: on ? "var(--surface)" : "var(--bg)",
                color: on ? "var(--text)" : "var(--text-muted)",
                boxShadow: on ? "var(--elevation)" : "none",
                transition: "width 0.12s ease",
              }}
            >
              {t.loading
                ? <span style={{ width: 15, fontSize: 12, textAlign: "center" }}>…</span>
                : <Icon name="globe" size={14} />}
              {editing ? (
                <input
                  ref={editRef}
                  autoFocus
                  value={t.addr}
                  onChange={(e) => patch(t.id, { addr: e.target.value })}
                  onKeyDown={(e) => { if (e.key === "Enter") go(t.id); if (e.key === "Escape") patch(t.id, { editing: false }); }}
                  onBlur={() => patch(t.id, { editing: false })}
                  onClick={(e) => e.stopPropagation()}
                  placeholder="Enter a URL or search…"
                  style={{
                    flex: 1, minWidth: 0, border: "none", outline: "none",
                    background: "transparent", color: "var(--text)", fontSize: 13,
                    fontFamily: "inherit",
                  }}
                />
              ) : (
                <span style={{ flex: 1, fontSize: 13, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                  {label(t)}
                </span>
              )}
              <span
                onClick={(e) => { e.stopPropagation(); closeTab(t.id); }}
                style={{ fontSize: 15, lineHeight: 1, opacity: 0.55, paddingLeft: 2 }}
                title="Close tab"
              >×</span>
            </div>
          );
        })}
        <button
          onClick={addTab}
          title="New tab"
          style={{
            display: "flex", alignItems: "center", justifyContent: "center",
            width: 32, height: 32, borderRadius: "var(--radius-control)",
            border: "var(--border-width) dashed var(--line)", background: "transparent",
            color: "var(--text-muted)", cursor: "pointer", fontSize: 18, lineHeight: 1, flexShrink: 0,
          }}
        >+</button>
      </div>

      {active.err && <Pill tone="danger">✗ {active.err}</Pill>}

      {/* SLICE 5 — WHO'S DRIVING HUD + hand-off. */}
      <div style={{ display: "flex", alignItems: "center", gap: 10, fontSize: 13 }}>
        <span style={{
          display: "inline-flex", alignItems: "center", gap: 6, padding: "3px 10px",
          borderRadius: "var(--radius-pill)", fontWeight: 700,
          border: "var(--border-width) solid var(--line)",
          color: control.driver === "human" ? "var(--accent)" : control.driver === "agent" ? "var(--ok)" : "var(--text-faint)",
        }}>
          <span style={{
            width: 8, height: 8, borderRadius: "50%",
            background: control.driver === "human" ? "var(--accent)" : control.driver === "agent" ? "var(--ok)" : "var(--text-faint)",
          }} />
          {control.driver === "human" ? "You're driving" : control.driver === "agent" ? "Agent driving" : "Idle"}
        </span>
        {control.driver === "human" && (
          <button onClick={() => invoke("browser_release_wheel").catch(() => {})}
            style={{ fontSize: 12, padding: "3px 10px", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)", background: "transparent", color: "var(--text-muted)", cursor: "pointer" }}>
            Give control back to agent
          </button>
        )}
        {control.note && (
          <span style={{ color: "var(--accent)", fontWeight: 600 }}>⚠ {control.note}</span>
        )}
      </div>

      {/* LIVE PAGE (Slice 2): the streamed frame updates in real time. Slice 3
          will forward clicks/keys on this surface into Chromium. */}
      <div style={{
        flex: 1, minHeight: 0, border: "var(--border-width) solid var(--line)",
        borderRadius: "var(--radius-card)", background: "var(--bg)", overflow: "auto",
        display: "flex", flexDirection: "column", position: "relative",
      }}>
        {frame ? (
          <img
            ref={frameRef}
            src={frame}
            alt={active.page?.title || active.page?.url || "page"}
            draggable={false}
            tabIndex={0}
            onClick={onFrameClick}
            onWheel={onFrameWheel}
            onKeyDown={onFrameKeyDown}
            style={{
              width: "100%", display: "block", cursor: "pointer", outline: "none",
              boxShadow: interactive ? "inset 0 0 0 2px var(--accent)" : "none",
            }}
          />
        ) : (
          <div style={{
            flex: 1, display: "flex", alignItems: "center", justifyContent: "center",
            color: "var(--text-faint)", fontSize: 14,
          }}>
            {active.loading ? "Loading the page…" : "Click the tab to type a URL, or search."}
          </div>
        )}
      </div>
    </div>
  );
}

// Small hook: is the in-app browser provisioned?
function useInstalled(): [boolean | null] {
  const [installed, setInstalled] = useState<boolean | null>(null);
  useEffect(() => {
    invoke<{ installed: boolean }>("browser_status")
      .then((s) => setInstalled(s.installed))
      .catch(() => setInstalled(false));
  }, []);
  return [installed];
}
