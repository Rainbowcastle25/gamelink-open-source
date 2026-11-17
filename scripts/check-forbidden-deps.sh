#!/usr/bin/env bash
# Fails if a production build would link code GameLink promises never to ship:
# SSH / SFTP servers, UPnP / NAT-PMP / PCP port mapping, exit-node or subnet
# routing support. Run from the repository root with the Go toolchain on PATH.
set -euo pipefail
cd "$(dirname "$0")/.."

TAGS="$(tr -d '\r\n' < build-tags.txt)"

# Regular expressions matched against the full import path of every dependency.
FORBIDDEN='^(github\.com/pkg/sftp|github\.com/tailscale/gliderssh|golang\.org/x/crypto/ssh|tailscale\.com/ssh/|tailscale\.com/net/portmapper$|tailscale\.com/feature/portmapper|github\.com/huin/goupnp|github\.com/jackpal/go-nat-pmp|tailscale\.com/feature/relayserver|tailscale\.com/wgengine/router/osrouter|github\.com/gamelink-project/gamelink/tests/)'

status=0
command -v go >/dev/null 2>&1 || { echo "go is not on PATH; cannot check dependencies" >&2; exit 2; }

check() { # <goos> <package>
  local goos="$1" pkg="$2" deps hits
  # Run go list on its own so a failure stops the check instead of looking
  # like "no forbidden packages found".
  if ! deps="$(CGO_ENABLED=1 GOOS="$goos" GOARCH=amd64 go list -tags "$TAGS" -deps "$pkg")"; then
    echo "go list failed for $pkg ($goos); cannot verify it" >&2
    status=1
    return
  fi
  hits="$(printf '%s\n' "$deps" | grep -E "$FORBIDDEN" || true)"
  if [ -n "$hits" ]; then
    echo "FORBIDDEN code is linked into $pkg ($goos):" >&2
    echo "$hits" | sed 's/^/    /' >&2
    status=1
  else
    echo "ok: $pkg ($goos) links no SSH/SFTP/port-mapping/router code"
  fi
}

check linux   ./shim/agent
check linux   ./shim/desktop
check windows ./shim/desktop

# The tag list must not drift from Tailcat's own release list (we add exactly
# ts_omit_ssh on top of it).
TAILCAT_DIR="$(go list -m -f '{{.Dir}}' github.com/tailscale/tailcat)"
EXPECT="$(tr -d '\r\n' < "$TAILCAT_DIR/build-tags.txt"),ts_omit_ssh"
if [ "$TAGS" != "$EXPECT" ]; then
  echo "build-tags.txt is out of sync with Tailcat's build-tags.txt (+ts_omit_ssh)." >&2
  echo "Regenerate: printf '%s,ts_omit_ssh' \"\$(cat \"$TAILCAT_DIR/build-tags.txt\")\" > build-tags.txt" >&2
  status=1
else
  echo "ok: build-tags.txt matches Tailcat's list plus ts_omit_ssh"
fi
exit $status
