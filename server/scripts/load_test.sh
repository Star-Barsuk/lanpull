#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Concurrency smoke test. Starts a scratch loopback server and runs several
# independent `pull.py` clients against it at the same time. Each client has
# its own account and mirror, so the run exercises the account cache, the
# per-request path, and concurrent manifest/file reads.
#
# Periodic, not part of the gate. It needs python3 and no root.
#
# Usage: load_test.sh <lanpull-binary> <client-dir> [clients]

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

binary=${1:-}
client_dir=${2:-}
clients=${3:-8}
[ -n "$binary" ] || die "load: server binary path is required"
[ -x "$binary" ] || die "load: server binary not executable: $binary (run make build)"
[ -n "$client_dir" ] || die "load: client directory is required"
[ -f "$client_dir/pull.py" ] || die "load: $client_dir/pull.py not found"
case "$clients" in
    ''|*[!0-9]*) die "load: clients must be a positive integer" ;;
esac
[ "$clients" -ge 1 ] || die "load: clients must be a positive integer"
command -v python3 >/dev/null 2>&1 || die "load: python3 is required"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/lanpull-load.XXXXXX")
serve_pid=""
cleanup() {
    if [ -n "$serve_pid" ]; then
        kill "$serve_pid" 2>/dev/null || true
        wait "$serve_pid" 2>/dev/null || true
    fi
    rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

share=$tmp/share
state=$tmp/state
conf=$tmp/lanpull.conf
mkdir -p "$share" "$state"
printf 'hello\n' >"$share/a.txt"

"$binary" init --config "$conf" --share "default=$share" --state-dir "$state" \
    --server-ip 127.0.0.1 >/dev/null
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
sed -i "s/^PORT=.*/PORT=$port/" "$conf"
"$binary" cert --config "$conf" >/dev/null
"$binary" access public add 'default:**' --config "$conf" >/dev/null
"$binary" share rescan --config "$conf" >/dev/null
install -Dm755 "$client_dir/pull.py" "$state/client/pull.py"
install -Dm644 "$client_dir/VERSION" "$state/client/VERSION"

i=1
while [ "$i" -le "$clients" ]; do
    "$binary" account add "load$i" --output "$tmp/mirror$i" --config "$conf" >/dev/null
    "$binary" account export "load$i" --to "$tmp/client$i" --config "$conf" >/dev/null
    chmod 700 "$tmp/client$i"
    chmod 600 "$tmp/client$i/auth"
    i=$((i + 1))
done
"$binary" account arm --all --ttl 15m --config "$conf" >/dev/null

"$binary" serve --config "$conf" >"$tmp/serve.log" 2>&1 &
serve_pid=$!

tries=0
until "$tmp/client1/pull.py" --dry-run >/dev/null 2>&1; do
    tries=$((tries + 1))
    if [ "$tries" -ge 50 ]; then
        cat "$tmp/serve.log" >&2
        die "load: server did not become ready"
    fi
    sleep 0.2
done

pids=""
i=1
while [ "$i" -le "$clients" ]; do
    "$tmp/client$i/pull.py" >"$tmp/client$i.log" 2>&1 &
    pids="$pids $!"
    i=$((i + 1))
done

failed=0
for pid in $pids; do
    wait "$pid" || failed=1
done

i=1
while [ "$i" -le "$clients" ]; do
    if [ ! -f "$tmp/mirror$i/default/a.txt" ]; then
        printf 'load: client %s did not receive a.txt\n' "$i" >&2
        failed=1
    fi
    i=$((i + 1))
done

if [ "$failed" -ne 0 ]; then
    for log in "$tmp"/client*.log; do
        [ -f "$log" ] || continue
        printf '\n--- %s ---\n' "$log" >&2
        cat "$log" >&2
    done
    die "load: $clients concurrent pulls failed"
fi

printf 'load: PASS (%s concurrent clients)\n' "$clients"
