#!/usr/bin/env bash
# Cheap guard against committing real key material or invitations.
# Fails on files that would be committed and contain private-key-shaped
# strings, real-looking invitations, or any non-placeholder `nodekey:` value.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# Existing files only (tracked or untracked-but-not-ignored); skip binaries/locks.
FILES=()
while IFS= read -r -d '' f; do
  [[ -f "$f" ]] || continue
  [[ "$f" =~ (^|/)(package-lock\.json|go\.sum)$|\.(png|ico|icns|svg|woff2?|exe)$ ]] && continue
  FILES+=("$f")
done < <(git ls-files -c -o --exclude-standard -z | sort -zu)

# Frozen Go-parity inputs contain generated, non-production invitations and
# public keys. Exempt only the reviewed bytes, so editing the fixture restores
# normal scanning instead of creating a permanent blind spot for this path.
fixture=core/fixtures/pure.json
fixture_sha=624a0034e90ab6373cd0dc6b9a06d79141d8c561e71738eb9fac1081a63e406d
if [ -f "$fixture" ] && [ "$(sha256sum "$fixture" | cut -d ' ' -f 1)" = "$fixture_sha" ]; then
  filtered=()
  for f in "${FILES[@]}"; do
    [ "$f" = "$fixture" ] || filtered+=("$f")
  done
  FILES=("${filtered[@]}")
fi

status=0
scan() { # <label> <regex> [allowed-regex]
  local label="$1" re="$2" allow="${3:-}" out code f
  for f in "${FILES[@]}"; do
    code=0
    # Match tokens individually: a placeholder elsewhere on the same line
    # cannot exempt a real key. Never print matched material, even truncated.
    out="$(grep -IoE -- "$re" "./$f")" || code=$?
    if [[ $code -gt 1 ]]; then
      printf 'cannot scan %q\n' "$f" >&2
      status=1
      continue
    fi
    if [[ -n "$allow" && -n "$out" ]]; then
      out="$(printf '%s\n' "$out" | grep -vEx -- "$allow" || true)"
    fi
    if [[ -n "$out" ]]; then
      printf 'possible secret (%s) in %q (content redacted)\n' "$label" "$f" >&2
      status=1
    fi
  done
}

# Private key formats used by Tailscale/Tailcat and PEM.
scan "private key" 'privkey:[0-9a-f]{64}|BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY'
# A GameLink invitation: prefix followed by a long base64url body.
scan "invitation" 'glink1\.[A-Za-z0-9_-]{60,}'
# Public node keys are not secret, but real ones do not belong in the repo.
# Only obvious placeholders are allowed: one hex digit repeated 64 times
# (test keys such as all zeros or all `a`s), or 0123456789abcdef x4.
scan "non-placeholder nodekey" 'nodekey:[0-9a-f]{64,}' 'nodekey:(([0-9a-f])\2{63}|(0123456789abcdef){4})([^0-9a-f]|$)'

if [ "$status" -eq 0 ]; then
  echo "ok: no key material or invitations found in ${#FILES[@]} files"
fi
exit "$status"
