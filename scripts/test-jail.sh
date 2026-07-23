#!/usr/bin/env bash
# Diagnostic: launch the daemon under the Seatbelt jail BY HAND so we can see
# its real stderr (Rust swallows it when the jailed process dies at boot) AND
# capture the exact macOS sandbox denials. Run from the repo root on the Mac.
set -uo pipefail

NODE_BIN="$(which node)"
NODE_BIN="$(readlink -f "$NODE_BIN" 2>/dev/null || echo "$NODE_BIN")"
NODE_PREFIX="$(dirname "$(dirname "$NODE_BIN")")"
# Allow the WHOLE daemon package (dist/ + node_modules/), not just dist/,
# or node can't load its deps (ws/index.js) -> EPERM at boot.
DAEMON_DIR="$(cd daemon && pwd)"
DAEMON_ENTRY="$DAEMON_DIR/dist/index.js"
PROFILE="/tmp/aygent-jail-test.sb"

echo "==> node: $NODE_BIN"
echo "==> node prefix: $NODE_PREFIX"
echo "==> daemon dir: $DAEMON_DIR"

# Materialize the profile (same substitutions the Rust supervisor does).
sed -e "s#<<NODE_BIN>>#$NODE_BIN#g" \
    -e "s#<<APP_BUNDLE_SUBPATH>>#$DAEMON_DIR#g" \
    seatbelt/folder-mode.sb > "$PROFILE"
# also allow the node prefix tree (dylibs/ICU)
echo "(allow file-read* (subpath \"$NODE_PREFIX\"))" >> "$PROFILE"

echo "==> profile written to $PROFILE"
echo "==> launching daemon under jail (its stderr is shown directly):"
echo "-----------------------------------------------------------------"

AYGENT_WS_TOKEN=diagtoken \
AYGENT_BROKER_PORT=1 \
AYGENT_BROKER_TOKEN=diagtoken \
sandbox-exec -f "$PROFILE" "$NODE_BIN" "$DAEMON_ENTRY"

echo "-----------------------------------------------------------------"
echo "==> daemon exited with code $?"
echo "==> recent sandbox denials for node:"
log show --last 2m --style syslog --predicate 'process == "node" AND eventMessage CONTAINS "deny"' 2>/dev/null | tail -30
