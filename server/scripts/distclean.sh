#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Remove repository-local runtime leftovers. Invoked by `make distclean`; the
# version-control, build-output, and client-virtualenv trees are skipped, and
# only lanpull's own runtime files are removed.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

root=${1:-}
[ -n "$root" ] || die "distclean: repository root is required"
[ -d "$root" ] || die "distclean: not a directory: $root"
root=$(CDPATH='' cd -- "$root" && pwd)
require_safe_path "$root" REPO_ROOT

find "$root" \
    -path "$root/.git" -prune -o \
    -path "$root/target" -prune -o \
    -path "$root/server/target" -prune -o \
    -path "$root/client/.venv" -prune -o \
    -type f \( -name '.lanpull.lock' -o -name 'state.json' \
        -o -name '*.part' -o -name '.lanpull.partials.json' \) \
    -print -exec rm -f {} +

# Release archives are generated output; `make wipe` promises to remove
# everything lanpull created, so they go with the runtime leftovers.
rm -rf "$root/dist"
