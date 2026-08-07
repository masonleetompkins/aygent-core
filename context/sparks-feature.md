# Sparks — interactive AI-built mini-apps (feature #4)

Built 2026-08-06. Named "Sparks" (Mason's pick). Option A + data-embedding:
the Spark is sandboxed and CANNOT call the agent/files/machine, but the agent
EMBEDS data into it at build time (const DATA = {...}).

## What a Spark is
A single self-contained `Sparks/<slug>/index.html` in the agent folder (jailed),
rendered live in the Sparks tab inside a SANDBOXED iframe:
`sandbox="allow-scripts allow-popups allow-forms"` (NO allow-same-origin) via
srcDoc. Runs its own JS + CDN libs + embedded data; isolated from app/FS/agent.

## Pieces shipped
- **Backend (lib.rs):** sparks_list / sparks_read / sparks_delete — all jailed
  through the broker. list scans Sparks/<slug>/ for index.html (+ optional
  spark.json: title/description/created). read returns the HTML string the UI
  injects as srcDoc. delete removes the slug dir (with a path-mismatch guard).
  Registered in the invoke_handler.
- **Embedded skill (lib.rs):** SPARKS_INSTRUCTIONS const, appended to every real
  agent turn's system prompt (right after dashboard tool_instructions). Teaches:
  slug rules, one self-contained index.html, CDN OK, DATA-embedded-at-build-time
  (gather with tools first, then inline as JS literal — Spark never calls back),
  write spark.json for the library, rewrite same slug to edit, no secrets.
  NOT a fake tool — just instructions over the existing jailed write_file.
- **UI:** screens/Sparks.tsx (library rail + live sandboxed iframe stage +
  delete + empty-state with example prompts + "Go to Chat"). Sidebar "Sparks"
  entry (icon: bolt, group: agent, after Skills). App route wired. ScreenId +.

## Edit loop
"Ask Chat to edit" — the agent rewrites Sparks/<slug>/index.html (same slug);
the panel reloads on reselect/refresh. (A future nicety: live file-watch reload.)

## Checks: cargo check clean, tsc --noEmit clean.
## NOT yet: live end-to-end test (ask an agent to actually build a Spark and
confirm it renders). Do at bundle-test time.
