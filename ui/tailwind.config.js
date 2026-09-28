/** @type {import('tailwindcss').Config} */
export default {
  darkMode: ["class", '[data-theme="dark"]'],
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      // All colors reference the CSS variables in theme.css (DESIGN.md tokens).
      // Components NEVER hardcode a hex — they use these token-backed names so
      // light/dark/accent flow automatically.
      colors: {
        bg: "var(--bg)",
        surface: "var(--surface)",
        text: "var(--text)",
        "text-muted": "var(--text-muted)",
        "text-faint": "var(--text-faint)",
        line: "var(--line)",
        accent: "var(--accent)",
        ok: "var(--ok)",
        danger: "var(--danger)",
        warn: "var(--warn)",
      },
      borderRadius: {
        card: "var(--radius-card)",
        control: "var(--radius-control)",
        pill: "var(--radius-pill)",
      },
      borderWidth: {
        DEFAULT: "var(--border-width)",
      },
      boxShadow: {
        // The lighting: soft shadow (light) / glow (dark) / accent-tinted.
        elevation: "var(--elevation)",
        "elevation-hover": "var(--elevation-hover)",
      },
      fontFamily: {
        sans: ["-apple-system", "SF Pro Text", "system-ui", "sans-serif"],
        mono: ["ui-monospace", "SF Mono", "Menlo", "monospace"],
      },
    },
  },
  plugins: [],
};
