// AYGENT — Browser screen (BROWSER-ARCH Slice 1 + tabs).
// A real browser inside AYGENT: multiple tabs, each with its own address bar +
// rendered page (a settled screenshot for now; Slice 2 = live screencast, Slice
// 3 = click-into-it). The selected tab's URL IS the address field — editing the
// address bar edits that tab; each tab shows its URL (or title) as its label.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Input, Pill } from "../components/ui";
import { Icon } from "../components/Icon";

type Tab = {
  id: number;
  addr: string;                 // what's in the address bar for this tab
  page: NavResult | null;       // the rendered page (screenshot + url + title)
  loading: boolean;
  err: string | null;
};
type NavResult = { screenshot: string; url: string; title: string };

let TAB_SEQ = 1;
function newTab(): Tab {
  return { id: TAB_SEQ++, addr: "", page: null, loading: false, err: null };
}

export function Browser() {
  const [installed] = useInstalled();
  const [tabs, setTabs] = useState<Tab[]>([newTab()]);
  const [activeId, setActiveId] = useState<number>(() => tabs[0].id);

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

  async function go(id: number, target?: string) {
    const tab = tabs.find((t) => t.id === id);
    if (!tab) return;
    const url = (target ?? tab.addr).trim();
    if (!url) return;
    patch(id, { loading: true, err: null });
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

  const tabLabel = (t: Tab) =>
    t.page?.title || t.page?.url || (t.addr.trim() ? t.addr : "New Tab");

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0, gap: 10 }}>
      {/* TAB BAR — each tab labeled by its page title/url; + adds a new one. */}
      <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
        {tabs.map((t) => {
          const on = t.id === activeId;
          return (
            <div
              key={t.id}
              onClick={() => setActiveId(t.id)}
              style={{
                display: "flex", alignItems: "center", gap: 6, maxWidth: 220,
                padding: "6px 10px", cursor: "pointer",
                borderRadius: "var(--radius-control)",
                border: `var(--border-width) solid ${on ? "var(--accent)" : "var(--line)"}`,
                background: on ? "var(--surface)" : "var(--bg)",
                color: on ? "var(--text)" : "var(--text-muted)",
                boxShadow: on ? "var(--elevation)" : "none",
              }}
            >
              {t.loading
                ? <span style={{ width: 14, fontSize: 11 }}>…</span>
                : <Icon name="globe" size={13} />}
              <span style={{ fontSize: 12, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                {tabLabel(t)}
              </span>
              <span
                onClick={(e) => { e.stopPropagation(); closeTab(t.id); }}
                style={{ fontSize: 14, lineHeight: 1, opacity: 0.6, paddingLeft: 2 }}
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
            width: 30, height: 30, borderRadius: "var(--radius-control)",
            border: "var(--border-width) dashed var(--line)", background: "transparent",
            color: "var(--text-muted)", cursor: "pointer", fontSize: 18, lineHeight: 1,
          }}
        >+</button>
      </div>

      {/* ADDRESS BAR — edits the ACTIVE tab. */}
      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <div style={{ flex: 1 }}>
          <Input
            value={active.addr}
            onChange={(e: any) => patch(active.id, { addr: e.target.value })}
            onKeyDown={(e: any) => { if (e.key === "Enter") go(active.id); }}
            placeholder="Enter a URL or search…"
          />
        </div>
        <Button onClick={() => go(active.id)} disabled={active.loading || !active.addr.trim()}>
          {active.loading ? "Loading…" : "Go"}
        </Button>
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
            {active.loading ? "Loading the page…" : "Enter a URL above to load a page."}
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
