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
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Input } from "../components/ui";
import { Icon } from "../components/Icon";

type Tab = { id: number; addr: string; title: string; editing: boolean; opened?: boolean };
// Monotonic tab id counter — NEVER reuse ids (Atlas P1: label reuse races the
// async webview close, so a reopened tab could grab a stale/half-closed handle).
let TAB_SEQ = 1;
// Width (CSS px) of the right-hand agent pane when handed off. syncBounds
// shrinks the native webview by this + a gap so the pane sits BESIDE the page.
const AGENT_PANE_W = 340;
// The native WKWebView's OWN CALayer is rounded in Rust (cornerRadius +
// masksToBounds) so the OS clips the page to a rounded rect. To make the accent
// frame read as an OUTER STROKE framing the page (not a hairline hidden under
// it), we pull the webview IN by the stroke width so the border lives in that
// reclaimed ring, and round the page to a slightly SMALLER radius so its
// corners nest just inside the frame's. --radius-card=14, --stroke≈2.
const FRAME_RADIUS = 14;
const FRAME_STROKE = 2;           // accent border width (outer stroke)
const WEB_INSET = FRAME_STROKE;   // pull webview in so the stroke frames it
const WEB_RADIUS = FRAME_RADIUS - FRAME_STROKE; // page corners nest inside frame
const newTab = (): Tab => ({ id: TAB_SEQ++, addr: "", title: "New Tab", editing: true, opened: false });

export function Browser() {
  const [installed] = useInstalled();
  const [tabs, setTabs] = useState<Tab[]>([newTab()]);
  const [activeId, setActiveId] = useState<number>(() => tabs[0].id);
  const [driver, setDriver] = useState<"human" | "agent">("human");
  const [agentPrompt, setAgentPrompt] = useState("");
  const [agentBusy, setAgentBusy] = useState(false);
  // CONVERSATIONAL AGENT THREAD (Mason's Part-2 redesign). The panel is a
  // conversation: each `Run` is [the user's prompt] followed by its REPLY —
  // the plan/checklist that checks off live, a single transient current-action
  // line, and a final one-line summary. We do NOT keep a growing raw tool-call
  // list anymore; the checklist IS the meaningful progress, and `action` is an
  // EPHEMERAL "currently doing X" line that gets overwritten each action.
  type Run = {
    id: string;
    prompt: string;
    steps: string[];      // the plan (reply to the prompt)
    stepsDone: number;    // how many checked off
    planDone: boolean;    // turn finished (spinner off, checklist frozen)
    action: string;       // TRANSIENT "currently doing X" line (replaceable)
    summary: string;      // final one-line summary when done
    stopped?: boolean;    // deny / take / step-failed => show why, freeze
  };
  const [runs, setRuns] = useState<Run[]>([]);
  const patchRun = (id: string, p: Partial<Run>) =>
    setRuns((rs) => rs.map((r) => (r.id === id ? { ...r, ...p } : r)));
  // SHARED-CONTROL state, mirrored from Rust `browser:control` events. When the
  // agent hits a wall (login/CAPTCHA/verification) it releases the wheel with a
  // non-empty `note` — that's the BLOCKED signal that pulses the You/Agent
  // toggle so the human knows to intervene.
  const [blockedNote, setBlockedNote] = useState<string>("");
  // BROWSER CHROME CONTROLS. Browsing history is now PERSISTED IN RUST
  // (history.rs -> <app_data>/browser/history.json): every committed navigation
  // — agent navigations, in-page nav, redirects AND human go()s — is recorded
  // at the webview/CDP source, survives restarts, and is capped at 1000. This
  // FE list is just what `browser_history_list` returns (newest-first). Ts is
  // UNIX SECONDS from Rust (not ms), matching the downloads mtime unit.
  type HistEntry = { url: string; title: string; ts: number };
  const [history, setHistory] = useState<HistEntry[]>([]);
  const [panel, setPanel] = useState<null | "history" | "downloads">(null);
  // OVERLAY-VS-NATIVE-WEBVIEW FIX (Atlas). The visible tab is a REAL native
  // WKWebView layer that paints ON TOP of ALL DOM chrome wherever it overlaps
  // the page-area rect — z-index is powerless against it. So the History /
  // Downloads drop panel (which drops DOWN into the page area) was getting
  // COVERED and Mason couldn't see it. Fix: while ANY drop panel is open, HIDE
  // the active tab's native webview (nothing left to cover the panel); on close
  // we re-show + reposition it. `panelRef` lets syncBounds / observer / timeout
  // closures read the CURRENT panel state so they never re-front the webview
  // over an open panel. GUARANTEE: every re-show path is gated on `!panelRef`,
  // and closing the panel (or leaving the screen) always restores the webview.
  const panelRef = useRef<null | "history" | "downloads">(panel);
  panelRef.current = panel;
  type DlEntry = { name: string; size: number; mtime: number };
  const [downloads, setDownloads] = useState<DlEntry[]>([]);
  // Fetch the PERSISTED history (newest-first) from Rust. Called when the
  // History panel opens and after a clear. Navigation RECORDING happens in Rust
  // at the source now, so the FE no longer tracks visits itself.
  const refreshHistory = async () => {
    try {
      const list = await invoke<HistEntry[]>("browser_history_list");
      console.log("[aygent][browser][HIST] browser_history_list ->", Array.isArray(list) ? list.length : 0, "entries");
      setHistory(Array.isArray(list) ? list : []);
    } catch (e) { console.error("browser_history_list", e); setHistory([]); }
  };
  async function openHistory() {
    if (panel === "history") { setPanel(null); return; }
    await refreshHistory();
    setPanel("history");
  }
  async function clearHistory() {
    try { await invoke("browser_history_clear"); } catch (e) { console.error("browser_history_clear", e); }
    await refreshHistory();
  }
  // Short relative timestamp for a unix-SECONDS value.
  const fmtWhen = (tsSecs: number) => {
    const diff = Math.max(0, Math.floor(Date.now() / 1000 - tsSecs));
    if (diff < 60) return "just now";
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`;
    try { return new Date(tsSecs * 1000).toLocaleDateString(); } catch { return ""; }
  };
  const editRef = useRef<HTMLInputElement>(null);
  // The div whose rect the native webview is positioned over.
  const paneRef = useRef<HTMLDivElement>(null);
  // Last rect we pushed to Rust — dedupe so we don't re-apply an unchanged rect.
  const lastBoundsRef = useRef<string>("");
  // Active tab id in a ref so syncBounds (called from observers/timeouts) always
  // reads the CURRENT active tab, not a stale closure value.
  const activeIdRef = useRef<number>(activeId);
  activeIdRef.current = activeId;
  // Same trick for `driver`: syncBounds runs inside a double-rAF callback whose
  // closure captured driver at render time. On the flip to "agent" the effect
  // fires syncBounds BEFORE a re-render rebinds the closure, so it read the OLD
  // driver and never shrank the webview (page stayed full-width, clipped behind
  // the agent panel). Read driver from a ref so syncBounds always sees current.
  const driverRef = useRef<"human" | "agent">(driver);
  driverRef.current = driver;

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
    // A drop panel (History/Downloads) is open — the native webview is hidden so
    // the panel is visible. Don't reposition/re-front it now; the panel-close
    // effect re-shows + re-syncs. (Guards the ResizeObserver/resize/timeout
    // callers that would otherwise re-front the webview over an open panel.)
    if (panelRef.current) return;
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
      // NO rightPane subtraction: paneRef is the LEFT flex child, and when the
      // Agent panel mounts as its right sibling the flex layout ALREADY shrinks
      // paneRef by the panel width + gap. r (paneRef's rect) is therefore the
      // correct target on its own. The old subtraction double-counted the shrink
      // (748 -> 48px in agent mode, per the NSVIEW log: 748-48≈700≈2×350). We
      // simply track paneRef's real border-box.
      // Webview rect == paneRef's BORDER-BOX rect (no inset). The rounded
      // outline is a pointer-events:none overlay rendered ON TOP of the webview,
      // so its ~radius corner arcs mask the webview's square corners while the
      // webview fills the whole box.
      const inset = WEB_INSET;
      // Derive edges from ROUNDED top/left AND rounded bottom/right, then take
      // the difference for size. Rounding width/height independently pushes ~1px
      // of accumulated rounding onto the bottom/right edge (the Y-flip computes
      // the bottom as flip_h - y - h), making the bottom inset look a hair
      // bigger than the top. Symmetric edges = symmetric insets.
      const left = Math.round(r.left + inset);
      const top = Math.round(r.top + inset);
      const right = Math.round(r.right - inset);
      const bottom = Math.round(r.bottom - inset);
      const bounds = {
        x: left, y: top,
        width: Math.max(right - left, 1),
        height: Math.max(bottom - top, 1),
        radius: WEB_RADIUS,
        // Content-area size the rect was measured against — Rust uses this to
        // compute the native titlebar inset at runtime (THE fix).
        clientWidth, clientHeight,
        parentHeight,
        tabId: activeIdRef.current,
      };
      // Skip redundant calls — only push when the rect actually changed.
      const key = `${bounds.tabId},${bounds.x},${bounds.y},${bounds.width},${bounds.height},${bounds.clientWidth},${bounds.clientHeight},${bounds.radius}`;
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
      // Leaving the Browser screen: hide ALL tab webviews so none float over
      // other screens.
      invoke("webview_hide_others", { keep: null }).catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [installed]);

  // `rawUrl` (from the input's live value on Enter) takes precedence over the
  // tabs-closure addr, which can be one keystroke stale under React batching —
  // that staleness made Enter read an empty url and silently bail ("nothing
  // happens"). Fall back to the tab's stored addr for programmatic calls.
  async function go(id: number, rawUrl?: string) {
    const tab = tabs.find((t) => t.id === id);
    const url = (rawUrl ?? tab?.addr ?? "").trim();
    if (!url) { patch(id, { editing: false }); return; }
    // Make sure the stored addr reflects what we're navigating to.
    patch(id, { addr: url });
    patch(id, { editing: false, title: url });
    // History is recorded in RUST at the navigation source now (webview_open ->
    // page-info poll + the CDP mirror), so the FE no longer pushes here.
    const el = paneRef.current;
    const r = el?.getBoundingClientRect();
    // First navigation for the pane opens/positions the webview; later ones reuse.
    // Guard the OPEN rect too — the embed is BORN at this rect, so an origin-
    // hugging / degenerate measurement here creates the child covering the whole
    // window (the pop-out) and no later set_bounds fully recovers it. If the
    // pane isn't laid out yet, fall back to a safe inset and let syncBounds
    // correct it on the next frame.
    const inset = WEB_INSET;
    let ox = Math.round((r?.left ?? 0) + inset), oy = Math.round((r?.top ?? 0) + inset);
    let ow = Math.round(Math.max((r?.width ?? 800) - inset * 2, 1)), oh = Math.round(Math.max((r?.height ?? 600) - inset * 2, 1));
    if (ow < 40 || oh < 40 || (ox <= 4 && oy <= 4)) {
      // Not laid out: use a conservative inset (right of sidebar, below tabs).
      ox = 300; oy = 100; ow = 600; oh = 500;
    }
    const clientWidth = Math.round(document.documentElement.clientWidth);
    const clientHeight = Math.round(document.documentElement.clientHeight);
    try {
      await invoke("webview_open", { url, x: ox, y: oy, width: ow, height: oh, clientWidth, clientHeight, radius: WEB_RADIUS, tabId: id });
    } catch (e) {
      console.error("webview_open failed", e);
    }
    // This tab's webview is now the active surface — hide the others.
    invoke("webview_hide_others", { keep: id }).catch(() => {});
    patch(id, { opened: true });
    // Re-sync a beat later so the webview lands on the SETTLED rect (the agent
    // prompt bar toggling can shift the pane by a row).
    setTimeout(syncBounds, 120);
    // Read the REAL page title back from the webview once it loads, so the tab
    // shows "Google" not "New Tab". Poll a few times because navigation is async
    // and document.title fills in after the load settles.
    let tries = 0;
    const pollTitle = async () => {
      tries++;
      try {
        const doc = await invoke<{ title?: string; url?: string }>("webview_page_info", { tabId: id });
        const real = (doc?.title || "").trim();
        const realUrl = (doc?.url || "").trim();
        // NOTE: the webview_page_info call itself records the landed url+title
        // in Rust history (covers in-page clicks/redirects) — no FE push needed.
        if (real) { patch(id, { title: real }); return; }
      } catch { /* webview not ready yet */ }
      if (tries < 6) setTimeout(pollTitle, 400);
    };
    setTimeout(pollTitle, 400);
  }

  function clickTab(id: number) {
    // Browser-standard tab behavior:
    //  - Click an INACTIVE tab  -> just switch to it (no editor).
    //  - Click the ACTIVE tab   -> open its address field to type a new URL.
    //  - A fresh unopened tab (no page yet) opens its editor so you can type.
    const tab = tabs.find((t) => t.id === id);
    const wasActive = id === activeId;
    if (!wasActive) setActiveId(id);
    if (wasActive || !tab?.opened) {
      patch(id, { editing: true });
      setTimeout(() => editRef.current?.select(), 0);
    }
  }
  function addTab() { const t = newTab(); setTabs((ts) => [...ts, t]); setActiveId(t.id); }
  function closeTab(id: number) {
    // Destroy this tab's native webview (frees its WebContent process).
    invoke("webview_close", { tabId: id }).catch(() => {});
    setTabs((ts) => {
      const next = ts.filter((t) => t.id !== id);
      if (next.length === 0) { const t = newTab(); setActiveId(t.id); return [t]; }
      if (id === activeId) setActiveId(next[next.length - 1].id);
      return next;
    });
  }

  // TAB SWITCH: show the active tab's webview + hide the rest. If the active
  // tab has already navigated (opened), reposition+show it; if it's a fresh
  // "New Tab" (not opened), just hide everything so the placeholder shows.
  useEffect(() => {
    if (installed !== true) return;
    const cur = tabs.find((t) => t.id === activeId);
    if (cur?.opened) {
      // Reposition + un-hide the active tab; webview_hide_others fronts it.
      // Use cur.id (the tab we just switched TO) — activeId in this closure is
      // fresh here, but be explicit so a re-render can't front a stale webview.
      invoke("webview_hide_others", { keep: cur.id }).catch(() => {});
      syncBounds();
      setTimeout(syncBounds, 60);
    } else {
      // Fresh tab with no page yet — hide all so the empty-state shows.
      invoke("webview_hide_others", { keep: null }).catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeId]);

  // THE HAND-OFF. Flip to agent: the right prompt pane opens + the webview
  // shrinks to make room. Flip to human: pane closes, webview reclaims width.
  // Re-sync bounds a beat after the toggle so the webview resizes with it.
  useEffect(() => { syncBounds(); const t = setTimeout(syncBounds, 60); return () => clearTimeout(t); }, [driver]); // eslint-disable-line react-hooks/exhaustive-deps

  // DROP-PANEL VISIBILITY (Atlas overlay fix). The native WKWebView paints on
  // top of DOM wherever it overlaps the page rect, so an open History/Downloads
  // panel would be COVERED. While a panel is open we HIDE all tab webviews
  // (nothing left to cover the panel); when it closes we re-show + reposition
  // the active tab's webview. This effect is the SINGLE owner of "panel drives
  // webview visibility", and its cleanup guarantees the webview is never left
  // stuck hidden — on unmount (leave screen) the mount effect's cleanup already
  // hides everything, and on panel-close here we restore the active tab.
  useEffect(() => {
    if (installed !== true) return;
    if (panel) {
      // Panel open: hide EVERY tab's native layer so the DOM panel shows.
      invoke("webview_hide_others", { keep: null }).catch(() => {});
      return;
    }
    // Panel closed: re-front the active tab's webview if it has a page, at its
    // correct rect. `panelRef` is already null here, so syncBounds runs.
    const cur = tabs.find((t) => t.id === activeIdRef.current);
    if (cur?.opened) {
      invoke("webview_hide_others", { keep: cur.id }).catch(() => {});
      syncBounds();
      setTimeout(syncBounds, 60);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [panel, installed]);

  // If the user switches tabs or flips the driver while a panel is open, close
  // the panel first so we never leave a webview re-fronted over a stale panel
  // (and so the panel-visibility effect above can cleanly restore the webview).
  useEffect(() => { setPanel(null); }, [activeId, driver]);

  // LISTEN for shared-control changes from Rust. A non-empty `note` means the
  // agent got blocked and wants the human -> pulse the toggle + show the note.
  // Taking the wheel (You) clears it.
  useEffect(() => {
    let un: undefined | (() => void);
    listen<{ driver: string; note: string; agent_active: boolean }>("browser:control", (ev) => {
      const note = ev.payload?.note ?? "";
      setBlockedNote(note);
      // If the agent handed back to the human, reflect that in the toggle.
      if (note && ev.payload?.driver === "human") setDriver("human");
    }).then((f) => { un = f; }).catch(() => {});
    return () => { un?.(); };
  }, []);

  // Flipping to "You" = human takes the wheel; clears any blocked pulse.
  useEffect(() => {
    if (driver === "human" && blockedNote) {
      invoke("browser_take_wheel").catch(() => {});
      setBlockedNote("");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [driver]);

  // Tell Rust which tab is active AND its current URL, so the agent tools act on
  // the VISIBLE tab and the host pre-check matches the page you're actually on
  // (otherwise it reads the empty CDP session and prompts even for same-site).
  useEffect(() => {
    const cur = tabs.find((t) => t.id === activeId);
    invoke("set_active_browser_tab", { tabId: activeId, url: cur?.addr || "" }).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeId, tabs]);

  // PENDING PERMISSION REQUEST from the agent (new host / blocked action).
  // { id, action, detail } -> render the Allow/Deny/Take Control card.
  const [permReq, setPermReq] = useState<{ id: string; action: string; detail: string } | null>(null);
  useEffect(() => {
    let un: undefined | (() => void);
    listen<{ id: string; action: string; detail: string }>("browser:permission-request", (ev) => {
      if (ev.payload?.id) setPermReq(ev.payload);
    }).then((f) => { un = f; }).catch(() => {});
    return () => { un?.(); };
  }, []);

  function answerPerm(answer: "allow" | "deny" | "take") {
    if (!permReq) return;
    invoke("browser_permission_answer", { id: permReq.id, answer, grantHost: permReq.detail }).catch(() => {});
    if (answer === "take") setDriver("human");
    setPermReq(null);
    setBlockedNote("");
  }

  // BROWSER CHROME CONTROLS (ITEM 3). Back / Forward / Refresh drive the visible
  // tab's engine-native WebKit history via webview_history. After a nav we
  // re-poll the page info so the tab title + session history stay current.
  const canNav = active?.opened === true;
  async function navHistory(action: "back" | "forward" | "reload") {
    if (!canNav) return;
    try { await invoke("webview_history", { action, tabId: active.id }); }
    catch (e) { console.error("webview_history", action, e); }
    // Give the WebKit history nav a beat, then refresh title/url + history list.
    setTimeout(async () => {
      try {
        const doc = await invoke<{ title?: string; url?: string }>("webview_page_info", { tabId: active.id });
        const real = (doc?.title || "").trim();
        const realUrl = (doc?.url || "").trim();
        if (real) patch(active.id, { title: real });
        if (realUrl) { patch(active.id, { addr: realUrl }); }
      } catch { /* not ready */ }
    }, 350);
  }
  async function openDownloads() {
    if (panel === "downloads") { setPanel(null); return; }
    try {
      const list = await invoke<DlEntry[]>("browser_downloads_list");
      setDownloads(Array.isArray(list) ? list : []);
    } catch (e) { console.error("browser_downloads_list", e); setDownloads([]); }
    setPanel("downloads");
  }
  function navTo(url: string) {
    // Navigate the active tab to a history entry.
    go(active.id, url);
    setPanel(null);
  }
  const fmtSize = (n: number) =>
    n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : n >= 1e3 ? `${(n / 1e3).toFixed(0)} KB` : `${n} B`;

  async function runAgent() {
    const task = agentPrompt.trim();
    if (!task || agentBusy) return;
    setAgentBusy(true);
    setAgentPrompt("");

    // Start a new conversational run: the user's prompt + an (about-to-fill)
    // reply. The plan arrives first as this run's checklist (the reply), then
    // checkmarks + a transient action line, then a one-line summary.
    const runId = `agent-run-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
    setRuns((rs) => [...rs, {
      id: runId, prompt: task, steps: [], stepsDone: 0,
      planDone: false, action: "", summary: "",
    }]);

    // Per-run event channel: the agent emits its PLAN + STEP checkmarks +
    // ACTION (transient) + DONE here. We render them live as the reply to this
    // prompt. Unlisten when the run resolves so channels don't stack.
    const channel = runId;
    let un: undefined | (() => void);
    try {
      un = await listen<any>(channel, (ev) => {
        const p = ev.payload || {};
        if (p.kind === "plan" && Array.isArray(p.steps)) {
          patchRun(runId, { steps: p.steps as string[], stepsDone: 0, planDone: false, action: "" });
        } else if (p.kind === "step") {
          // `done` = how many steps are now checked (1-based count). Advancing
          // clears the transient action line (that action is finished).
          if (typeof p.done === "number") patchRun(runId, { stepsDone: p.done, action: "" });
        } else if (p.kind === "action") {
          // TRANSIENT current-action line — a single replaceable "→ doing X…"
          // string, NOT an accumulating log. Overwrite the previous one.
          if (typeof p.label === "string") patchRun(runId, { action: p.label });
        } else if (p.kind === "done") {
          // Normal finish marks every step done; a STOPPED finish (deny /
          // take / step-failed) freezes the checklist and surfaces WHY.
          if (p.stopped) {
            patchRun(runId, {
              planDone: true, action: "", stopped: true,
              summary: typeof p.summary === "string" && p.summary ? p.summary : "stopped",
            });
          } else {
            const patch: Partial<Run> = {
              planDone: true, action: "",
              summary: typeof p.summary === "string" ? p.summary : "",
            };
            if (typeof p.total === "number") patch.stepsDone = p.total;
            patchRun(runId, patch);
          }
        }
      });
    } catch { /* channel listen best-effort */ }

    try {
      // REAL model-in-the-loop agent. It PLANS the task into a checklist, then
      // drives the browser via CDP (single source of truth — acts + reads the
      // SAME page, no desync). It asks permission for new hosts (Allow/Deny/
      // Take) and hands off on login/CAPTCHA walls (pulse). Prime it toward the
      // page. folder:null => Rust resolves the ACTIVE agent + its configured model.
      const cur = tabs.find((t) => t.id === activeId);
      const primed = `You are driving the in-app browser in the tab the human is watching (currently: ${cur?.addr || cur?.title || "a page"}). Use the browser tools to act in THAT page. Task: ${task}`;
      await invoke<string>("agent_run", { prompt: primed, folder: null, channel });
      // The raw transcript is intentionally DISCARDED — the checklist + the
      // one-line `done` summary are the user-facing result now (no raw list).
      patchRun(runId, { planDone: true, action: "" });
    } catch (e) {
      patchRun(runId, { planDone: true, action: "", stopped: true, summary: `${e}` });
    } finally {
      un?.();
      setAgentBusy(false);
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
                  onKeyDown={(e) => {
                    if (e.key === "Enter") { e.preventDefault(); go(t.id, e.currentTarget.value); }
                    if (e.key === "Escape") patch(t.id, { editing: false });
                  }}
                  // Blur (click away / clicking another tab / clicking the page)
                  // just EXITS edit mode — it does NOT navigate. Enter is how you
                  // commit a URL. This is what makes clicking the page close the
                  // editor and clicking another tab switch cleanly on the first
                  // click (blur closed the editor; the tab's own onClick switches).
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

        {/* BROWSER CHROME CONTROLS (ITEM 3) + THE HAND-OFF TOGGLE. The nav
            controls (Back / Forward / Refresh / History / Downloads) sit BESIDE
            the You/Agent toggle, which stays where it is. The toggle pulses when
            the agent is blocked (blockedNote set) so the human knows to step in. */}
        <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 8, position: "relative" }}>
          {/* Nav cluster */}
          <div style={{ display: "flex", alignItems: "center", gap: 2 }}>
            <button title="Back" onClick={() => navHistory("back")} disabled={!canNav} style={navBtnStyle(false, !canNav)}>
              <Icon name="arrow-left" size={15} />
            </button>
            <button title="Forward" onClick={() => navHistory("forward")} disabled={!canNav} style={navBtnStyle(false, !canNav)}>
              <Icon name="arrow-right" size={15} />
            </button>
            <button title="Refresh" onClick={() => navHistory("reload")} disabled={!canNav} style={navBtnStyle(false, !canNav)}>
              <Icon name="refresh" size={15} />
            </button>
            <button title="History" onClick={openHistory} style={navBtnStyle(panel === "history", false)}>
              <Icon name="clock" size={15} />
            </button>
            <button title="Downloads" onClick={openDownloads} style={navBtnStyle(panel === "downloads", false)}>
              <Icon name="download" size={15} />
            </button>
          </div>
          {blockedNote && (
            <span style={{ fontSize: 11, color: "var(--accent)", fontWeight: 700, maxWidth: 200, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }} title={blockedNote}>
              ⚠ {blockedNote}
            </span>
          )}
          <div className={blockedNote ? "aygent-blocked-pulse" : undefined}
            style={{ display: "flex", borderRadius: "var(--radius-control)", overflow: "hidden", border: `var(--border-width) solid ${blockedNote ? "var(--accent)" : "var(--line)"}` }}>
            <button onClick={() => setDriver("human")}
              style={segStyle(driver === "human")}>You</button>
            <button onClick={() => setDriver("agent")}
              style={segStyle(driver === "agent")}>Agent</button>
          </div>

          {/* HISTORY / DOWNLOADS drop panel — anchored under the control cluster. */}
          {panel && (
            <div style={{
              position: "absolute", top: "calc(100% + 6px)", right: 0, zIndex: 50,
              width: 340, maxHeight: 360, overflowY: "auto",
              background: "var(--surface)", border: "var(--border-width) solid var(--line)",
              borderRadius: "var(--radius-card)", boxShadow: "var(--elevation)", padding: 8,
            }}>
              <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "2px 4px 8px" }}>
                <Icon name={panel === "history" ? "clock" : "download"} size={14} />
                <span style={{ fontSize: 12.5, fontWeight: 700 }}>
                  {panel === "history" ? "History" : "Downloads"}
                </span>
                <span onClick={() => setPanel(null)} title="Close"
                  style={{ marginLeft: "auto", cursor: "pointer", fontSize: 15, lineHeight: 1, opacity: 0.6 }}>×</span>
              </div>
              {panel === "history" ? (
                history.length === 0 ? (
                  <div style={{ fontSize: 12, color: "var(--text-muted)", padding: "6px 4px" }}>No pages visited yet.</div>
                ) : (
                  <>
                    {/* Rust returns newest-first already; render as-is. */}
                    {history.map((h, i) => (
                      <div key={`${h.url}-${h.ts}-${i}`} onClick={() => navTo(h.url)}
                        style={{ padding: "6px 6px", borderRadius: "var(--radius-control)", cursor: "pointer", display: "flex", flexDirection: "column", gap: 1 }}
                        onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg)")}
                        onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
                        <span style={{ fontSize: 12.5, fontWeight: 600, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{h.title || h.url}</span>
                        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                          <span style={{ flex: 1, fontSize: 11, color: "var(--text-muted)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{h.url}</span>
                          <span style={{ fontSize: 10.5, color: "var(--text-muted)", flexShrink: 0 }}>{fmtWhen(h.ts)}</span>
                        </div>
                      </div>
                    ))}
                    {/* Clear history — pinned at the BOTTOM of the History pane. */}
                    <div style={{ borderTop: "var(--border-width) solid var(--line)", marginTop: 6, paddingTop: 6 }}>
                      <button onClick={clearHistory}
                        style={{ width: "100%", padding: "6px 8px", fontSize: 12, fontWeight: 600,
                          color: "var(--text-muted)", background: "transparent", border: "none",
                          borderRadius: "var(--radius-control)", cursor: "pointer", display: "flex",
                          alignItems: "center", justifyContent: "center", gap: 6 }}
                        onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg)")}
                        onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
                        <Icon name="trash" size={13} /> Clear history
                      </button>
                    </div>
                  </>
                )
              ) : (
                downloads.length === 0 ? (
                  <div style={{ fontSize: 12, color: "var(--text-muted)", padding: "6px 4px" }}>No downloads yet.</div>
                ) : (
                  downloads.map((d, i) => (
                    <div key={`${d.name}-${i}`}
                      style={{ padding: "6px 6px", borderRadius: "var(--radius-control)", display: "flex", alignItems: "center", gap: 8 }}>
                      <Icon name="download" size={14} />
                      <span style={{ flex: 1, fontSize: 12.5, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }} title={d.name}>{d.name}</span>
                      <span style={{ fontSize: 11, color: "var(--text-muted)", flexShrink: 0 }}>{fmtSize(d.size)}</span>
                    </div>
                  ))
                )
              )}
            </div>
          )}
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
          {/* (Permission prompt moved INTO the Agent panel — answer inline
              without switching to You.) */}
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

            {/* INLINE PERMISSION PROMPT — the agent wants to do something outside
                the current site. Answer right here (Allow/Deny/Take Control),
                no need to switch to You. */}
            {permReq && (
              <div style={{
                border: "2px solid var(--accent)", borderRadius: "var(--radius-control)",
                padding: 10, background: "var(--bg)", display: "flex", flexDirection: "column", gap: 8,
              }}>
                <div style={{ fontSize: 12.5, fontWeight: 700 }}>Needs your OK</div>
                <div style={{ fontSize: 12, color: "var(--text)" }}>
                  The agent wants to <b>{permReq.action}</b>.
                </div>
                <div style={{ fontSize: 11, color: "var(--text-muted)", wordBreak: "break-all" }}>{permReq.detail}</div>
                <div style={{ display: "flex", gap: 6 }}>
                  <button onClick={() => answerPerm("allow")}
                    style={{ flex: 1, fontSize: 12, fontWeight: 700, padding: "6px 0", borderRadius: "var(--radius-control)", border: "none", background: "var(--accent)", color: "#fff", cursor: "pointer" }}>Allow</button>
                  <button onClick={() => answerPerm("deny")}
                    style={{ flex: 1, fontSize: 12, fontWeight: 700, padding: "6px 0", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)", background: "var(--surface)", color: "var(--text)", cursor: "pointer" }}>Deny</button>
                  <button onClick={() => answerPerm("take")}
                    style={{ flex: 1.2, fontSize: 12, fontWeight: 700, padding: "6px 0", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--accent)", background: "var(--surface)", color: "var(--accent)", cursor: "pointer" }}>Take Control</button>
                </div>
              </div>
            )}

            {/* CONVERSATIONAL THREAD (Mason's Part-2 redesign). Each run is a
                [user prompt] bubble followed by its REPLY: the plan/checklist
                that checks off live, a single TRANSIENT current-action line,
                and a one-line summary when done. NO raw tool-call list — the
                checklist is the meaningful progress; the action line is
                ephemeral (it overwrites, never accumulates). */}
            <div style={{ flex: 1, minHeight: 0, overflow: "auto", display: "flex", flexDirection: "column", gap: 12, paddingRight: 2 }}>
              {runs.length === 0 ? (
                <div style={{ fontSize: 12, color: "var(--text-faint)", marginTop: 4 }}>
                  Try: “search for the docs and open the first result”, “summarize this page”.
                </div>
              ) : runs.map((run) => {
                const isLast = run.id === runs[runs.length - 1].id;
                const running = isLast && agentBusy && !run.planDone;
                return (
                  <div key={run.id} style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                    {/* USER PROMPT bubble */}
                    <div style={{
                      alignSelf: "flex-end", maxWidth: "92%",
                      fontSize: 12.5, lineHeight: 1.4, padding: "7px 10px", borderRadius: 10,
                      background: "var(--accent)", color: "#fff",
                      whiteSpace: "pre-wrap", wordBreak: "break-word",
                    }}>{run.prompt}</div>

                    {/* REPLY: the plan/checklist, threaded UNDER the prompt. */}
                    {run.steps.length > 0 && (
                      <div style={{
                        alignSelf: "flex-start", maxWidth: "96%",
                        border: "var(--border-width) solid var(--line)", borderRadius: 10,
                        padding: 10, background: "var(--bg)", display: "flex", flexDirection: "column", gap: 6,
                      }}>
                        <div style={{ fontSize: 11.5, fontWeight: 700, color: "var(--text-muted)", display: "flex", alignItems: "center", gap: 6 }}>
                          <span>Plan</span>
                          <span style={{ marginLeft: "auto", fontWeight: 600, color: "var(--text-faint)" }}>
                            {Math.min(run.stepsDone, run.steps.length)}/{run.steps.length}
                          </span>
                        </div>
                        {run.steps.map((s, i) => {
                          const done = i < run.stepsDone;
                          const active = i === run.stepsDone && running;
                          return (
                            <div key={i} style={{ display: "flex", alignItems: "flex-start", gap: 8, fontSize: 12.5, lineHeight: 1.35 }}>
                              <span style={{
                                flexShrink: 0, width: 16, height: 16, borderRadius: 4, marginTop: 1,
                                display: "flex", alignItems: "center", justifyContent: "center",
                                fontSize: 11, fontWeight: 800,
                                border: `1.5px solid ${done || active ? "var(--accent)" : "var(--line)"}`,
                                background: done ? "var(--accent)" : "transparent",
                                color: done ? "#fff" : "var(--accent)",
                              }}>{done ? "✓" : active ? "•" : ""}</span>
                              <span style={{
                                color: done ? "var(--text-muted)" : "var(--text)",
                                textDecoration: done ? "line-through" : "none",
                                opacity: done ? 0.75 : 1,
                              }}>{s}</span>
                            </div>
                          );
                        })}

                        {/* TRANSIENT current-action line — one replaceable row,
                            only while this run is live and has an action set. */}
                        {running && run.action && (
                          <div style={{
                            marginTop: 2, fontSize: 11.5, color: "var(--accent)", fontStyle: "italic",
                            display: "flex", alignItems: "center", gap: 6, opacity: 0.9,
                            whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis",
                          }}>→ {run.action}…</div>
                        )}
                      </div>
                    )}

                    {/* If the plan hasn't arrived yet, a tiny "thinking" hint. */}
                    {run.steps.length === 0 && running && (
                      <div style={{ alignSelf: "flex-start", fontSize: 12, color: "var(--text-faint)" }}>planning…</div>
                    )}

                    {/* FINAL one-line summary (or stop reason). Falls back to a
                        plain "Done." when the turn finished cleanly without an
                        explicit summary (all steps checked). */}
                    {run.planDone && (run.summary || !run.stopped) && (
                      <div style={{
                        alignSelf: "flex-start", maxWidth: "96%",
                        fontSize: 12, lineHeight: 1.4, color: run.stopped ? "var(--accent)" : "var(--text-muted)",
                        display: "flex", alignItems: "flex-start", gap: 6,
                        whiteSpace: "pre-wrap", wordBreak: "break-word",
                      }}>{run.stopped ? "⛔" : "✓"} {run.summary || "Done."}</div>
                    )}
                  </div>
                );
              })}
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

// Browser nav-control button (ITEM 3): square icon button matching the tab-row
// affordances. `active` = a panel it toggles is open; `disabled` = no page yet.
function navBtnStyle(active: boolean, disabled: boolean): React.CSSProperties {
  return {
    display: "flex", alignItems: "center", justifyContent: "center",
    width: 30, height: 30, borderRadius: "var(--radius-control)",
    border: `var(--border-width) solid ${active ? "var(--accent)" : "var(--line)"}`,
    background: active ? "var(--surface)" : "transparent",
    color: disabled ? "var(--text-muted)" : active ? "var(--accent)" : "var(--text)",
    cursor: disabled ? "default" : "pointer",
    opacity: disabled ? 0.4 : 1, flexShrink: 0, padding: 0,
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
