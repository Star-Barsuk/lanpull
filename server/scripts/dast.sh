#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Local DAST: TLS posture with testssl.sh plus HTTP behaviour probes against a
# scratch server. Dev-only and not part of `make ci-full`: the scanner is an
# external tool and the scan takes time. Self-signed certificate findings are
# expected; only protocol, cipher, and SAN regressions fail.
#
# Usage: dast.sh <lanpull-binary> <client-dir>
#
# TESTSSL may point at testssl.sh; otherwise it is looked up on PATH.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

binary=${1:-}
client_dir=${2:-}
[ -n "$binary" ] || die "dast: server binary path is required"
[ -x "$binary" ] || die "dast: binary not executable: $binary (run make build)"
[ -n "$client_dir" ] || die "dast: client directory is required"
[ -f "$client_dir/pull.py" ] || die "dast: $client_dir/pull.py not found"
command -v python3 >/dev/null 2>&1 || die "dast: python3 is required"
command -v curl >/dev/null 2>&1 || die "dast: curl is required"
command -v openssl >/dev/null 2>&1 || die "dast: openssl is required"

testssl=${TESTSSL:-}
if [ -z "$testssl" ]; then
    testssl=$(command -v testssl.sh || true)
fi
if [ -z "$testssl" ] || [ ! -x "$testssl" ]; then
    die "dast: testssl.sh not found; set TESTSSL=/path/to/testssl.sh"
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/lanpull-dast.XXXXXX")
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
cert=$state/server.crt
mkdir -p "$share/sub" "$state"
printf 'hello\n' >"$share/a.txt"
printf 'nested\n' >"$share/sub/b.txt"
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')

"$binary" init --config "$conf" --share "dast=$share" --state-dir "$state" \
    --server-ip 127.0.0.1 --port "$port" >/dev/null
"$binary" cert --config "$conf" >/dev/null
"$binary" access public add 'dast:**' --config "$conf" >/dev/null
"$binary" share rescan --config "$conf" >/dev/null
install -Dm755 "$client_dir/pull.py" "$state/client/pull.py"
install -Dm644 "$client_dir/VERSION" "$state/client/VERSION"
"$binary" account add dast --output "$tmp/mirror" --local --config "$conf" >/dev/null
creds=$(cat "$state/client-ready/dast/auth")

"$binary" serve --config "$conf" >"$tmp/serve.log" 2>&1 &
serve_pid=$!

base="https://127.0.0.1:$port"
manifest="$base/_lanpull/share/dast/manifest.json"
file="$base/_lanpull/share/dast/file/a.txt"

ready=0
for _ in $(seq 1 60); do
    if curl -sS -o /dev/null --cacert "$cert" -u "$creds" "$manifest" 2>/dev/null; then
        ready=1
        break
    fi
    sleep 0.2
done
if [ "$ready" != 1 ]; then
    cat "$tmp/serve.log" >&2
    die "dast: server did not become ready"
fi

fail=0
pass() { printf 'ok   - %s\n' "$1"; }
fail_msg() {
    printf 'FAIL - %s\n' "$1" >&2
    fail=1
}
code() { curl -sS -o /dev/null -w '%{http_code}' --cacert "$cert" "$@"; }
check_code() { # <desc> <want> <curl args...>
    desc=$1
    want=$2
    shift 2
    got=$(code "$@")
    if [ "$got" = "$want" ]; then pass "$desc"; else fail_msg "$desc: got $got, want $want"; fi
}

# --- HTTP behaviour -------------------------------------------------------
check_code "manifest without credentials is 401" 401 "$manifest"
check_code "manifest with credentials is 200" 200 -u "$creds" "$manifest"
check_code "unknown path is 404" 404 -u "$creds" "$base/nope"
check_code "POST is method not allowed" 405 -u "$creds" -X POST "$manifest"
check_code "path traversal is refused" 404 -u "$creds" --path-as-is \
    "$base/_lanpull/share/dast/file/../../etc/passwd"
check_code "encoded traversal is refused" 404 -u "$creds" --path-as-is \
    "$base/_lanpull/share/dast/file/%2e%2e%2f%2e%2e%2fetc%2fpasswd"
check_code "malformed Basic credential is 401" 401 -H 'Authorization: Basic !!!' "$manifest"
check_code "HEAD manifest is 200" 200 -I -u "$creds" "$manifest"

curl -sS -o /dev/null -D "$tmp/401.headers" --cacert "$cert" "$manifest"
if grep -qi '^www-authenticate: *Basic' "$tmp/401.headers"; then
    pass "401 carries a Basic challenge"
else
    fail_msg "401 is missing the WWW-Authenticate challenge"
fi

curl -sS -o /dev/null -D "$tmp/manifest.headers" --cacert "$cert" -u "$creds" "$manifest"
if grep -qi '^x-content-type-options: *nosniff' "$tmp/manifest.headers"; then
    pass "responses carry X-Content-Type-Options: nosniff"
else
    fail_msg "X-Content-Type-Options: nosniff is missing"
fi

# Generated test credentials, not secrets.
curl -sS -D "$tmp/wrong.headers" -o "$tmp/wrong.body" --cacert "$cert" -u "dast:wrong" "$manifest" #gitleaks:allow
curl -sS -D "$tmp/unknown.headers" -o "$tmp/unknown.body" --cacert "$cert" -u "ghost:whatever" "$manifest" #gitleaks:allow
status_of() { sed -n '1s/.* \([0-9][0-9][0-9]\) .*/\1/p' "$1"; }
reason_of() { sed -n 's/^[Xx]-[Ll]anpull-[Rr]eason: *//p' "$1" | tr -d '\r'; }
wrong_status=$(status_of "$tmp/wrong.headers")
unknown_status=$(status_of "$tmp/unknown.headers")
wrong_reason=$(reason_of "$tmp/wrong.headers")
unknown_reason=$(reason_of "$tmp/unknown.headers")
if [ "$wrong_status" = "401" ] && [ "$unknown_status" = "401" ] \
    && cmp -s "$tmp/wrong.body" "$tmp/unknown.body" \
    && [ "$wrong_reason" = "auth_failed" ] && [ "$unknown_reason" = "auth_failed" ]; then
    pass "unknown account and wrong password are indistinguishable"
else
    fail_msg "unknown account and wrong password differ (status/body/reason)"
fi

range_code=$(code -u "$creds" -r 0-4 "$file")
range_body=$(curl -sS --cacert "$cert" -u "$creds" -r 0-4 "$file")
if [ "$range_code" = "206" ] && [ "$range_body" = "hello" ]; then
    pass "Range request returns 206 with the requested bytes"
else
    fail_msg "Range request failed (status $range_code, body '$range_body')"
fi

headers_body=$(curl -sS --cacert "$cert" -u "$creds" -I "$manifest")
length=$(printf '%s\n' "$headers_body" | sed -n 's/^[Cc]ontent-[Ll]ength: *//p' | tr -d '\r' | head -n 1)
if [ -n "$length" ] && [ "$length" -gt 0 ]; then
    pass "HEAD manifest reports a Content-Length"
else
    fail_msg "HEAD manifest has no Content-Length"
fi

# --- TLS posture ----------------------------------------------------------
"$testssl" --quiet --color 0 --warnings off -p -s -S "$base" \
    >"$tmp/testssl.txt" 2>&1 || true

weak_protocols=$(grep -E 'TLS 1( |\.0|\.1)' "$tmp/testssl.txt" \
    | grep -i 'offered' | grep -vi 'not offered' || true)
if [ -z "$weak_protocols" ]; then
    pass "TLS 1.0 and 1.1 are not offered"
else
    fail_msg "weak TLS protocol offered: $weak_protocols"
fi

weak_ciphers=$(grep -Ei '(NULL|EXPORT|RC4|3DES|DES-CBC|anon).*offered' "$tmp/testssl.txt" \
    | grep -vi 'not offered' || true)
if [ -z "$weak_ciphers" ]; then
    pass "no NULL/EXPORT/RC4/3DES/anon cipher is offered"
else
    fail_msg "weak cipher offered: $weak_ciphers"
fi

openssl x509 -in "$cert" -noout -ext subjectAltName >"$tmp/san.txt"
if grep -q 'IP Address:127.0.0.1' "$tmp/san.txt" && grep -q 'DNS:localhost' "$tmp/san.txt"; then
    pass "certificate SAN covers loopback and localhost"
else
    fail_msg "certificate SAN is missing loopback or localhost"
fi

if openssl x509 -in "$cert" -noout -checkend 0 >/dev/null 2>&1; then
    pass "certificate is not expired"
else
    fail_msg "certificate is expired"
fi

algo=$(openssl x509 -in "$cert" -noout -text | grep -m 1 'Public Key Algorithm' || true)
case "$algo" in
    *rsa*)
        bits=$(openssl x509 -in "$cert" -noout -text | sed -n 's/.*Public-Key: (\([0-9]*\) bit).*/\1/p' | head -n 1)
        if [ -n "$bits" ] && [ "$bits" -ge 2048 ]; then
            pass "RSA key is at least 2048 bits"
        else
            fail_msg "RSA key is shorter than 2048 bits"
        fi
        ;;
    *)
        pass "certificate uses an EC key"
        ;;
esac

if [ "$fail" -ne 0 ]; then
    printf 'dast: FAIL\n' >&2
    exit 1
fi
printf 'dast: PASS\n'
