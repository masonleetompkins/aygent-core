// AYGENT theme controller (DESIGN.md). Flips light/dark and applies the accent
// as *lighting* (retints outline + shadow/glow). Persistence to SQLite lands
// with the Settings screen; for now it reads/writes localStorage so the choice
// survives reloads during dev.

export type Mode = "light" | "dark";

const LS_MODE = "aygent.theme.mode";
const LS_ACCENT = "aygent.theme.accent"; // hex, or "" for default (black/white)

/** #rrggbb -> "r, g, b" for rgba() in the CSS variables. */
function hexToRgb(hex: string): string | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return `${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}`;
}

export function applyTheme(mode: Mode, accentHex: string) {
  const root = document.documentElement;
  root.setAttribute("data-theme", mode);

  // AYGENT REMOTE: mirror the theme to the Rust side so the web client can
  // match it (sent in the remote hello). Fire-and-forget.
  import("@tauri-apps/api/core")
    .then(({ invoke }) => invoke("theme_sync", { mode, accent: accentHex || "" }))
    .catch(() => {});

  const rgb = accentHex ? hexToRgb(accentHex) : null;
  if (rgb) {
    root.style.setProperty("--accent", accentHex);
    root.style.setProperty("--accent-rgb", rgb);
    root.setAttribute("data-accent", "on");
  } else {
    // default: clear overrides so the mode's black/white accent applies
    root.style.removeProperty("--accent");
    root.style.removeProperty("--accent-rgb");
    root.removeAttribute("data-accent");
  }
}

export function loadTheme(): { mode: Mode; accent: string } {
  const mode = (localStorage.getItem(LS_MODE) as Mode) || "light";
  const accent = localStorage.getItem(LS_ACCENT) || "";
  return { mode, accent };
}

export function saveTheme(mode: Mode, accent: string) {
  localStorage.setItem(LS_MODE, mode);
  localStorage.setItem(LS_ACCENT, accent);
  applyTheme(mode, accent);
}

/** Call once at app start. */
export function initTheme() {
  const { mode, accent } = loadTheme();
  applyTheme(mode, accent);
  return { mode, accent };
}
