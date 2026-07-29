// AYGENT — Browser screen (BROWSER-ARCH Slice 1 + inline-tab addressing).
// ONE row of tabs. The ACTIVE tab IS the address field: click it (when already
// active) to edit its URL inline; type + Enter to navigate. Inactive tabs show
// their page title — click to switch. + adds a tab, x closes. Plain text (no
// domain) routes to a Google search. The rendered page below is a settled
// screenshot for now (Slice 2 = live screencast; Slice 3 = click-into-it).
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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

  const active = tabs.find((t) => t.id === activeId) ?? tabs[0];

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

      {/* RENDERED PAGE for the active tab. */}
      <div style={{
        flex: 1, minHeight: 0, border: "var(--border-width) solid var(--line)",
        borderRadius: "var(--radius-card)", background: "var(--bg)", overflow: "auto",
        display: "flex", flexDirection: "column",
      }}>
        {active.page ? (
          <img
            src={active.page.screenshot}
            alt={active.page.title || active.page.url}
            style={{ width: "100%", display: "block" }}
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
