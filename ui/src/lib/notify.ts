// Command Pro thread alerts: a little ding, nothing else (branch ui-command-pro).
// Tab dots + rail dots already show state; no popups, no system notifications.
// Uses a WebAudio blip so there are no asset files to ship.

const LS_KEY = "aygent.notify.enabled";

export function notifyEnabled(): boolean {
  try { return localStorage.getItem(LS_KEY) !== "off"; } catch { return true; }
}
export function setNotifyEnabled(on: boolean) {
  try { localStorage.setItem(LS_KEY, on ? "on" : "off"); } catch { /* ignore */ }
}

let ctx: AudioContext | null = null;

/** Short soft blip (~880Hz sine, 0.12s). Best-effort: never throws. */
export function ding() {
  if (!notifyEnabled()) return;
  try {
    const AC = window.AudioContext || (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!AC) return;
    if (!ctx) ctx = new AC();
    if (ctx.state === "suspended") void ctx.resume();
    const t = ctx.currentTime;
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = "sine";
    osc.frequency.setValueAtTime(880, t);
    gain.gain.setValueAtTime(0.0001, t);
    gain.gain.exponentialRampToValueAtTime(0.12, t + 0.015);
    gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.14);
    osc.connect(gain).connect(ctx.destination);
    osc.start(t);
    osc.stop(t + 0.16);
  } catch { /* ignore */ }
}

/** A thread finished — just ding. Dots on the tab + rail carry the state. */
export function notifyThread() {
  ding();
}
