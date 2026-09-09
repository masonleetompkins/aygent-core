# AYGENT Video — v0.3 (agentic NLE)

The Video tab is a full editor (Diffusion-Studio-style layout, AYGENT theming) plus an
agent dock. Human and agent edit **the same file**: `Video/<project>/composition.json`.

```
Video/<project>/
  composition.json   the edit (schema v3 below)
  assets.json        imported media manifest (id, name, kind, rel, path, linked, probe, thumbs)
  transcript.json    word-level transcript (Whisper verbose_json; asset-source time)
  chat.json          the agent dock conversation (msgs + provider history)
  media/             HARDLINKS to source footage (never copies) + generated audio/mattes
  luts/              .cube LUTs (hardlinked; copied only if cross-volume — tiny files)
  .cache/            filmstrips, waveforms, captions.ass, frame renders, filter scripts
  renders/           exports
  .gitignore         media/ .cache/ renders/  (keeps Save Points lean)
```

## Layout
`top bar · tool rail | panel | player + timeline | dock`
- Rail: Media · Text · Captions · Color · Audio · Export (panel toggles).
- Player: real playback via `aygent-media://` (one `<video>` per active clip, synced to the
  playhead), live text/caption overlays, **Render frame** = exact ffmpeg composite.
- Timeline: V3/V2/V1/T1/A1/A2 (+VN by typing a track in the inspector), ruler scrub,
  snapping, filmstrips + waveforms, move/trim (source in/out follow, speed-aware),
  razor, marquee, multi-select, undo/redo, drag from Media or Finder.
- Dock: **Agent** (streaming chat, tool cards, per-project persistence, live reload) ·
  **Inspector** (timing, transform, text, audio, clip grade).
- Keyboard: Space · J/K/L · ←/→ (⇧ = 1s) · Home/End · K / ⌘K split · ⌫ / ⇧⌫ ripple ·
  ⌘D · ⌘Z/⇧⌘Z · V/C tools · S snap · +/− · ⇧Z fit · [ ] edit points · I inspector ·
  ⌘⏎ focus agent · ⌘S save.

## Backend (src-tauri)
| file | purpose |
| --- | --- |
| `video.rs` | project store; hardlink import (EXDEV → reference); ffprobe; filmstrip/wave thumbs; LUT import; commands `video_status/projects/create/load/save/chat_save/pick_media/import_paths/remove_asset/refresh_thumbs/list_luts/pick_lut/reveal` |
| `video_render.rs` | composition v3 schema + v0.2 upgrade; ffmpeg filter graph (layers, transforms, fades, drawtext titles, ASS captions "pop/karaoke/plain", per-clip + global grade: eq/colorbalance/curves S-curve/lut3d, matte via alphamerge, audio: trims/atempo/volume envelopes/adelay/amix/sidechain duck/loudnorm); `video_render` (VideoToolbox → libx264 fallback, progress events, cancel), `video_frame`, `video_validate`, `video_list_renders`, `video_caption_lines` |
| `video_media.rs` | `aygent-media://localhost/<agent>/<project>/{asset|cache|render|lut}/<name>` — jailed, Range (206), 8 MB chunks |
| `video_tools.rs` | agent tools `video_project · video_edit · video_transcribe · video_silences · video_takes · video_auto_cut · video_frame · video_render · video_audio_enhance · video_matte`; `video_tool` command = same core for UI buttons; `video_set_auphonic` |

Dispatch is wired in all four agent loops (Anthropic, OpenAI, local, headless) next to
`dashboard::is_dashboard_tool`. Schemas + instructions are added in `agent_tools_for_full`.

## Composition v3
```jsonc
{ "version": 3,
  "scene": { "width": 1920, "height": 1080, "fps": 30, "background": "#000000" },
  "clips": [ { "id", "track": "V1", "type": "video|audio|image|text", "asset", "audioAsset",
      "name", "start", "end", "in", "out", "speed", "volume", "muted", "hidden", "fit",
      "transform": { "x", "y", "scale", "rotation", "opacity" },
      "text": { "content", "font", "size", "weight", "color", "align", "x", "y", "bg", "bgOpacity", "padding", "animation", "shadow", "maxWidth" },
      "color": { "lut", "lutIntensity", "sCurve", "exposure", "contrast", "saturation", "temperature" },
      "audio": { "fadeIn", "fadeOut", "keyframes": [{ "t", "db" }] },
      "behindSubject", "transitionIn": { "kind", "duration" }, "transitionOut" } ],
  "captions": { "enabled", "preset": "pop|karaoke|plain", "font", "weight", "size", "color", "keyColor", "y", "wordsPerLine", "maxChars", "uppercase", "behindSubject", "sourceAsset", "keyWords": [], "shadow" },
  "audio": { "duck": { "enabled", "musicDb", "duckedDb", "attack", "release" }, "enhance", "masterDb", "loudnorm" },
  "color": { ...grade },  "adjustmentLayer": true,
  "matte": { "enabled", "sourceAsset", "alphaAsset", "feather" },
  "exports": [ { "name", "width", "height", "bitrate", "codec": "h264|hevc|prores", "fps" } ] }
```
Timing: `start/end` = timeline placement; `in/out` = source range; `out = in + (end-start)*speed`.
Stacking: V1 < V2 < V3 … < T1 < captions < adjustment layer. A2 = music (ducked under everything else).
Transcript words are in **asset** time and are remapped through every clip that plays that asset.

## Workflow (Mason)
import A-roll (hardlink) → **Rough cut** (`video_auto_cut`: silencedetect + take detection,
keep LAST) → review selects → transcribe → titles/graphics (T1/V2/V3) → captions (pop) →
Color: LUT + S-curve on the adjustment layer → Audio: Auphonic / local enhance + duck →
Export presets (any size) → optional `video_matte` (RVM via uv) for text-behind-subject.

## Security
- Every path goes through `broker.resolve`. Media reads of hardlinks are admitted (rule 7
  only refuses WRITES to nlink>1). Cross-volume sources are served only if listed in
  `assets.json` (manifest is inside the jail and only import writes it).
- `aygent-media://` serves exactly four folders/kinds; single path component; no `..`.
- No new exec authority for the agent: tools call the provisioned ffmpeg/ffprobe/uv with fixed
  argument shapes; filter graphs are written to a script file (no shell).
- CSP: `media-src`/`img-src` allow `aygent-media:`.
- Auphonic credentials live in the Keychain (`auphonic`).

## Not yet
- Keyframed transforms (position/scale over time) — agent can emulate with splits.
- Audio waveform *editing* (keyframes are agent-set, UI shows/clears).
- Auto-reframe helper (agent does it with `update_clip transform.x`).
- Preview LUT (CSS approximates exposure/contrast/saturation; "Render frame" is exact).

## QA (stage bundle)
1. Video tab → welcome card → create project → Media Import (native picker) → thumbs appear, first clip lands on V1, plays.
2. Drag from Finder onto the editor → hardlinked (ls -l shows link count 2).
3. Scrub, J/K/L, split, trim edges, drag between V1/V2, marquee, ⌘Z.
4. Agent dock: "Rough cut" → tool cards stream, timeline updates live, chat persists across reopen.
5. Captions: Transcribe → enable → preview words pop; Render frame shows libass result.
6. Color: pick .cube → Render frame shows the LUT. Export → progress → renders/ list → reveal.
7. Light / dark / neutral / matrix themes all legible.
