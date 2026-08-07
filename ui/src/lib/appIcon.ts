// AYGENT — DYNAMIC APP ICON (renderer).
//
// Mason 08-04: the Dock icon should track the app's theme — a flat "AY" in
// the current accent colour, on a black or white ground depending on light or
// dark mode. The accent is a free-form colour, so pre-rendering every variant
// as a PNG is impossible; instead we DRAW the icon here on a <canvas> and hand
// the bytes to Rust, which installs it via NSApplication.applicationIconImage.
//
// macOS Dock icons are read at 1024x1024 and the system rounds/squircles them
// itself for the Dock, so we draw a full-bleed rounded square at that size.

import { invoke } from "@tauri-apps/api/core";
import type { Mode } from "./theme";

const SIZE = 1024;

/** Resolve the accent to a concrete hex. "" means the theme's default, which
 *  is the inverse of the background (black on light, white on dark). */
function resolveAccent(accent: string, mode: Mode): string {
  if (accent) return accent;
  if (mode === "matrix") return "#00ff41"; // phosphor green is the identity
  if (mode === "dark") return "#ffffff";
  if (mode === "neutral") return "#22201c"; // soft near-black on warm paper
  return "#111111";
}

/**
 * Draw the icon and return it as a PNG data URL's base64 payload.
 * Exported separately from `applyAppIcon` so it can be previewed/tested
 * without touching the Dock.
 */
export function renderIconPng(mode: Mode, accent: string): string | null {
  const canvas = document.createElement("canvas");
  canvas.width = SIZE;
  canvas.height = SIZE;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;

  const ink = resolveAccent(accent, mode);
  const bg = mode === "matrix" ? "#000000" : mode === "dark" ? "#0b0b0c" : mode === "neutral" ? "#f4f1ea" : "#fafafa";

  // Ground: a rounded square, full-bleed. macOS applies its own mask, but a
  // radius here keeps it looking right if the mask ever changes.
  const r = SIZE * 0.22;
  ctx.fillStyle = bg;
  ctx.beginPath();
  ctx.moveTo(r, 0);
  ctx.arcTo(SIZE, 0, SIZE, SIZE, r);
  ctx.arcTo(SIZE, SIZE, 0, SIZE, r);
  ctx.arcTo(0, SIZE, 0, 0, r);
  ctx.arcTo(0, 0, SIZE, 0, r);
  ctx.closePath();
  ctx.fill();

  // The "AY" wordmark, centred.
  ctx.font = `700 ${Math.round(SIZE * 0.42)}px -apple-system, BlinkMacSystemFont, "SF Pro Display", "Helvetica Neue", Arial, sans-serif`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.letterSpacing = `${Math.round(SIZE * 0.01)}px`;

  // Flat letterform — no glow/shadow (Mason 08-04: same call as the in-app
  // icons; the bloom read as muddy at Dock size).
  ctx.fillStyle = ink;
  ctx.fillText("AY", SIZE / 2, SIZE / 2 + SIZE * 0.012);

  const url = canvas.toDataURL("image/png");
  const comma = url.indexOf(",");
  return comma >= 0 ? url.slice(comma + 1) : null;
}

function hexToRgba(hex: string, alpha: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return `rgba(0,0,0,${alpha})`;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
}

/** Render for the current theme and install it as the Dock icon. */
export async function applyAppIcon(mode: Mode, accent: string): Promise<void> {
  try {
    const b64 = renderIconPng(mode, accent);
    if (!b64) return;
    await invoke("set_app_icon", { pngB64: b64 });
  } catch {
    // Non-fatal: a themed icon is a nicety, never a reason to break startup.
  }
}
