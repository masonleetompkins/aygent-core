# AYGENT — Design System

_The single source of truth for AYGENT's UI. Every screen references this; nothing drifts.
Written 2026-07-23 (first Phase-1 artifact). Decisions locked with Mason._

---

## The core idea: THE LIGHTING IS THE THEME

AYGENT's signature is that **the shape system is constant** (soft shadows/glows + outlines +
rounding) and the **theme controls the *lighting*, not just the colors**:

- **Light mode (default):** white bg, black text, black outlines, **big soft drop shadows**.
- **Dark mode:** black bg, white text, and shadows **become glows** (same soft diffuse feel,
  inverted — a white glow where the light-mode shadow was).
- **Accent color (user-picked):** recolors the **shadows/glows AND outlines** to the accent.
  Not accent-on-buttons — accent-as-*lighting*. The whole UI takes on the user's color.

Mental model: one shape language; `light|dark` flips shadow↔glow; `accent` tints the lighting.
This is implemented as **CSS variables** so a theme change is a token swap, never a rewrite.

---

## Tokens (CSS variables — the one source of truth)

```css
:root {
  /* ---- shape (constant across themes) ---- */
  --radius-card: 14px;
  --radius-control: 8px;   /* buttons, inputs */
  --radius-pill: 999px;
  --border-width: 1.5px;
  --space: 8px;            /* base unit; use multiples (8/16/24/32) */

  /* ---- LIGHT MODE (default) ---- */
  --bg: #ffffff;
  --surface: #ffffff;
  --text: #0a0a0a;
  --text-muted: #5c5c5c;
  --text-faint: #9a9a9a;
  --line: #0a0a0a;                 /* outlines (accent overrides this) */
  --accent: #0a0a0a;               /* default accent = black; user picks in Settings */

  /* the "lighting" — a big soft drop shadow in light mode */
  --elevation:
    0 2px 8px rgba(0,0,0,0.06),
    0 8px 24px rgba(0,0,0,0.10);
  --elevation-hover:
    0 4px 12px rgba(0,0,0,0.10),
    0 12px 32px rgba(0,0,0,0.14);

  /* ---- semantic (theme-independent hues, tuned per mode) ---- */
  --ok: #1f9d55;       /* admit / success */
  --danger: #d64545;   /* refused / error */
  --warn: #c77d1a;     /* Pro-Mode / caution */
}

/* ---- DARK MODE: flip shadow -> glow ---- */
:root[data-theme="dark"] {
  --bg: #000000;
  --surface: #0b0b0b;
  --text: #ffffff;
  --text-muted: #a6a6a6;
  --text-faint: #666666;
  --line: #ffffff;
  --accent: #ffffff;               /* default; user pick overrides */

  /* lighting becomes a GLOW (light emitted, not shadow cast) */
  --elevation:
    0 0 10px rgba(255,255,255,0.10),
    0 0 28px rgba(255,255,255,0.06);
  --elevation-hover:
    0 0 14px rgba(255,255,255,0.16),
    0 0 36px rgba(255,255,255,0.10);

  --ok: #34d17e;
  --danger: #ff6b6b;
  --warn: #e0a53f;
}

/* ---- ACCENT applied (set --accent-rgb from the picker, JS writes it) ---- */
/* When a user picks an accent, JS sets --accent + --accent-rgb, and we retint
   both the outline (--line) and the lighting (--elevation) to the accent.
   Light mode = accent-tinted shadow; dark mode = accent-tinted glow. */
:root[data-accent="on"] {
  --line: var(--accent);
  --elevation:
    0 2px 10px rgba(var(--accent-rgb), 0.20),
    0 10px 30px rgba(var(--accent-rgb), 0.16);
  --elevation-hover:
    0 4px 14px rgba(var(--accent-rgb), 0.28),
    0 14px 38px rgba(var(--accent-rgb), 0.22);
}
:root[data-theme="dark"][data-accent="on"] {
  --line: var(--accent);
  --elevation:
    0 0 12px rgba(var(--accent-rgb), 0.35),
    0 0 34px rgba(var(--accent-rgb), 0.20);
  --elevation-hover:
    0 0 16px rgba(var(--accent-rgb), 0.5),
    0 0 44px rgba(var(--accent-rgb), 0.3);
}
```

**Theme switching (Settings):**
- Light/dark → set `data-theme="light|dark"` on `:root`.
- Accent → JS sets `--accent` (hex) + `--accent-rgb` (`"r,g,b"`) + `data-accent="on"`.
- All persisted to SQLite (per the no-config-files rule). Default: light, accent = black/white.

---

## Typography
- **UI:** `-apple-system, "SF Pro Text", system-ui, sans-serif` — native macOS, fast, premium.
- **Mono:** `ui-monospace, "SF Mono", Menlo, monospace` — paths, code, transcripts, keys, hashes.
  The mono texture is deliberate: it signals "technical honesty" (this is where the real work
  is shown — file paths, tool calls, diffs).
- **Scale:** display 28 / title 20 / body 15 / small 13 / mono 13. Weight: 400 body, 600–700
  headings/buttons. Generous line-height (1.5 body).

## Shape & feel
- **Rounding:** cards `14px`, controls `8px`, pills `999px`.
- **Outlines:** `1.5px` solid `var(--line)` — every frame/box/button has a crisp outline.
- **Elevation:** `var(--elevation)` (soft shadow in light, glow in dark, accent-tinted when set).
  This is the whole aesthetic — flat fills, defined by outline + soft lighting, NOT gradients.
- **Density:** generous padding (16–24px in cards), real breathing room. Calm, not cluttered.
- **Motion:** minimal + purposeful — 150ms ease fades, gentle elevation lift on hover. No bounce.
- **Personality:** clean / neutral / pro (Linear/Raycast/Things restraint). Content leads; the
  chrome is quiet. The one flourish is the lighting system.

## Component conventions
- **Card:** `--surface` fill, `1.5px --line` outline, `14px` radius, `--elevation`, 20–24px pad.
- **Button (primary):** filled `--accent`, contrast text, `8px` radius, `--elevation`, hover lifts.
- **Button (secondary):** `--surface` fill, `--line` outline, same shape.
- **Input:** `--surface`, `--line` outline, `8px`, mono for path/key fields.
- **Toggle / switch:** track uses `--line`; on-state fills `--accent`.
- **Pill / badge:** `999px`, thin outline; status pills tint with `--ok`/`--danger`/`--warn`.
- **Transcript / code:** mono, `--surface`, subtle inset, scrollable, wrapped (never overflow).

## Stack (locked)
- **Tailwind CSS** for styling (tokens above wired into `tailwind.config` + a `theme.css`).
- **shadcn/ui** components (copied into the repo → we own + restyle them to these tokens).
  Accessible interactive primitives (dropdown, dialog, switch, slider, tabs) come correct;
  we theme them ONCE via the CSS variables so light/dark/accent flows everywhere free.
- Everything reads from the CSS variables — no hardcoded hex anywhere in components.

## The 7 screens (all inherit this system)
Onboarding · Chat · Agents switcher · **Settings (THE WEDGE — most polish)** · Scheduler ·
Connections · Checkpoints timeline.

---
_Change rule: colors/shape/lighting live ONLY as tokens here. A component must never hardcode
a hex or a shadow — it references a variable. Adjust the system in one place, everything follows._
