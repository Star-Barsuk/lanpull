#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Keep the cargo-audit and cargo-deny advisory ignore lists identical. Both
# tools read the same advisory database, but each has its own configuration
# file; an ignore added to one and not the other makes the checks disagree.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$script_dir/.." && pwd)

ignored_ids() {
    sed -n '/^\[advisories\]/,/^\[/p' "$1" \
        | grep -E '^[[:space:]]*id[[:space:]]*=' \
        | sed 's/.*"\([^"]*\)".*/\1/' \
        | sort
}

deny=$(ignored_ids "$root/deny.toml")
audit=$(ignored_ids "$root/.cargo/audit.toml")
if [ "$deny" != "$audit" ]; then
    printf 'lanpull: cargo-deny and cargo-audit ignore different advisories\n' >&2
    printf '  deny:  %s\n' "${deny:-none}" >&2
    printf '  audit: %s\n' "${audit:-none}" >&2
    exit 1
fi
