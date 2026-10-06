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
mkdir -p "$share/sub" "$state"
printf 'hello\n' >"$share/a.txt"
printf 'nested\n' >"$share/sub/b.txt"
# Default-ignored artifacts: build, cache, and VCS files that must never be
# distributed unless an operator re-includes them.
mkdir -p "$share/target/debug" "$share/__pycache__" "$share/.git" "$share/node_modules/pkg"
printf 'bin\n' >"$share/target/debug/app"
printf 'pyc\n' >"$share/__pycache__/m.pyc"
printf 'ref\n' >"$share/.git/HEAD"
printf 'js\n' >"$share/node_modules/pkg/a.js"

# STATE_DIR already exists, so init never escalates.
"$binary" init --config "$conf" --share "default=$share" --state-dir "$state" \
    --server-ip 127.0.0.1 >/dev/null
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
# GNU sed: `-i` without a suffix is not portable to BSD/macOS, but lanpull is
# Linux-only and this is a dev-only script.
sed -i "s/^PORT=.*/PORT=$port/" "$conf"

"$binary" cert --config "$conf" >/dev/null
"$binary" access public add 'default:**' --config "$conf" >/dev/null
"$binary" share rescan --config "$conf" >/dev/null

install -Dm755 "$client_dir/pull.py" "$state/client/pull.py"
install -Dm644 "$client_dir/VERSION" "$state/client/VERSION"
"$binary" account add e2e --output "$tmp/mirror" --config "$conf" >/dev/null
"$binary" account export e2e --to "$client" --config "$conf" >/dev/null
chmod 700 "$client"
chmod 600 "$client/auth"
"$binary" account arm e2e --ttl 15m --config "$conf" >/dev/null

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
if "$client/pull.py" --dry-run | grep -q 'target/debug'; then
    die "e2e: default-ignored path listed in --dry-run"
fi
"$client/pull.py" >/dev/null
[ -f "$tmp/mirror/default/a.txt" ] || die "e2e: a.txt not delivered"
[ -f "$tmp/mirror/default/sub/b.txt" ] || die "e2e: sub/b.txt not delivered"
for ignored in target/debug/app __pycache__/m.pyc .git/HEAD node_modules/pkg/a.js; do
    [ ! -e "$tmp/mirror/default/$ignored" ] || die "e2e: default-ignored file delivered: $ignored"
done
"$client/pull.py" --check >/dev/null || die "e2e: --check should be clean after the first pull"

# Second round: a modified and a new file are picked up after rescan.
printf 'updated\n' >"$share/a.txt"
printf 'new\n' >"$share/c.txt"
"$binary" share rescan --config "$conf" >/dev/null
"$client/pull.py" >/dev/null
grep -q 'updated' "$tmp/mirror/default/a.txt" || die "e2e: a.txt not updated"
[ -f "$tmp/mirror/default/c.txt" ] || die "e2e: c.txt not delivered"
"$client/pull.py" --check >/dev/null || die "e2e: --check should be clean after the second pull"

# Ignore round: an operator rule excludes a file, a `!` rule overrides a
# default, an invalid line is a warning rather than a failure, and the ignore
# file itself is never distributed.
printf 'secret\n' >"$share/ignored.bin"
printf 'ignored.bin\n!__pycache__/\nbad//pattern\n' >"$share/.lanpullignore"
"$binary" share rescan --config "$conf" >/dev/null 2>"$tmp/rescan.err" \
    || die "e2e: rescan failed on an invalid .lanpullignore line"
grep -q 'bad//pattern' "$tmp/rescan.err" || die "e2e: rescan did not warn about the invalid pattern"
if "$client/pull.py" --dry-run | grep -q 'ignored.bin'; then
    die "e2e: .lanpullignore entry was served"
fi
"$client/pull.py" >/dev/null
[ ! -e "$tmp/mirror/default/ignored.bin" ] || die "e2e: ignored file was delivered"
[ -f "$tmp/mirror/default/__pycache__/m.pyc" ] || die "e2e: negated default was not delivered"
[ ! -e "$tmp/mirror/default/target/debug/app" ] || die "e2e: target/ leaked past a negation"
[ ! -e "$tmp/mirror/default/.lanpullignore" ] || die "e2e: .lanpullignore was delivered"
"$client/pull.py" --check >/dev/null || die "e2e: --check should be clean after the ignore round"

# Third round: a file removed on the server is deleted with --delete.
rm -f "$share/a.txt"
"$binary" share rescan --config "$conf" >/dev/null
"$client/pull.py" --delete >/dev/null
[ ! -e "$tmp/mirror/default/a.txt" ] || die "e2e: stale a.txt not deleted"

# Self-update at the same version is a no-op.
"$client/pull.py" --self-update >/dev/null || die "e2e: self-update failed"

# Offline cleanup removes the runtime residue but keeps delivered files.
"$client/pull.py" --clean --yes >/dev/null || die "e2e: --clean failed"
[ ! -e "$client/state.json" ] || die "e2e: --clean left state.json"
[ ! -e "$tmp/mirror/default/.lanpull.lock" ] || die "e2e: --clean left the lock"
[ -f "$tmp/mirror/default/c.txt" ] || die "e2e: --clean removed a delivered file"

echo "e2e: PASS"
