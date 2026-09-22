// Herdr-style notifications for Command Pro (branch ui-command-pro).
// working = tab pulse (CSS), done / needs-input = system Notification (if
// granted) + in-app toast. No new Rust deps: uses the WebView Notification API.

export type NotifyKind = "done" | "needs-input";

const LS_KEY = "aygent.notify.enabled";

export function notifyEnabled(): boolean {
  try { return localStorage.getItem(LS_KEY) !== "off"; } catch { return true; }
}
export function setNotifyEnabled(on: boolean) {
  try { localStorage.setItem(LS_KEY, on ? "on" : "off"); } catch { /* ignore */ }
}

export async function ensureNotifyPermission(): Promise<boolean> {
  try {
    if (!("Notification" in window)) return false;
    if (Notification.permission === "granted") return true;
    if (Notification.permission === "denied") return false;
    return (await Notification.requestPermission()) === "granted";
  } catch { return false; }
}

/** Heuristic: a turn that ends with a question probably needs the human. */
export function kindFor(text: string): NotifyKind {
  const t = (text || "").trim();
  if (/\?\s*$/.test(t)) return "needs-input";
  return "done";
}

export function notifyThread(agentName: string, threadTitle: string, kind: NotifyKind, preview: string) {
  if (!notifyEnabled()) return;
  const title = kind === "needs-input"
    ? `${agentName} · ${threadTitle} — needs you`
    : `${agentName} · ${threadTitle} — done`;
  const body = (preview || "").trim().slice(0, 140) || (kind === "needs-input" ? "Asked a question." : "Turn finished.");
  // System notification (best-effort).
  try {
    if ("Notification" in window && Notification.permission === "granted") {
      new Notification(title, { body });
    }
  } catch { /* ignore */ }
  // In-app toast (always).
  window.dispatchEvent(new CustomEvent("aygent-thread-notify", {
    detail: { agentName, threadTitle, kind, body },
  }));
}
