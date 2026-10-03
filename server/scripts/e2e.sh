#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Loopback end-to-end smoke test. Drives the real server binary and the real
# `pull.py` over pinned TLS in a temporary directory: certificate, access
# policy, manifest, staged client, arm, serve, pull, update, stale deletion,
# and self-update. It touches only its own temporary directory and never
# requires root.
#
# Dev tool only. It requires `python3` (to pick a free port and to run the
# client) and is invoked by `make e2e` after the server binary is built.
#
# Usage: e2e.sh <lanpull-binary> <client-dir>

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

binary=${1:-}
client_dir=${2:-}
[ -n "$binary" ] || die "e2e: server binary path is required"
[ -x "$binary" ] || die "e2e: server binary not executable: $binary (run make build)"
[ -n "$client_dir" ] || die "e2e: client directory is required"
[ -f "$client_dir/pull.py" ] || die "e2e: $client_dir/pull.py not found"
command -v python3 >/dev/null 2>&1 || die "e2e: python3 is required"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/lanpull-e2e.XXXXXX")
serve_pid=""
cleanup() {
    if [ -n "$serve_pid" ]; then
        kill "$serve_pid" 2>/dev/null || true
        wait "$serve_pid" 2>/dev/null || true
    fi
    rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

share="$tmp/share"
state="$tmp/state"
conf="$tmp/lanpull.conf"
client="$tmp/client"
mkdir -p "$share/sub" "$state" "$client"
printf 'hello\n' >"$share/a.txt"
printf 'nested\n' >"$share/sub/b.txt"

# STATE_DIR already exists, so init-config never escalates.
"$script_dir/init-config.sh" --config "$conf" --share "$share" --state "$state" \
    --server-ip 127.0.0.1 --non-interactive >/dev/null
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
sed -i "s/^PORT=.*/PORT=$port/" "$conf"

"$binary" cert --config "$conf" >/dev/null
"$binary" access public add 'default:**' --config "$conf" >/dev/null
"$binary" manifest --config "$conf" >/dev/null

install -Dm755 "$client_dir/pull.py" "$state/client/pull.py"
install -Dm644 "$client_dir/VERSION" "$state/client/VERSION"
"$binary" add-client e2e --output "$tmp/mirror" --config "$conf" >/dev/null
cp -r "$state/client-ready/e2e/." "$client/"
chmod 700 "$client"
chmod 600 "$client/auth"
"$binary" arm e2e --ttl 15m --config "$conf" >/dev/null

"$binary" serve --config "$conf" >"$tmp/serve.log" 2>&1 &
serve_pid=$!

tries=0
until "$client/pull.py" --dry-run >/dev/null 2>&1; do
    tries=$((tries + 1))
    if [ "$tries" -ge 50 ]; then
        cat "$tmp/serve.log" >&2
        die "e2e: server did not become ready"
    fi
    sleep 0.2
done

# First round: updates exist, then the pull delivers every file.
if "$client/pull.py" --check >/dev/null; then
    die "e2e: --check should report updates before the first pull"
fi
"$client/pull.py" --dry-run | grep -q 'NEED: a.txt' || die "e2e: dry-run missed a.txt"
"$client/pull.py" >/dev/null
[ -f "$tmp/mirror/default/a.txt" ] || die "e2e: a.txt not delivered"
[ -f "$tmp/mirror/default/sub/b.txt" ] || die "e2e: sub/b.txt not delivered"
"$client/pull.py" --check >/dev/null || die "e2e: --check should be clean after the first pull"

# Second round: a modified and a new file are picked up after rescan.
printf 'updated\n' >"$share/a.txt"
printf 'new\n' >"$share/c.txt"
"$binary" manifest --config "$conf" >/dev/null
"$client/pull.py" >/dev/null
grep -q 'updated' "$tmp/mirror/default/a.txt" || die "e2e: a.txt not updated"
[ -f "$tmp/mirror/default/c.txt" ] || die "e2e: c.txt not delivered"
"$client/pull.py" --check >/dev/null || die "e2e: --check should be clean after the second pull"

# Third round: a file removed on the server is deleted with --delete.
rm -f "$share/a.txt"
"$binary" manifest --config "$conf" >/dev/null
"$client/pull.py" --delete >/dev/null
[ ! -e "$tmp/mirror/default/a.txt" ] || die "e2e: stale a.txt not deleted"

# Self-update at the same version is a no-op.
"$client/pull.py" --self-update >/dev/null || die "e2e: self-update failed"

echo "e2e: PASS"
