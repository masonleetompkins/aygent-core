// AYGENT — DYNAMIC APP ICON (renderer).
//
// Mason 08-04: the Dock icon should track the app's theme — a flat "AY" in
// the current accent colour, on a black or white ground depending on light or
// dark mode. The accent is a free-form colour, so pre-rendering every variant
// as a PNG is impossible; instead we DRAW the icon here on a <canvas> and hand
// the bytes to Rust, which installs it via NSApplication.applicationIconImage.
//
// SIZING (Mason 09-04): macOS reads the icon at 1024x1024 but the visible
// artwork must sit on Apple's icon grid — an 824x824 squircle centred in the
// canvas (~100 px transparent margin on every side). The bundled icon.icns
// already does that, which is why the Dock tile looked right when the app was
// closed and ~24% too big while it was running: the live canvas render was
// full-bleed. We now draw the same grid, with Apple's continuous-curvature
// squircle rather than a circular-corner rounded rect.

import { invoke } from "@tauri-apps/api/core";
import type { Mode } from "./theme";

const SIZE = 1024;
// Apple icon grid: artwork is 824/1024 of the canvas, centred.
const ART = Math.round(SIZE * (824 / 1024));
const PAD = (SIZE - ART) / 2;

/** Apple's squircle (superellipse, n≈5) for a square of side `s` at (x, y). */
function squirclePath(ctx: CanvasRenderingContext2D, x: number, y: number, s: number) {
  const n = 5, half = s / 2, cx = x + half, cy = y + half, steps = 256;
  ctx.beginPath();
  for (let i = 0; i <= steps; i++) {
    const t = (i / steps) * Math.PI * 2;
    const c = Math.cos(t), sn = Math.sin(t);
    const px = cx + Math.sign(c) * Math.pow(Math.abs(c), 2 / n) * half;
    const py = cy + Math.sign(sn) * Math.pow(Math.abs(sn), 2 / n) * half;
    if (i === 0) ctx.moveTo(px, py); else ctx.lineTo(px, py);
  }
  ctx.closePath();
}

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

  // Ground: Apple-grid squircle (824/1024, centred), transparent margin around
  // it — identical footprint to the bundled icon.icns so the Dock tile doesn't
  // change size between running and quit.
  ctx.clearRect(0, 0, SIZE, SIZE);
  ctx.fillStyle = bg;
  squirclePath(ctx, PAD, PAD, ART);
  ctx.fill();

  // The "AY" wordmark, centred.
  ctx.font = `700 ${Math.round(ART * 0.42)}px -apple-system, BlinkMacSystemFont, "SF Pro Display", "Helvetica Neue", Arial, sans-serif`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.letterSpacing = `${Math.round(ART * 0.01)}px`;

  // Flat letterform — no glow/shadow (Mason 08-04: same call as the in-app
  // icons; the bloom read as muddy at Dock size).
  ctx.fillStyle = ink;
  ctx.fillText("AY", SIZE / 2, SIZE / 2 + ART * 0.012);

  const url = canvas.toDataURL("image/png");
  const comma = url.indexOf(",");
  return comma >= 0 ? url.slice(comma + 1) : null;
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
