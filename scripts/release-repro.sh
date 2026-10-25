#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Build the release archive twice from the same working tree and compare the
# SHA256 values. This is a release-only reproducibility check: the first run
# warms caches, the second must produce identical bytes.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
cd "$root"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/lanpull-repro.XXXXXX")
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT INT TERM

printf 'release-repro: first build\n'
if ! scripts/release.sh --check >"$tmp/first.log" 2>&1; then
    cat "$tmp/first.log" >&2
    echo "release-repro: first build failed" >&2
    exit 1
fi
first=$(sed -n 's/^sha256:  *//p' "$tmp/first.log" | tail -n 1)
[ -n "$first" ] || { cat "$tmp/first.log" >&2; echo "release-repro: first build has no hash" >&2; exit 1; }

printf 'release-repro: second build\n'
if ! scripts/release.sh --check >"$tmp/second.log" 2>&1; then
    cat "$tmp/second.log" >&2
    echo "release-repro: second build failed" >&2
    exit 1
fi
second=$(sed -n 's/^sha256:  *//p' "$tmp/second.log" | tail -n 1)
[ -n "$second" ] || { cat "$tmp/second.log" >&2; echo "release-repro: second build has no hash" >&2; exit 1; }

if [ "$first" != "$second" ]; then
    printf 'release-repro: FAIL\n  first:  %s\n  second: %s\n' "$first" "$second" >&2
    exit 1
fi

printf 'release-repro: PASS\n  sha256: %s\n' "$first"
