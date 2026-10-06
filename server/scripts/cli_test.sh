#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Operator-style CLI test. Drives the real `lanpull` binary the way the docs
# describe and asserts effects (files on disk, generated manifests, exit codes,
# stream separation, JSON envelopes), not just printed text. It needs no root
# and no network.
#
# Usage: cli_test.sh <lanpull-binary>

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

binary=${1:-}
[ -n "$binary" ] || die "cli-test: binary path is required"
[ -x "$binary" ] || die "cli-test: binary not executable: $binary (run make build)"
client_dir=$script_dir/../../client
[ -f "$client_dir/pull.py" ] || die "cli-test: client bundle not found under $client_dir"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/lanpull-cli.XXXXXX")
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT INT TERM

out=$tmp/stdout
err=$tmp/stderr
rc=0
fail=0

pass() { printf 'ok   - %s\n' "$1"; }
fail_msg() {
    printf 'FAIL - %s\n' "$1" >&2
    fail=1
}
section() { printf '\n# %s\n' "$1"; }

# run <command...>: capture stdout in $out, stderr in $err, exit code in $rc.
run() {
    set +e
    "$@" >"$out" 2>"$err"
    rc=$?
    set -e
}

assert_rc() { # <desc> <want>
    if [ "$rc" -eq "$2" ]; then
        pass "$1"
    else
        fail_msg "$1: exit $rc, want $2"
    fi
}
assert_out_has() { # <desc> <substring>
    if grep -Fq "$2" "$out"; then pass "$1"; else fail_msg "$1: stdout lacks '$2'"; fi
}
assert_out_not_has() {
    if grep -Fq "$2" "$out"; then fail_msg "$1: stdout has '$2'"; else pass "$1"; fi
}
assert_err_has() {
    if grep -Fq "$2" "$err"; then pass "$1"; else fail_msg "$1: stderr lacks '$2'"; fi
}
assert_out_eq() { # <desc> <exact>
    if [ "$(cat "$out")" = "$2" ]; then pass "$1"; else fail_msg "$1: stdout is not '$2'"; fi
}
assert_out_empty() {
    if [ ! -s "$out" ]; then pass "$1"; else fail_msg "$1: stdout is not empty"; fi
}
assert_err_empty() {
    if [ ! -s "$err" ]; then pass "$1"; else fail_msg "$1: stderr is not empty"; fi
}
assert_json_ok() {
    if grep -Fq '"status": "ok"' "$out" && [ ! -s "$err" ]; then
        pass "$1"
    else
        fail_msg "$1: not an ok envelope on stdout with empty stderr"
    fi
}
assert_json_error() { # <desc> <code>
    if grep -Fq '"status": "error"' "$out" && grep -Fq "\"code\": $2" "$out"; then
        pass "$1"
    else
        fail_msg "$1: not an error envelope with code $2"
    fi
}

# --- Setup: a working server configuration, certificate, and access policy ---
share=$tmp/share
state=$tmp/state
conf=$tmp/lanpull.conf
mkdir -p "$share/sub" "$state"
printf 'hello\n' >"$share/a.txt"
printf 'nested\n' >"$share/sub/b.txt"
mkdir -p "$share/target/debug" "$share/__pycache__" "$share/.git"
printf 'bin\n' >"$share/target/debug/app"
printf 'pyc\n' >"$share/__pycache__/m.pyc"
printf 'ref\n' >"$share/.git/HEAD"

run "$binary" init --config "$conf" --share "default=$share" --state-dir "$state" \
    --server-ip 127.0.0.1
assert_rc "setup: init creates the configuration" 0
install -Dm755 "$client_dir/pull.py" "$state/client/pull.py"
install -Dm644 "$client_dir/VERSION" "$state/client/VERSION"
run "$binary" cert --config "$conf"
assert_rc "setup: cert generates the certificate" 0
run "$binary" access public add 'default:**' --config "$conf"
assert_rc "setup: access public add grants the share" 0

# --- 1. Global contract ---
section "global contract"
run "$binary" --help
assert_rc "--help exits 0" 0
assert_out_has "--help shows usage" "Usage: lanpull"
run "$binary" --version
assert_rc "--version exits 0" 0
assert_out_has "--version prints a version" "lanpull"
run "$binary" bogus
assert_rc "unknown command exits 2" 2
assert_err_has "unknown command error prefix" "error:"
assert_err_has "unknown command hint" "hint:"
run "$binary" --nope
assert_rc "unknown flag exits 2" 2
run "$binary" config show --config "$tmp/missing.conf"
assert_rc "missing config exits 3" 3
assert_err_has "missing config is not-initialized" "not initialized"
assert_err_has "missing config hint" "hint:"
run "$binary" --json config show --config "$tmp/missing.conf"
assert_rc "missing config --json exits 3" 3
assert_json_error "missing config --json error envelope" 3
assert_out_has "missing config --json command" '"command": "config show"'
assert_out_has "missing config --json message" '"message":'
assert_out_has "missing config --json hint" '"hint":'
assert_err_empty "missing config --json keeps stderr empty"
run "$binary" --json bogus
assert_rc "unknown command --json exits 2" 2
assert_json_error "unknown command --json error envelope" 2
run env LANPULL_CONFIG="$conf" "$binary" config path
assert_rc "LANPULL_CONFIG selects the config" 0
assert_out_eq "LANPULL_CONFIG resolves the path" "$conf"
run env LANPULL_CONFIG="$tmp/missing.conf" "$binary" config path --config "$conf"
assert_rc "--config overrides LANPULL_CONFIG" 0
assert_out_eq "--config wins" "$conf"

# --- 2. init boundaries ---
section "init boundaries"
run "$binary" init --config "$tmp/init.conf" --share "no-separator" \
    --state-dir "$tmp/istate" --server-ip 127.0.0.1
assert_rc "init rejects a malformed --share" 2
run "$binary" init --config "$tmp/init.conf" --share "Bad=x" \
    --state-dir "$tmp/istate" --server-ip 127.0.0.1
assert_rc "init rejects an invalid share name" 2
run "$binary" init --config "$tmp/init.conf" --share "default=$tmp/ishare" \
    --state-dir "$tmp/istate" --server-ip not-an-ip
assert_rc "init rejects an invalid --server-ip" 1
mkdir -p "$tmp/ishare"
run "$binary" init --config "$tmp/init.conf" --share "default=$tmp/ishare" \
    --state-dir "$tmp/istate" --server-ip 127.0.0.1
assert_rc "init creates a fresh configuration" 0
if [ -f "$tmp/ishare/.lanpullignore" ]; then
    pass "init writes the .lanpullignore template"
else
    fail_msg "init did not write .lanpullignore"
fi
run "$binary" init --config "$tmp/init.conf" --share "default=$tmp/ishare" \
    --state-dir "$tmp/istate" --server-ip 127.0.0.1
assert_rc "init refuses to overwrite without --force" 2
run "$binary" init --config "$tmp/init.conf" --share "default=$tmp/ishare" \
    --state-dir "$tmp/istate" --server-ip 127.0.0.1 --force
assert_rc "init --force overwrites" 0
if [ "$(id -u)" -ne 0 ]; then
    run "$binary" init --config /etc/lanpull/lanpull.conf --share "default=$tmp/ishare" \
        --state-dir "$tmp/istate" --server-ip 127.0.0.1
    assert_rc "init refuses the canonical path without root" 2
fi

# --- 3. config ---
section "config"
run "$binary" config show --config "$conf"
assert_rc "config show exits 0" 0
assert_out_has "config show lists STATE_DIR" "STATE_DIR:"
assert_out_has "config show lists the share" "default:"
run "$binary" config path --config "$conf"
assert_out_eq "config path prints the file" "$conf"
run "$binary" config get PORT --config "$conf"
assert_out_eq "config get returns a known key" "8000"
run "$binary" config get SHARE_default --config "$conf"
assert_out_eq "config get returns the share path" "$share"
run "$binary" config get NOPE --config "$conf"
assert_rc "config get rejects an unknown key" 1
assert_err_has "config get unknown-key hint" "config show"
run "$binary" config set SHARE_x /tmp --config "$conf"
assert_rc "config set rejects SHARE_ keys" 1
assert_err_has "config set points at share add" "lanpull share add"
run "$binary" config set PORT "" --config "$conf"
assert_rc "config set rejects an empty value" 2
run "$binary" config set PORT 9000 --dry-run --config "$conf"
assert_rc "config set --dry-run exits 0" 0
assert_out_has "config set --dry-run reports the change" "would set PORT=9000"
run "$binary" config get PORT --config "$conf"
assert_out_eq "config set --dry-run writes nothing" "8000"
run "$binary" config set PORT 9000 --config "$conf"
assert_rc "config set writes the value" 0
run "$binary" --json config get PORT --config "$conf"
assert_json_ok "config get --json is an ok envelope"
assert_out_has "config get --json carries the data" '"value": "9000"'

# --- 4. cert ---
section "cert"
run "$binary" cert --config "$conf"
assert_rc "cert refuses to overwrite without --force" 2
assert_err_has "cert overwrite hint" "already exists"
run "$binary" cert --force --config "$conf"
assert_rc "cert --force regenerates" 0
assert_out_has "cert reports the certificate" "certificate written to"
if [ "$(stat -c '%a' "$state/server.key")" = "600" ]; then
    pass "cert writes the key mode 600"
else
    fail_msg "cert wrote the key with the wrong mode"
fi

# --- 5. share ---
section "share"
run "$binary" share list --config "$conf"
assert_rc "share list exits 0" 0
assert_out_has "share list shows the share" "default"
run "$binary" share remove default --config "$conf"
assert_rc "share remove is rejected (D77)" 2
second=$tmp/second
mkdir -p "$second"
run "$binary" share add second "$second" --config "$conf"
assert_rc "share add creates a share" 0
if [ -f "$second/.lanpullignore" ]; then
    pass "share add writes the .lanpullignore template"
else
    fail_msg "share add did not write .lanpullignore"
fi
run "$binary" share add second "$second" --config "$conf"
assert_rc "share add rejects a duplicate" 1
run "$binary" share add ghost "$tmp/nope" --config "$conf"
assert_rc "share add rejects a missing directory" 1
run "$binary" share add "Bad" "$second" --config "$conf"
assert_rc "share add rejects an invalid name" 2
run "$binary" share add ghost "$second" --dry-run --config "$conf"
assert_rc "share add --dry-run exits 0" 0
assert_out_has "share add --dry-run reports the change" "would add share ghost"

# --- 6. access ---
section "access"
run "$binary" access public list --config "$conf"
assert_rc "access public list exits 0" 0
assert_out_has "access public list shows the rule" "default:**"
run "$binary" access public add 'default:notes.txt' --config "$conf"
assert_rc "access public add exits 0" 0
run "$binary" access public add 'nope:**' --config "$conf"
assert_rc "access public add rejects an unknown share" 1
run "$binary" access public add 'default' --config "$conf"
assert_rc "access public add rejects a malformed rule" 1
assert_err_has "malformed rule hint" "expected <share>:<path>"
run "$binary" access public add 'default:missing.pdf' --config "$conf"
assert_rc "access public add accepts a glob-free path" 0
run "$binary" access doctor --config "$conf"
assert_rc "access doctor exits 0" 0
assert_out_has "access doctor summarizes the warnings" "warning(s)"
assert_err_has "access doctor reports a missing file" "does not exist"
run "$binary" access public remove 'default:missing.pdf' --yes --config "$conf"
assert_rc "access public remove --yes exits 0" 0
run "$binary" access public remove 'default:notes.txt' --config "$conf"
assert_rc "access public remove without --yes needs a terminal" 2
assert_err_has "confirmation refusal hint" "pass --yes"
run "$binary" access client add ghost 'default:**' --config "$conf"
assert_rc "access client add for a future account exits 0" 0
assert_err_has "access client add warns about the missing account" "does not exist yet"
run "$binary" access client list --config "$conf"
assert_rc "access client list exits 0" 0
run "$binary" access client list ghost --config "$conf"
assert_out_has "access client list shows the granted path" "ghost"
run "$binary" access client remove ghost 'default:**' --yes --config "$conf"
assert_rc "access client remove --yes exits 0" 0
printf 'ghost default:**\n' >"$tmp/old.access"
run "$binary" access import --from "$tmp/old.access" --config "$conf"
assert_rc "access import exits 0" 0
assert_out_has "access import reports the count" "imported"
run "$binary" access apply --config "$conf"
assert_rc "access apply exits 0" 0

# --- 7. account ---
section "account"
run "$binary" account add e2e --output "$tmp/mirror" --config "$conf"
assert_rc "account add creates an account" 0
assert_out_has "account add reports staging" "created account e2e"
run "$binary" account add e2e --output "$tmp/mirror" --config "$conf"
assert_rc "account add rejects a duplicate" 1
run "$binary" account list --config "$conf"
assert_rc "account list exits 0" 0
assert_out_has "account list shows the account" "e2e"
run "$binary" --json account list --config "$conf"
assert_json_ok "account list --json is an ok envelope"
run "$binary" account passwd e2e --config "$conf"
assert_rc "account passwd exits 0" 0
assert_out_has "account passwd prints a password" "password for e2e:"
run "$binary" account passwd nope --config "$conf"
assert_rc "account passwd rejects an unknown account" 1
run "$binary" account export e2e --to "$tmp/export" --config "$conf"
assert_rc "account export exits 0" 0
run "$binary" account export nope --to "$tmp/export" --config "$conf"
assert_rc "account export rejects an unknown account" 1
run "$binary" account arm e2e --ttl 15m --config "$conf"
assert_rc "account arm exits 0" 0
assert_out_has "account arm reports the window" "armed e2e for"
run "$binary" account arm --config "$conf"
assert_rc "account arm without a name is a usage error" 2
run "$binary" account arm nope --ttl 15m --config "$conf"
assert_rc "account arm rejects an unknown account" 1
run "$binary" account arm e2e --ttl bogus --config "$conf"
assert_rc "account arm rejects an invalid ttl" 1
run "$binary" account disarm e2e --config "$conf"
assert_rc "account disarm exits 0" 0
run "$binary" account disarm e2e --config "$conf"
assert_out_has "account disarm is idempotent" "was not armed"
run "$binary" account remove nope --yes --config "$conf"
assert_rc "account remove rejects an unknown account" 1
run "$binary" account remove e2e --yes --config "$conf"
assert_rc "account remove --yes exits 0" 0
run "$binary" account list --config "$conf"
assert_out_has "account list is empty after removal" "no accounts"

# Missing prerequisites for `account add` are reported distinctly.
need=$tmp/need
mkdir -p "$need/share" "$need/state"
run "$binary" init --config "$need/lanpull.conf" --share "default=$need/share" \
    --state-dir "$need/state" --server-ip 127.0.0.1
assert_rc "need: init" 0
run "$binary" account add a --output "$need/mirror" --config "$need/lanpull.conf"
assert_rc "account add without a bundle fails" 1
assert_err_has "account add bundle hint" "make install"
install -Dm755 "$client_dir/pull.py" "$need/state/client/pull.py"
install -Dm644 "$client_dir/VERSION" "$need/state/client/VERSION"
run "$binary" account add a --output "$need/mirror" --config "$need/lanpull.conf"
assert_rc "account add without access fails" 1
assert_err_has "account add access hint" "has no access"
run "$binary" access public add 'default:**' --config "$need/lanpull.conf"
assert_rc "need: access public add" 0
run "$binary" account add a --output "$need/mirror" --config "$need/lanpull.conf"
assert_rc "account add without a certificate fails" 1
assert_err_has "account add certificate hint" "lanpull cert"

# --- 8. misc ---
section "misc"
run "$binary" status --config "$conf"
assert_rc "status exits 0" 0
assert_out_has "status lists the manifest" "manifest default:"
run "$binary" report --config "$conf"
assert_rc "report on an empty log exits 0" 0
assert_out_has "report says there are no requests" "no requests recorded"
run "$binary" report --since bogus --config "$conf"
assert_rc "report rejects an invalid --since" 1
run "$binary" report --user e2e --config "$conf"
assert_rc "report --user exits 0" 0
run "$binary" audit --config "$conf"
assert_rc "audit exits 0" 0
assert_out_has "audit lists the configuration artifact" "config"
run "$binary" clean --config "$conf"
assert_rc "clean without --yes needs a terminal when there is work" 2
# The orphaned client-ready folder for the removed account is a real leftover.
printf 'stale\n' >"$conf.shares"
run "$binary" clean --dry-run --config "$conf"
assert_rc "clean --dry-run exits 0" 0
assert_out_has "clean --dry-run lists a leftover" "would remove"
assert_out_not_has "clean no longer looks for the removed *.conf.shares" "conf.shares"
run "$binary" clean --yes --config "$conf"
assert_rc "clean --yes removes the leftover" 0
run "$binary" clean --config "$conf"
assert_rc "clean with nothing left exits 0" 0
assert_out_has "clean reports nothing to do" "nothing to clean"

# --- 9. Streams and JSON ---
section "streams and JSON"
run "$binary" --config "$conf" access client add future 'default:**'
assert_rc "warning command exits 0" 0
assert_out_has "the summary is on stdout" "granted"
assert_err_has "the warning is on stderr" "warning:"
run "$binary" --json --config "$conf" share list
assert_json_ok "share list --json is an ok envelope"
assert_out_has "the envelope carries the command" '"command": "share list"'
assert_err_empty "a successful --json run keeps stderr empty"

if [ "$fail" -eq 0 ]; then
    echo "cli tests passed"
else
    echo "cli tests failed" >&2
    exit 1
fi
