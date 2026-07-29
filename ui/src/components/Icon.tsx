// AYGENT icons — SF-Symbol-style line glyphs (inline SVG). NO emoji anywhere.
// Every glyph is a 24x24 stroked path that inherits `currentColor`, so it takes
// the accent/text color + any glow we apply via CSS. One consistent visual
// language across nav, the agent rail, and the agent picker.
import type { CSSProperties } from "react";

export type IconName =
  // nav
  | "chat" | "agents" | "tools" | "scheduler" | "connections" | "savepoints" | "settings"
  // agent-picker symbol set (SF-Symbol-ish)
  | "sparkles" | "brain" | "bolt" | "book" | "flask" | "briefcase" | "palette"
  | "chart" | "leaf" | "compass" | "rocket" | "star" | "terminal" | "globe"
  // ui affordances
  | "plus" | "trash" | "pencil" | "pin" | "pin-fill" | "close";

// The curated set offered as agent icons (order = swatch order in the picker).
export const AGENT_ICONS: IconName[] = [
  "sparkles", "brain", "bolt", "book", "flask", "briefcase",
  "palette", "chart", "leaf", "compass", "rocket", "star", "terminal", "globe",
];

// 24x24 viewBox paths. Stroked, round caps/joins — reads like SF Symbols.
const PATHS: Record<IconName, JSX.Element> = {
  chat: <path d="M21 15a2 2 0 0 1-2 2H8l-4 4V5a2 2 0 0 1 2-2h13a2 2 0 0 1 2 2z" />,
  agents: <><circle cx="12" cy="8" r="4" /><path d="M4 20c0-4 3.6-6 8-6s8 2 8 6" /></>,
  tools: <path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18v3h3l6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2-2z" />,
  scheduler: <><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 2" /></>,
  connections: <path d="M9 12a3 3 0 0 1 0-4l2-2a3 3 0 0 1 4 4l-1 1M15 12a3 3 0 0 1 0 4l-2 2a3 3 0 0 1-4-4l1-1" />,
  savepoints: <path d="M9 14L4 9l5-5M4 9h9a7 7 0 0 1 7 7v3" />,
  settings: <><circle cx="12" cy="12" r="3" /><path d="M12 2v3m0 14v3M4.2 4.2l2.1 2.1m11.4 11.4l2.1 2.1M2 12h3m14 0h3M4.2 19.8l2.1-2.1m11.4-11.4l2.1-2.1" /></>,

  sparkles: <path d="M12 3l1.8 4.9L18 9l-4.2 1.1L12 15l-1.8-4.9L6 9l4.2-1.1zM18 14l.9 2.4L21 17l-2.1.6L18 20l-.9-2.4L15 17l2.1-.6z" />,
  brain: <path d="M9 3a3 3 0 0 0-3 3 3 3 0 0 0-1 5 3 3 0 0 0 2 5 3 3 0 0 0 5 1 3 3 0 0 0 5-1 3 3 0 0 0 2-5 3 3 0 0 0-1-5 3 3 0 0 0-3-3 3 3 0 0 0-3 2 3 3 0 0 0-3-2z" />,
  bolt: <path d="M13 2L4 14h6l-1 8 9-12h-6z" />,
  book: <path d="M4 5a2 2 0 0 1 2-2h13v16H6a2 2 0 0 0-2 2zM19 3v16" />,
  flask: <path d="M9 3h6M10 3v6l-5 9a2 2 0 0 0 2 3h10a2 2 0 0 0 2-3l-5-9V3M7 15h10" />,
  briefcase: <><rect x="3" y="7" width="18" height="13" rx="2" /><path d="M8 7V5a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /></>,
  palette: <><circle cx="12" cy="12" r="9" /><circle cx="8" cy="10" r="1" /><circle cx="12" cy="8" r="1" /><circle cx="16" cy="10" r="1" /></>,
  chart: <path d="M4 20V4M4 20h16M8 16v-4m4 4V8m4 8v-6" />,
  leaf: <path d="M4 20C4 10 12 4 20 4c0 10-8 16-16 16zM4 20c4-6 8-8 12-9" />,
  compass: <><circle cx="12" cy="12" r="9" /><path d="M15.5 8.5l-2 5-5 2 2-5z" /></>,
  rocket: <path d="M12 3c4 2 6 6 6 11l-3 3H9l-3-3c0-5 2-9 6-11zM9 17l-2 4m8-4l2 4M12 10a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z" />,
  star: <path d="M12 3l2.6 6.3L21 10l-4.9 4.3L17.5 21 12 17.5 6.5 21l1.4-6.7L3 10l6.4-.7z" />,
  terminal: <><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M7 9l3 3-3 3M13 15h4" /></>,
  globe: <><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18" /></>,

  plus: <path d="M12 5v14M5 12h14" />,
  trash: <path d="M4 7h16M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2m-9 0l1 13a1 1 0 0 0 1 1h6a1 1 0 0 0 1-1l1-13" />,
  pencil: <path d="M4 20l4-1 11-11a2 2 0 0 0-3-3L5 16z" />,
  pin: <path d="M12 3l3 6 5 1-4 4 1 6-5-3-5 3 1-6-4-4 5-1z" />,
  "pin-fill": <path d="M12 3l3 6 5 1-4 4 1 6-5-3-5 3 1-6-4-4 5-1z" fill="currentColor" />,
  close: <path d="M6 6l12 12M18 6L6 18" />,
};

export function Icon({
  name, size = 18, stroke = 1.8, color, style,
}: {
  name: IconName; size?: number; stroke?: number; color?: string;
  style?: CSSProperties;
}) {
  // Flat symbols only — no shadow/glow. The accent color alone carries them
  // (Mason's call: glow read as muddy/unfinished).
  return (
    <svg
      width={size} height={size} viewBox="0 0 24 24"
      fill="none" stroke={color ?? "currentColor"} strokeWidth={stroke}
      strokeLinecap="round" strokeLinejoin="round"
      style={{ display: "block", flexShrink: 0, ...style }}
      aria-hidden="true"
    >
      {PATHS[name]}
    </svg>
  );
}
