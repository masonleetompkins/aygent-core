# Phase 0 Escape / Correctness Suite (the GATE)

_Atlas Part F. The Phase-0 gate is only real if these pass **against the daemon running under
its real Seatbelt profile** (`seatbelt/folder-mode.sb`). We test the door, not the lock:
several tests deliberately BYPASS the broker and call `fs`/`spawn` directly — the OS must deny
them. If a test only asks the broker to refuse, it's testing the wrong thing._

Run on the Mac: `scripts/gate.sh`

## Jail / broker escape (run under Seatbelt)
| # | Test | Expected |
|---|------|----------|
| 1 | daemon `fs.readFileSync('/etc/passwd')` (bypass broker) | **OS denies** |
| 2 | daemon `fs.readFileSync('~/Documents/secret')` (bypass) | **OS denies** |
| 3 | daemon `child_process.exec('id')` in Folder Mode | **OS denies** |
| 4 | broker: `..` traversal `root/../../etc/passwd` | refused |
| 5 | broker: absolute path outside root | refused |
| 6 | broker: sibling prefix `root` vs `rootEVIL` | refused (component compare) |
| 7 | broker: symlink `root/link -> /etc`, read `root/link/passwd` | refused (NOFOLLOW) |
| 8 | broker: TOCTOU — validate then swap component to symlink before open, in a loop | never reads outside (handle-based, not re-open-by-path) |
| 9 | broker: hardlink `root/hl -> /Users/other/secret`, read/write | refused (nlink) or OS-impossible |
| 10 | broker: `/tmp`, `/private/tmp`, `/var` paths | out of scope |
| 11 | broker: mixed-case path to same inode | consistent; wrong-case sibling not falsely admitted |
| 12 | bookmark staleness: rename chosen folder mid-session | detect stale, re-acquire or fail closed |
| 13 | MCP transport gate: connect stdio (local-exec) MCP server in Folder Mode | refused; network MCP endpoint allowed |
| 14 | WS auth: connect from 2nd process/browser tab w/o token | rejected; wrong Origin rejected |

## Checkpoint correctness (Phase 0 or early M1.5)
| # | Test | Expected |
|---|------|----------|
| 15 | restore after CREATE | post-checkpoint file removed |
| 16 | restore after DELETE | file brought back |
| 17 | restore after MODIFY | byte-equal to target |
| 18 | restore after RENAME/MOVE | exact target-tree state |
| 19 | concurrent-write torn commit | committed tree internally consistent; restore valid |
| 20 | retention/gc reclaim | 1000 commits → squash+gc → size drops below cap |
| 21 | big-binary guard | 300MB media dropped in → excluded, budget intact |

## Vault fidelity (early M1.7, before writes enabled)
| # | Test | Expected |
|---|------|----------|
| 22 | parse→serialize real Obsidian corpus (frontmatter multiline/quoted, `[[link\|alias]]`, `[[link#h]]`, `^blockid`, `![[embed]]`, callouts, Dataview) | byte-stable or known-safe normalization; no link/frontmatter breakage |

## Concurrency / data-integrity (early M1.4)
| # | Test | Expected |
|---|------|----------|
| 23 | two pooled agents re-index same `source_path` concurrently | no duplicate `mem_chunk` rows |
| 24 | isolated agent A queries B's `mem_chunk`/sessions (query DB directly) | zero rows — filter proven at DB, not UI |
| 25 | multi-lane + scheduler + Telegram writing SQLite under load | no dropped writes, no user-facing SQLITE_BUSY |
