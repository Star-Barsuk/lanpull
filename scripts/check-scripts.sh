#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Parse and lint every repository-level shell script: the tools in scripts/
# and the installed git hooks in scripts/git-hooks/.
#
# `sh -n` always runs; shellcheck runs when it is installed, and its absence is
# reported rather than silently accepted.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$script_dir/.." && pwd)

set --
for file in "$root"/scripts/*.sh "$root"/scripts/git-hooks/*; do
    [ -f "$file" ] || continue
    set -- "$@" "$file"
done

if [ "$#" -eq 0 ]; then
    printf 'check-scripts: no scripts found\n'
    exit 0
fi

status=0
for file in "$@"; do
    sh -n "$file" || status=1
done

if command -v shellcheck >/dev/null 2>&1; then
    shellcheck -x -s sh "$@" || status=1
elif [ "${REQUIRE_SHELLCHECK:-0}" = "1" ]; then
    printf 'check-scripts: shellcheck is required by the full gate; install it and re-run\n' >&2
    status=1
else
    printf 'check-scripts: shellcheck not installed; checked syntax only\n' >&2
fi

[ "$status" -eq 0 ] && printf 'check-scripts: clean\n'
exit "$status"
