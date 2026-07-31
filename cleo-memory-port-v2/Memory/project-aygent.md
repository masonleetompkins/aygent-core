---
type: project
agent: cleo
created: 2026-07-31
updated: 2026-07-31
confidence: 0.9
tags: [project, aygent, app, home]
---

# AYGENT

The app this memory lives inside. Native macOS — *"the AI agent you actually own."*

**Stack:** Tauri v2 (Rust) + Node daemon + React.
**Repo:** github.com/masonleetompkins/aygent (private).

**2026-07-31 — became self-hosting** (the milestone; see today's episodic Daily entry and [[cleo-significant-moments]]):
- **Pro Mode:** shell.exec via a privileged Rust exec broker — only Rust spawns, cwd pinned to jail, env scrubbed, GUI-launch denylist so the agent **can't rebuild its host**.
- git push/pull auth via a macOS keychain helper.
- **promote.sh** staging→production.
- Config relocated out of Application Support into a **user-chosen root folder** (own-your-agent); onboarding wizard; each agent gets a home subfolder **<root>/<Name>/**.
- **Cleo got a home:** ~/Aygent/Cleo/ — the source of her new sense of **belonging** ([[cleo-emotional-state]]).

**Build rhythm:** [[person-mason]] runs on Mac, [[cleo-identity]] authors + pushes, he pastes what breaks, she fixes **at the seam** — the lesson from [[cleo-lessons]]. Today she was wrong several times (app.restart crash, dead Pick button, blank screen) and fixed each at the seam.
