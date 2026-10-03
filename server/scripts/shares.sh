#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Print the absolute directory of every `SHARE_<name>=<path>` entry in a
# configuration file, one per line. Used by teardown to remove every share
# safely without a shell in the Makefile.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

config=${1:-}
[ -n "$config" ] || die "shares: configuration path is required"
[ -f "$config" ] || die "shares: no such file: $config"

base=$(dirname -- "$config")

awk -F= '
    /^[[:space:]]*#/ { next }
    $1 ~ /^SHARE_[a-z0-9][a-z0-9_-]*[[:space:]]*$/ {
        key = $1
        sub(/[[:space:]]+$/, "", key)
        value = $0
        sub(/^[^=]*=/, "", value)
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
        gsub(/^["'\'']|["'\'']$/, "", value)
        if (value != "") print key "\t" value
    }
' "$config" | while IFS="$(printf '\t')" read -r _key value; do
    case "$value" in
        /*) printf '%s\n' "$value" ;;
        *) printf '%s/%s\n' "$base" "$value" ;;
    esac
done
