#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Parse and lint shell scripts. With no arguments the repository-level
# scripts/ and scripts/git-hooks/ trees are checked; directory arguments let
# the server Makefile reuse the same checker for server/scripts/.
#
# `sh -n` always runs; shellcheck runs when it is installed, and its absence is
# reported rather than silently accepted. The full gate sets
# REQUIRE_SHELLCHECK=1 to turn the fallback into a failure.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$script_dir/.." && pwd)

if [ "$#" -eq 0 ]; then
    set -- "$root/scripts" "$root/scripts/git-hooks"
fi

status=0
found=0
for directory in "$@"; do
    [ -d "$directory" ] || continue
    count=$(find "$directory" -maxdepth 1 -type f \
        \( -name '*.sh' -o -name 'pre-push' \) | wc -l)
    [ "$count" -gt 0 ] && found=1
    find "$directory" -maxdepth 1 -type f \
        \( -name '*.sh' -o -name 'pre-push' \) -exec sh -n {} + || status=1
done

if [ "$found" -eq 0 ]; then
    printf 'check-scripts: no scripts found\n'
    exit 0
fi

if command -v shellcheck >/dev/null 2>&1; then
    for directory in "$@"; do
        [ -d "$directory" ] || continue
        find "$directory" -maxdepth 1 -type f \( -name '*.sh' -o -name 'pre-push' \) \
            -exec shellcheck -x -s sh -P "$directory" {} + || status=1
    done
elif [ "${REQUIRE_SHELLCHECK:-0}" = "1" ]; then
    printf 'check-scripts: shellcheck is required by the full gate; install it and re-run\n' >&2
    status=1
else
    printf 'check-scripts: shellcheck not installed; checked syntax only\n' >&2
fi

[ "$status" -eq 0 ] && printf 'check-scripts: clean\n'
exit "$status"
