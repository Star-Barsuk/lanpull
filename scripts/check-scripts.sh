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

files=""
for file in "$root"/scripts/*.sh "$root"/scripts/git-hooks/*; do
    [ -f "$file" ] || continue
    files="$files $file"
done

if [ -z "$files" ]; then
    printf 'check-scripts: no scripts found\n'
    exit 0
fi

status=0
# shellcheck disable=SC2086 # the list is intentionally split into arguments
for file in $files; do
    sh -n "$file" || status=1
done

if command -v shellcheck >/dev/null 2>&1; then
    # shellcheck disable=SC2086 # the list is intentionally split into arguments
    shellcheck -x -s sh $files || status=1
else
    printf 'check-scripts: shellcheck not installed; checked syntax only\n' >&2
fi

[ "$status" -eq 0 ] && printf 'check-scripts: clean\n'
exit "$status"
