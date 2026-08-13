// AYGENT — Browser screen: ONE Chromium (Chrome-for-Testing via CDP).
//
// BROWSER REBUILD (Mason 2026-08-06): the WKWebView layer is GONE. What you
// see is the live screencast of the SAME Chromium page the agent acts on —
// one engine, one page, zero desync. Human input (clicks/keys/scroll)
// forwards into the page via CDP Input.dispatch* (isTrusted events).
//
// Layout: tab-ish top bar (address + nav + You/Agent toggle) over the frame
// mirror. Agent mode opens the right-hand prompt pane (plan checklist +
// permission prompts) beside the page — unchanged protocol from the old
// screen (browser:agent-plan / -step / -done / browser:permission-request).

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button } from "../components/ui";

type HistEntry = { url: string; title: string; ts: number };

const AGENT_PANE_W = 320;

type Run = {
  id: string; prompt: string; steps: string[]; stepsDone: number;
  planDone: boolean; action: string; summary: string; stopped?: boolean;
};

export function Browser() {
  const [installed, setInstalled] = useState<boolean | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installPct, setInstallPct] = useState(0);

  // Page state — the CDP page is the single source of truth.
  const [addr, setAddr] = useState("");
  const [pageUrl, setPageUrl] = useState("");
  const [pageTitle, setPageTitle] = useState("");
  const [frame, setFrame] = useState<string | null>(null); // base64 JPEG
  const [loadingPage, setLoadingPage] = useState(false);

  // Driver: who's at the wheel.
  const [driver, setDriver] = useState<"human" | "agent">("human");
  const [blockedNote, setBlockedNote] = useState("");

  // Panels + agent runs + permissions.
  const [panel, setPanel] = useState<null | "history" | "downloads">(null);
  const [history, setHistory] = useState<HistEntry[]>([]);
  type DlEntry = { name: string; size: number; mtime: number };
  const [downloads, setDownloads] = useState<DlEntry[]>([]);
  const [permReq, setPermReq] = useState<null | { id: string; action: string; detail: string }>(null);
  const [runs, setRuns] = useState<Run[]>([]);
  const [agentPrompt, setAgentPrompt] = useState("");
  const [agentBusy, setAgentBusy] = useState(false);

  const frameRef = useRef<HTMLImageElement>(null);
  const paneRef = useRef<HTMLDivElement>(null);
  const runsScrollRef = useRef<HTMLDivElement>(null);

  // ---- install gate --------------------------------------------------------
  useEffect(() => {
    invoke<{ installed: boolean }>("browser_status")
      .then((s) => setInstalled(s.installed))
      .catch(() => setInstalled(false));
  }, []);

  async function install() {
    setInstalling(true); setInstallPct(0);
    const ch = `browser-install-${Date.now()}`;
    const un = await listen<{ got: number; total: number; done?: boolean }>(ch, (e) => {
      const p = e.payload;
      if (p.total > 0) setInstallPct(Math.round((p.got / p.total) * 100));
      if (p.done) setInstallPct(100);
    });
    try {
      await invoke("browser_install", { channel: ch });
      setInstalled(true);
    } catch (e) { console.error("browser_install", e); }
    finally { un(); setInstalling(false); }
  }

  // ---- screencast frames ---------------------------------------------------
  useEffect(() => {
    if (installed !== true) return;
    let un: (() => void) | undefined;
    (async () => {
      un = await listen<{ data: string }>("browser:frame", (e) => {
        setFrame(e.payload.data);
      });
      // Wake the session so frames start (idempotent).
      invoke("browser_start_view").catch(() => {});
    })();
    return () => { if (un) un(); };
  }, [installed]);

  // ---- FIT-TO-PANE: keep the Chromium viewport aspect ratio == this pane's ----
  // The screencast captures the browser's own viewport; if the pane is a
  // different shape, objectFit:contain letterboxes (the "fills width, not
  // height" empty band). A ResizeObserver pushes the pane's CSS px size to
  // browser_set_viewport (debounced) so the captured frame matches the box and
  // the mirror fills it edge to edge — corners still rounded, meeting the line.
  // We keep objectFit:contain (not cover) so click coord mapping stays exact;
  // once the viewport matches, contain has zero letterbox anyway. `driver` is in
  // the deps because toggling You/Agent opens the side pane and reshapes the box.
  useEffect(() => {
    if (installed !== true) return;
    const el = paneRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    let t: ReturnType<typeof setTimeout> | undefined;
    const push = () => {
      const r = el.getBoundingClientRect();
      if (r.width < 8 || r.height < 8) return;
      invoke("browser_set_viewport", { width: r.width, height: r.height, dpr: window.devicePixelRatio || 1 }).catch(() => {});
    };
    const ro = new ResizeObserver(() => {
      if (t) clearTimeout(t);
      t = setTimeout(push, 120); // debounce drag-resize / animated layout
    });
    ro.observe(el);
    push(); // initial sync on mount
    return () => { if (t) clearTimeout(t); ro.disconnect(); };
  }, [installed, driver]);

  // Poll url/title so the address bar tracks agent navigations too.
  useEffect(() => {
    if (installed !== true) return;
    const t = setInterval(async () => {
      try {
        const info = await invoke<{ url: string; title: string }>("browser_page_info");
        if (info.url && info.url !== "about:blank") {
          setPageUrl(info.url); setPageTitle(info.title || "");
        }
      } catch { /* session not up yet */ }
    }, 1500);
    return () => clearInterval(t);
  }, [installed]);

  // ---- control events ------------------------------------------------------
  useEffect(() => {
    const unl: Array<() => void> = [];
    (async () => {
      unl.push(await listen<{ driver: string; note: string }>("browser:control", (ev) => {
        if (ev.payload.driver === "human" || ev.payload.driver === "agent") {
          setDriver(ev.payload.driver as "human" | "agent");
        }
        setBlockedNote(ev.payload.note || "");
      }));
      unl.push(await listen<{ id: string; action: string; detail: string }>("browser:permission-request", (ev) => {
        setPermReq(ev.payload);
      }));
    })();
    return () => unl.forEach((u) => u());
  }, []);

  function answerPerm(answer: "allow" | "deny" | "take") {
    if (!permReq) return;
    invoke("browser_permission_answer", { id: permReq.id, answer, grantHost: permReq.detail }).catch(() => {});
    if (answer === "take") setDriver("human");
    setPermReq(null); setBlockedNote("");
  }

  // ---- navigation ----------------------------------------------------------
  async function go(url: string) {
    if (!url.trim()) return;
    setLoadingPage(true);
    try {
      const r = await invoke<{ url: string; title: string }>("browser_navigate", { url });
      setPageUrl(r.url); setPageTitle(r.title || ""); setAddr("");
    } catch (e) { console.error("browser_navigate", e); }
    finally { setLoadingPage(false); }
  }

  async function navHistory(action: "back" | "forward" | "reload") {
    try { await invoke("browser_history_nav", { action }); }
    catch (e) { console.error("browser_history_nav", e); }
  }

  // ---- human input forwarding (CDP Input.dispatch*) ------------------------
  function frameCoords(e: React.MouseEvent): { fx: number; fy: number } | null {
    const el = frameRef.current;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return null;
    return { fx: (e.clientX - r.left) / r.width, fy: (e.clientY - r.top) / r.height };
  }

  function onFrameClick(e: React.MouseEvent) {
    if (driver !== "human") return;
    const c = frameCoords(e);
    if (!c) return;
    invoke("browser_click", { fx: c.fx, fy: c.fy, button: "left" }).catch(() => {});
  }

  function onFrameWheel(e: React.WheelEvent) {
    if (driver !== "human") return;
    const c = frameCoords(e as unknown as React.MouseEvent);
    if (!c) return;
    invoke("browser_scroll", { fx: c.fx, fy: c.fy, dx: e.deltaX, dy: e.deltaY }).catch(() => {});
  }

  function onFrameKey(e: React.KeyboardEvent) {
    if (driver !== "human") return;
    const special = ["Enter", "Backspace", "Tab", "Escape", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Delete", "Home", "End", "PageUp", "PageDown"];
    if (special.includes(e.key)) {
      e.preventDefault();
      invoke("browser_key", { key: e.key }).catch(() => {});
    } else if (e.key.length === 1 && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      invoke("browser_type", { text: e.key }).catch(() => {});
    }
  }

  // ---- agent runs (plan checklist protocol -- unchanged wire format) --------
  async function runAgent() {
    const task = agentPrompt.trim();
    if (!task || agentBusy) return;
    setAgentBusy(true);
    setAgentPrompt("");
    const runId = `agent-run-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
    setRuns((rs) => [...rs, { id: runId, prompt: task, steps: [], stepsDone: 0, planDone: false, action: "", summary: "" }]);
    const channel = runId;
    const patchRun = (id: string, patch: Partial<Run>) =>
      setRuns((rs) => rs.map((r) => (r.id === id ? { ...r, ...patch } : r)));
    let un: (() => void) | undefined;
    try {
      un = await listen<Record<string, unknown>>(channel, (ev) => {
        const p = ev.payload as any;
        if (!p || typeof p !== "object") return;
        if (p.kind === "plan" && Array.isArray(p.steps)) {
          patchRun(runId, { steps: p.steps as string[], stepsDone: 0, planDone: false, action: "" });
        } else if (p.kind === "step") {
          // Rust sends {index, done, total}; legacy JS expected {step}. Support both.
          const doneVal = typeof p.done === "number" ? p.done : typeof p.step === "number" ? p.step : typeof p.index === "number" ? p.index + 1 : 0;
          if (typeof doneVal === "number" && doneVal > 0) patchRun(runId, { stepsDone: doneVal, action: "" });
        } else if (p.kind === "action") {
          const txt = typeof p.text === "string" ? p.text : typeof p.label === "string" ? p.label : "";
          if (txt) patchRun(runId, { action: txt });
        } else if (p.kind === "stopped") {
          patchRun(runId, { planDone: true, action: "", stopped: true, summary: String(p.reason || p.summary || "stopped") });
        } else if (p.kind === "done") {
          // Rust sends {total, summary} and for stops {stopped:true, reason/summary}. Handle both.
          const isStopped = !!p.stopped || !!p.reason;
          const s = String(p.summary || p.reason || "");
          if (isStopped) patchRun(runId, { planDone: true, action: "", stopped: true, summary: s });
          else patchRun(runId, { planDone: true, action: "", summary: s });
          // Ensure checklist ticks to total when done without per-step events
          if (typeof p.total === "number" && p.total > 0) {
            patchRun(runId, { stepsDone: p.total });
          }
        }
      });
      const primed = task;
      await invoke<string>("agent_run", { prompt: primed, folder: null, channel });
      patchRun(runId, { planDone: true, action: "" });
    } catch (e) {
      patchRun(runId, { planDone: true, action: "", stopped: true, summary: String(e) });
    } finally {
      if (un) un();
      setAgentBusy(false);
      setTimeout(() => runsScrollRef.current?.scrollTo({ top: 1e9 }), 50);
    }
  }

  // ---- panels ----------------------------------------------------------------
  async function openHistory() {
    if (panel === "history") { setPanel(null); return; }
    try {
      const list = await invoke<HistEntry[]>("browser_history_list");
      setHistory(Array.isArray(list) ? list : []);
    } catch { setHistory([]); }
    setPanel("history");
  }
  async function openDownloads() {
    if (panel === "downloads") { setPanel(null); return; }
    try {
      const list = await invoke<DlEntry[]>("browser_downloads_list");
      setDownloads(Array.isArray(list) ? list : []);
    } catch { setDownloads([]); }
    setPanel("downloads");
  }
  const fmtWhen = (tsSecs: number) => {
    const diff = Math.max(0, Math.floor(Date.now() / 1000 - tsSecs));
    if (diff < 60) return "just now";
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    try { return new Date(tsSecs * 1000).toLocaleDateString(); } catch { return ""; }
  };
  const fmtSize = (n: number) =>
    n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : n >= 1e3 ? `${(n / 1e3).toFixed(0)} KB` : `${n} B`;

  // ---- install gate render -----------------------------------------------------
  if (installed === false) {
    return (
      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", height: "100%", gap: 14 }}>
        <h2 style={{ fontSize: 20, fontWeight: 800, margin: 0 }}>AYGENT Browser</h2>
        <p style={{ color: "var(--text-muted)", fontSize: 14, maxWidth: 420, textAlign: "center", margin: 0 }}>
          A real Chromium your agents can drive — downloaded on demand so the app stays small.
          One download (~180 MB), then the browser just works.
        </p>
        {installing ? (
          <div style={{ width: 280 }}>
            <div style={{ height: 8, borderRadius: 4, background: "var(--surface)", border: "var(--border-width) solid var(--line)", overflow: "hidden" }}>
              <div style={{ height: "100%", width: `${installPct}%`, background: "var(--accent)", transition: "width .3s ease" }} />
            </div>
            <p style={{ color: "var(--text-muted)", fontSize: 12, textAlign: "center", marginTop: 6 }}>{installPct}%</p>
          </div>
        ) : (
          <Button onClick={() => void install()}>Download & enable</Button>
        )}
      </div>
    );
  }
  if (installed === null) return null;

  // ---- main render ---------------------------------------------------------------
  const agentMode = driver === "agent";
  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0, gap: 10 }}>
      {/* top chrome: nav + address + toggle */}
      <div style={{ display: "flex", alignItems: "center", gap: 8, flexShrink: 0 }}>
        <button onClick={() => void navHistory("back")} title="Back" style={navBtnStyle()}>&#8592;</button>
        <button onClick={() => void navHistory("forward")} title="Forward" style={navBtnStyle()}>&#8594;</button>
        <button onClick={() => void navHistory("reload")} title="Reload" style={navBtnStyle()}>&#8635;</button>
        <input
          value={addr}
          placeholder={pageTitle || pageUrl || "Search or enter address"}
          onChange={(e) => setAddr(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") void go(addr); }}
          style={{
            flex: 1, minWidth: 0, height: 32, padding: "0 12px",
            borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)",
            background: "var(--surface)", color: "var(--text)", fontSize: 13, outline: "none",
          }}
        />
        <button onClick={() => void openHistory()} title="History" style={navBtnStyle(panel === "history")}>&#128337;</button>
        <button onClick={() => void openDownloads()} title="Downloads" style={navBtnStyle(panel === "downloads")}>&#8595;</button>
        {/* You / Agent toggle */}
        <div style={{ display: "flex", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)", overflow: "hidden", flexShrink: 0 }}>
          <button onClick={() => { setDriver("human"); invoke("browser_take_wheel").catch(() => {}); }}
            style={toggleStyle(!agentMode)}>You</button>
          <button onClick={() => setDriver("agent")} style={toggleStyle(agentMode)}>Agent</button>
        </div>
      </div>

      {/* page area: frame mirror + optional agent pane */}
      <div style={{ display: "flex", flex: 1, minHeight: 0, gap: 10, position: "relative" }}>
        <div
          ref={paneRef}
          tabIndex={0}
          onKeyDown={onFrameKey}
          style={{
            flex: 1, minWidth: 0, position: "relative", overflow: "hidden",
            border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
            background: "var(--surface)", outline: "none",
          }}
        >
          {frame ? (
            <img
              ref={frameRef}
              src={frame} /* payload is ALREADY a full data: URL (Rust prefixes it) */
              onClick={onFrameClick}
              onWheel={onFrameWheel}
              alt=""
              draggable={false}
              style={{ width: "100%", height: "100%", objectFit: "contain", display: "block", cursor: agentMode ? "default" : "pointer", userSelect: "none" }}
            />
          ) : (
            <div style={{ display: "flex", alignItems: "center", justifyContent: "center", height: "100%", color: "var(--text-muted)", fontSize: 14 }}>
              {loadingPage ? "Loading…" : "Enter an address above to start browsing."}
            </div>
          )}

          {/* history / downloads panel overlays the page area */}
          {panel && (
            <div style={{
              position: "absolute", top: 0, right: 0, bottom: 0, width: 320,
              background: "var(--surface)", borderLeft: "var(--border-width) solid var(--line)",
              boxShadow: "var(--elevation)", overflowY: "auto", padding: 12,
            }}>
              <div style={{ display: "flex", alignItems: "center", marginBottom: 8 }}>
                <strong style={{ fontSize: 14 }}>{panel === "history" ? "History" : "Downloads"}</strong>
                <span style={{ flex: 1 }} />
                {panel === "history" && (
                  <Button variant="secondary" onClick={() => { invoke("browser_history_clear").then(() => setHistory([])).catch(() => {}); }}>Clear</Button>
                )}
                <button onClick={() => setPanel(null)} style={{ marginLeft: 8, border: "none", background: "none", color: "var(--text-muted)", cursor: "pointer", fontSize: 16 }}>&#215;</button>
              </div>
              {panel === "history" && history.map((h, i) => (
                <div key={i} onClick={() => { setPanel(null); void go(h.url); }}
                  style={{ padding: "8px 6px", borderRadius: 6, cursor: "pointer", fontSize: 13 }}>
                  <div style={{ fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{h.title || h.url}</div>
                  <div style={{ color: "var(--text-faint)", fontSize: 11, display: "flex", gap: 6 }}>
                    <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1 }}>{h.url}</span>
                    <span style={{ flexShrink: 0 }}>{fmtWhen(h.ts)}</span>
                  </div>
                </div>
              ))}
              {panel === "history" && history.length === 0 && <p style={{ color: "var(--text-muted)", fontSize: 13 }}>Nothing yet.</p>}
              {panel === "downloads" && downloads.map((d, i) => (
                <div key={i} style={{ padding: "8px 6px", fontSize: 13 }}>
                  <div style={{ fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{d.name}</div>
                  <div style={{ color: "var(--text-faint)", fontSize: 11 }}>{fmtSize(d.size)} · {fmtWhen(d.mtime)}</div>
                </div>
              ))}
              {panel === "downloads" && downloads.length === 0 && <p style={{ color: "var(--text-muted)", fontSize: 13 }}>No downloads yet.</p>}
            </div>
          )}
        </div>

        {/* agent pane */}
        {agentMode && (
          <div style={{
            width: AGENT_PANE_W, flexShrink: 0, display: "flex", flexDirection: "column",
            border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
            background: "var(--surface)", padding: 12, gap: 10,
          }}>
            <div style={{ display: "flex", alignItems: "baseline", gap: 8 }}>
              <strong style={{ fontSize: 14 }}>&#10024; Agent</strong>
              <span style={{ marginLeft: "auto", fontSize: 11, color: "var(--text-faint)" }}>acting in the page &#8594;</span>
            </div>
            <p style={{ fontSize: 12, color: "var(--text-muted)", margin: 0 }}>
              Tell the agent what to do on this page. You&#39;re watching &#8212; hit &#8220;You&#8221; anytime to take back control.
            </p>
            {blockedNote && (
              <div style={{ fontSize: 12, color: "var(--warn, #c77d1a)", border: "1px solid currentColor", borderRadius: 6, padding: 8 }}>
                {blockedNote}
              </div>
            )}
            {permReq && (
              <div style={{ fontSize: 12, border: "var(--border-width) solid var(--accent)", borderRadius: 8, padding: 10, display: "flex", flexDirection: "column", gap: 8 }}>
                <span>The agent wants to: <b>{permReq.action}</b></span>
                <div style={{ display: "flex", gap: 6 }}>
                  <Button onClick={() => answerPerm("allow")}>Allow</Button>
                  <Button variant="secondary" onClick={() => answerPerm("deny")}>Deny</Button>
                  <Button variant="secondary" onClick={() => answerPerm("take")}>Take control</Button>
                </div>
              </div>
            )}
            <div ref={runsScrollRef} className="aygent-scroll" style={{ flex: 1, minHeight: 0, overflowY: "auto", display: "flex", flexDirection: "column", gap: 10 }}>
              {runs.map((r) => (
                <div key={r.id} style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  <div style={{ alignSelf: "flex-end", background: "var(--accent)", color: "var(--bg)", borderRadius: 10, padding: "6px 10px", fontSize: 12.5, maxWidth: "90%" }}>{r.prompt}</div>
                  {r.steps.length > 0 && (
                    <div style={{ border: "var(--border-width) solid var(--line)", borderRadius: 8, padding: 8, fontSize: 12.5 }}>
                      <div style={{ display: "flex", marginBottom: 4 }}>
                        <b>Plan</b><span style={{ marginLeft: "auto", color: "var(--text-faint)" }}>{r.stepsDone}/{r.steps.length}</span>
                      </div>
                      {r.steps.map((s, i) => (
                        <div key={i} style={{ display: "flex", gap: 6, padding: "2px 0", opacity: i < r.stepsDone ? 0.6 : 1 }}>
                          <span>{i < r.stepsDone ? "\u2611" : "\u2610"}</span><span>{s}</span>
                        </div>
                      ))}
                    </div>
                  )}
                  {r.action && !r.planDone && <div style={{ fontSize: 12, color: "var(--text-muted)" }}>&#9881; {r.action}</div>}
                  {r.planDone && r.summary && (
                    <div style={{ fontSize: 12.5, color: r.stopped ? "var(--danger, #d64545)" : "var(--text)" }}>
                      {r.stopped ? "\u26D4 " : ""}{r.summary}
                    </div>
                  )}
                  {!r.planDone && r.steps.length === 0 && <div style={{ fontSize: 12, color: "var(--text-muted)" }}>thinking&#8230;</div>}
                </div>
              ))}
            </div>
            <div style={{ display: "flex", gap: 6 }}>
              <input
                value={agentPrompt}
                onChange={(e) => setAgentPrompt(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter") void runAgent(); }}
                placeholder="Tell the agent&#8230;"
                style={{ flex: 1, minWidth: 0, height: 34, padding: "0 10px", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)", fontSize: 13, outline: "none" }}
              />
              {agentBusy ? (
                <Button variant="secondary" onClick={() => {
                  const last = runs[runs.length - 1];
                  if (last) invoke("agent_stop", { channel: last.id }).catch(() => {});
                }}>Stop</Button>
              ) : (
                <Button onClick={() => void runAgent()} disabled={!agentPrompt.trim()}>Act</Button>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function navBtnStyle(active = false): React.CSSProperties {
  return {
    display: "flex", alignItems: "center", justifyContent: "center",
    width: 32, height: 32, borderRadius: "var(--radius-control)",
    border: `var(--border-width) solid ${active ? "var(--accent)" : "var(--line)"}`,
    background: "transparent", color: active ? "var(--accent)" : "var(--text)",
    cursor: "pointer", flexShrink: 0, fontSize: 14, padding: 0,
  };
}

function toggleStyle(on: boolean): React.CSSProperties {
  return {
    padding: "6px 14px", fontSize: 13, fontWeight: 700, border: "none", cursor: "pointer",
    background: on ? "var(--accent)" : "transparent",
    color: on ? "var(--bg)" : "var(--text-muted)",
  };
}
