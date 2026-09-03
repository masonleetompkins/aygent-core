# VIDEO TOOL v0.2 — spec (staging, committed)

Status: slice 2 — full track editor + agent prompt bar. No renderer. No new exec authority.

## What landed (v0.2 rewrites Video.tsx, backend unchanged)
- `ui/src/screens/Video.tsx` — agentic Premiere Pro, Diffusion-referenced, rebuilt on our stack:
  - Top bar: project picker, new-project field, 16:9 / 9:16 / 1:1 switch, save, Export…
  - Left: media/assets panel (clip usage counts) + pipeline card (captions preset, LUT, enhance); add-clip at playhead.
  - Center: preview canvas (active clips at playhead, black-frame warning, timecode + scene size), transport (play/pause, scrub, zoom), multi-track timeline V2/V1/T1/A1/A2 with drag-move, edge-trim (sourceIn/out follow), click-select, split at playhead, delete.
  - Right: inspector (label, src, title text, start/end/in/out, track, volume dB, muted, hidden, duration + id) + agent bar (prompt field wired to the selected agent via agent_stream reading/writing composition.json, plus silence+take and transcribe presets).
  - Old v0.1 compositions normalize into tracks on load (default V1, 4s blocks).
- Backend (`video.rs`, 4 commands) + nav/icon/App/lib wiring unchanged from v0.1.
- Docs: `docs/CAPABILITIES.md` changelog entry (staging copy).

## Composition schema (v0.2, agent <-> UI contract)
```jsonc
{
  "scene": { "width": 1920, "height": 1080, "fps": 30, "background": "#000" },
  "sequence": [
    { "id": "c1", "track": "V1", "src": "a-roll.mp4", "name": "take 3",
      "start": 0, "end": 12.5, "sourceIn": 2.0, "sourceOut": 14.5,
      "volume": 0, "muted": false, "hidden": false,
      "x": 0, "y": 0, "width": 1920, "height": 1080, "opacity": 1, "text": "title?" }
  ],
  "captions": { "preset": "hyperframes", "animated": true },
  "color": { "lut": "brand.cube", "sCurve": true, "adjustmentLayer": true },
  "audio": { "enhance": "auphonic", "ducking": true, "musicBed": "music/bed.mp3" },
  "exports": [
    { "size": "1920x1080", "bitrate": "12M", "codec": "avc" },
    { "size": "1080x1920", "bitrate": "8M", "codec": "avc" }
  ]
}
```
Tracks: V2 overlays / V1 a-roll / T1 titles / A1 audio / A2 music.
Timing mirrors Diffusion: start/end = placement; sourceIn/sourceOut = source window (rate 1 in UI math).
Agent edits = read_file + write_file on Video/<project>/composition.json, ids stable.
Renders run in Pro Mode shell + ffmpeg/HyperFrames — the tab never executes anything.

## Security
- No new capabilities. `video_*` resolve every path through `broker.resolve`
  (per-agent scope, fail-closed). `video_save` is a Write-mode resolve + bounded
  (object only, ≤2MB). Slug guard (`[A-Za-z0-9-_]`, ≤80) on project names.
- Agent bar calls the existing `agent_stream` command (no new tool, no new reach).
- No daemon changes, no new tools in the agent loop, no CONTRACTS change.

## Next slices (not started)
1. `cutlist` helper: transcribe → silence drop + take-picker (keep last / longest coherent).
2. Caption preset port from Hyperframes look → native preset.
3. LUT (.cube) + S-curve as ffmpeg filter chain; adjustment-layer ordering.
4. Audio: enhance round-trip + music-bed auto-duck keyframes.
5. Reframe helper: 16:9 → 9:16 (objectFit cover + reframed x/y).
6. RVM matte stage (separate repo, MIT) → foreground layer for text-behind-subject.

## QA (stage bundle, built 2026-09-03 ~14:04)
1. Relaunch stage bundle → Video tab shows editor (timeline + inspector + agent bar), not the v0.1 file list.
2. Open project → clips land on tracks; drag body = move; drag edges = trim (in/out follow).
3. Split at playhead on a selected clip → two clips, ids stable, source windows contiguous.
4. Inspector edit (src/times/track/volume/muted/hidden) → unsaved pill → Save → composition.json updates.
5. Agent bar: "silence+take" preset → agent_stream rewrites composition.json → timeline updates on reload.
6. 16:9 / 9:16 / 1:1 switch resizes preview + scene; Export… drops a render brief into the agent box.
7. `cargo tauri build` exit 0, 9 warnings (pre-existing dead-code), 0 errors.
8. Regression: Chat turn with file tools, Sparks tab, Browser tab all unaffected.
