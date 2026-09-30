# Changelog

## 1.1.0 — Linux fixes (AppImage rebuilt)
- Linux: the agent engine now starts. It was launched through macOS's `sandbox-exec`, which Linux doesn't have; it now runs in a Landlock jail (kernel 5.13+) matching the macOS Seatbelt profile: no access to your files, `/tmp` or other programs, only its own code and the Node runtime. On a kernel without Landlock it still runs, unconfined, and the app says so.
- Linux: the window no longer draws blank on some GPUs (WebKitGTK's DMA-BUF renderer is turned off by default).
- Linux: voice input works: the microphone permission is granted to the app's own UI, and recording falls back to WAV where WebKit has no working recorder. The AppImage now bundles GStreamer.
- Linux: the AppImage bundles its own Node.js runtime (v24.21.0, checksum-pinned), so there is nothing to install; the Landlock jail lets the engine run only that binary.
- One running copy: launching AYGENT again shows the running app instead of starting a second one (which ran the scheduler twice). The engine exits when the app does, even if the app is killed.
- Build: `cargo tauri build` works from the repo root, `src-tauri/` or `ui/`; the UI's IPC channel is no longer blocked by the content security policy.

## 1.1.0 — Core goes open
- Open-core release: desktop agent harness (macOS / Windows / Linux) under Apache-2.0.
- Agent loop with multiple chats, tools, memory, save points, scheduler, browser.
- BYOK cloud providers (Anthropic, OpenAI, OpenRouter, Muse) + local models (GGUF, MLX).
- OS-enforced folder jail (Seatbelt on macOS) with per-agent Allow Shell Access.
- See `CONTRIBUTING.md` for the Core-clean rule and how to submit PRs.
