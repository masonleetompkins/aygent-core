# UI-TASKS — Mason's list, 2026-08-01 (working doc, check off as shipped)

1. [x] Continuation reports should land IN THE ORIGINATING CHAT (not just Activity).
       Route: encode conv id in task_continue mailbox body envelope (conv:<id>\n<note>);
       run_headless_turn persists continues to that conv (preserving its title);
       UI reloads the open conv when a headless turn for it completes.
       Compact framing: "⏰ resumed: <note>" instead of "📨 from Continuation: …".
2. [x] Chat input grows with newlines but COVERS the last message — slide content up
       (scroll-to-bottom when input height changes).
3. [x] Chat window scrolls horizontally — lock to vertical; text must always wrap to width.
4. [x] Browser toggle: move Settings → Tools; rename "AYGENT Browser"; hide sidebar
       entry entirely when disabled.
5. [x] Plus (+) button left of chat box: attach ANY file (image/audio/video/etc) as context.
6. [x] Whisper transcription Tool (OpenAI API): available when OpenAI key set + enabled in Tools.
7. [x] Mic button (left of upload): record voice, transcribe into the input via Whisper.
8. [x] #tool tagging in the input, like @agent mentions (directly request a tool).

Order: 2+3 (CSS, fast) → 1 (routing) → 4 → 6 (Whisper is the base for 7) → 5 → 7 → 8.
