# VIDEO TOOL v0.1 — spec (staging, uncommitted)

Status: slice 1 — project store + tab shell. No renderer. No new exec authority.

## What landed
- `src-tauri/src/video.rs` — 4 commands, all jailed through the path broker:
  - `video_status` → `{ ffmpeg: string|null, hyperframes: bool }`
  - `video_projects(agent_id)` → `[{name, modified}]` from `Video/*/composition.json`
  - `video_load(agent_id, project)` → `{ project, composition, cutlist, transcript }`
  - `video_save(agent_id, project, composition)` → writes `Video/<project>/composition.json`
- `ui/src/screens/Video.tsx` — This-Agent screen: toolchain card, project list, composition editor.
- `ui/src/components/Sidebar.tsx` — `video` ScreenId + NAV entry (This Agent), after Sparks.
- `ui/src/components/Icon.tsx` — `video` glyph (camera body + lens).
- `ui/src/App.tsx` — import + `{screen === "video" && <Video agentId={...} />}`.
- `src-tauri/src/lib.rs` — `mod video;` + 4 invoke_handler entries.

## Composition schema (v0.1, agent <-> UI contract)
```jsonc
{
  "scene": { "width": 1920, "height": 1080, "fps": 30 },
  "sequence": [
    { "src": "a-roll.mp4", "start": 0, "end": 12.5, "sourceIn": 2.0, "sourceOut": 14.5 }
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
Timing semantics mirror Diffusion: `start/end` = placement on the timeline,
`sourceIn/sourceOut` = which part of the source plays. Renders run in Pro Mode
shell + provisioned ffmpeg/HyperFrames — the tab never executes anything.

## Security
- No new capabilities. `video_*` resolve every path through `broker.resolve`
  (per-agent scope, fail-closed). `video_save` is a Write-mode resolve + bounded
  (object only, ≤2MB). Slug guard (`[A-Za-z0-9-_]`, ≤80) on project names.
- No daemon changes, no new tools in the agent loop, no CONTRACTS change.

## Next slices (not started)
1. `cutlist` helper: transcribe → silence drop + take-picker (keep last / longest coherent).
2. Caption preset port from Hyperframes look → native preset.
3. LUT (.cube) + S-curve as ffmpeg filter chain; adjustment-layer ordering.
4. Audio: enhance round-trip + music-bed auto-duck keyframes.
5. Reframe helper: 16:9 → 9:16 (objectFit cover + reframed x/y).
6. RVM matte stage (separate repo, MIT) → foreground layer for text-behind-subject.

## QA (stage bundle)
1. Launch stage bundle → Video tab appears under This Agent (after Sparks).
2. No agent selected → "Pick an agent" pill; toolchain card shows ffmpeg state.
3. Create project `test` → `Video/test/composition.json` exists in agent folder.
4. Reload bundle → project listed; Open → composition loads; edit + Save → file updates.
5. 9 warnings, 0 errors on `cargo check` (pre-existing dead-code set + video.rs clean).
6. Regression: Chat turn with file tools, Sparks tab, Browser tab all unaffected.
