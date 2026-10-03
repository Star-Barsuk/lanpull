#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Remove repository-local runtime leftovers. Invoked by `make distclean`; it
# prunes version control, build outputs, and the client virtual environment.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

root=${1:-}
[ -n "$root" ] || die "distclean: repository root is required"
[ -d "$root" ] || die "distclean: not a directory: $root"

find "$root" \
    -path "$root/.git" -prune -o \
    -path "$root/target" -prune -o \
    -path "$root/server/target" -prune -o \
    -path "$root/client/.venv" -prune -o \
    -type f \( -name '.lanpull.lock' -o -name 'state.json' \
        -o -name '*.part' -o -name '.manifest.tmp' \) \
    -print -exec rm -f {} +
