# Blender MCP integration (Option 1: provision `uv`)

Started 2026-08-06. Mirrors the Premiere MCP plug-and-play pattern, but the
Blender server is **Python / uv**, not Node — so it needs a new provisioning
path (uv) parallel to `ensure_node`.

## Ground truth (verified from the real repo, not the marketing page)
- Repo: https://projects.blender.org/lab/blender_mcp  (v1.0.0, author dfelinto)
- Server dir `mcp/`: Python. `pyproject.toml` → package `blender-mcp`,
  `[project.scripts] blender-mcp = "blmcp:main"`, requires-python >=3.10,
  deps: docutils, mcp[cli]>=1.2.0, pyyaml.
- `manifest.json` (the .mcpb): `server.type = "uv"`,
  `mcp_config = { command:"uv", args:["run","blender-mcp"] }`,
  runtimes.python ">= 3.10".
- Needs a Blender ADD-ON installed inside Blender (drag-drop the Blender Lab
  repo + the add-on) — see https://lab.blender.org/mcp-server/#addon. Blender 5.1+.
- SECURITY: executes LLM-generated Python in Blender with NO guards
  (`execute_blender_code` tool). Recommend a VM / non-sensitive system.
- Read-only tools good for verify: `get_objects_summary`,
  `get_blendfile_summary_datablocks`, `get_object_detail_summary`,
  `get_python_api_docs`. Chosen verify_tool = **get_objects_summary** (no args).

## Launch decision
- `uv` is a single static binary that can bootstrap its own Python → keeps
  AYGENT's "install nothing" promise. Provision it into
  <app_data>/runtime/uv/ (parallel to runtime/node).
- Launch: `uv run --directory <cache> blender-mcp` OR `uvx blender-mcp`.
  Using `uv tool install blender-mcp` at enable time + `uv run blender-mcp`
  at launch, with uv's data/cache pinned under our runtime dir so nothing
  touches the user's system.

## Edits
1. provision.rs — ensure_uv(app, channel), uv_bin(app), uv_dir, and a
   provisioned PATH that includes uv. Pin UV_* env (UV_CACHE_DIR, UV_DATA_DIR,
   UV_PYTHON_INSTALL_DIR) under runtime/uv so Python installs land in our tree.
2. mcp.rs — McpConfig gains `needs_uv`. launch_spec handles needs_uv (uv on
   PATH + pinned UV_* env). Catalog entry `blender`. install_plan/uninstall_plan
   `blender` arms (uv tool install / uninstall). list() merge already generic.
3. lib.rs — mcp_enable: in needs_uv branch call provision::ensure_uv (parallel
   to ensure_node). mcp_list serializes needs_uv.
4. McpConnections.tsx — the hardcoded "Final step (in Premiere)" block becomes
   setup_note-driven so Blender shows its add-on step + security note.

## VERIFY LIVE (the discipline): enable against a real Blender 5.1+ with the
add-on loaded; confirm uv launch + handshake + one get_objects_summary call
BEFORE calling it done. Needs Mason at a Mac with Blender.

## STATUS 2026-08-06
- provision.rs: ensure_uv/uv_bin/uv_env/provisioned_uv_path/uv_uninstall added; UV_VERSION=0.12.2. DONE.
- mcp.rs: McpConfig.needs_uv; launch_spec uv branch; blender catalog entry (verify_tool=get_objects_summary); premiere+add_custom literals patched. DONE.
- lib.rs: mcp_enable ensure_uv branch; mcp_disable uv_uninstall on uninstall; mcp_list serializes needs_uv. DONE.
- McpConnections.tsx: needs_uv type; generic setup_note-driven verify block (was Premiere-hardcoded); whitespace pre-line for multi-line notes. DONE.
- cargo check: clean. tsc --noEmit: clean.
- NOT YET: cargo tauri build + LIVE verify against real Blender 5.1 + add-on (uv launch + handshake + get_objects_summary). This is the acceptance gate.
- install_plan/uninstall_plan: blender falls to _=>vec![] (empty). uv run blender-mcp fetches+caches the pkg on first launch; uninstall reclaims via uv_uninstall.
