#!/usr/bin/env bash
# Run this same non-network CLI flow natively and under qemu-aarch64.
set -euo pipefail
binary="$1"; shift
runner=("$@")
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
agent() { "${runner[@]}" "$binary" --dir "$dir/agent" "$@"; }
agent --help >/dev/null
agent --version
agent init >/dev/null
agent games add --type minecraft-java --id minecraft-java --relay-host derp.example.com >/dev/null
agent games list | grep minecraft-java >/dev/null
agent games list --json | grep '"games":\[{"approvedDevices":0,.*"id":"minecraft-java"' >/dev/null
key="nodekey:$(printf '0123456789abcdef%.0s' {1..4})"
agent devices approve --game minecraft-java --key "$key" --label smoke >/dev/null
# Invitation stdout is a secret, even in a smoke test. Discard it.
agent invite --game minecraft-java >/dev/null 2>/dev/null
agent status | grep 'not running' >/dev/null
agent status --json | grep '"agent":{"pid":null,"running":false,"startedAt":null}' >/dev/null
agent status --json | grep '"stats":null' >/dev/null
agent devices revoke --game minecraft-java --key "$key" >/dev/null
agent devices list | grep 'No approved devices' >/dev/null
agent games remove minecraft-java >/dev/null
[[ ! -f "$dir/agent/keys/minecraft-java.json" ]]
echo 'PASS: init/add/list/approve/invite/status/revoke/remove'
