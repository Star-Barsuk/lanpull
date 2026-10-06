#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Remove the installed lanpull binary, unit, and state. The configuration
# directory is removed too when present. Every destructive path is guarded
# against empty or "/".

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

binary=""
unit=""
state_dir=""
conf_dir=""
service="lanpull.service"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --binary) binary=$2; shift 2 ;;
        --unit) unit=$2; shift 2 ;;
        --state-dir) state_dir=$2; shift 2 ;;
        --conf-dir) conf_dir=$2; shift 2 ;;
        --service) service=$2; shift 2 ;;
        *) die "uninstall: unknown argument: $1" ;;
    esac
done

[ -n "$unit" ] || die "uninstall: --unit is required"
[ -n "$state_dir" ] || die "uninstall: --state-dir is required"
require_safe_path "$state_dir" STATE_DIR
if [ -n "$conf_dir" ]; then
    require_safe_path "$conf_dir" CONFIG_DIR
fi

# Stop the unit; tolerate an absent unit. The unit has no [Install] section, so
# `systemctl disable` would be a no-op and is intentionally not called.
run_root systemctl stop "$service" 2>/dev/null || true
run_root rm -f "$unit"
run_root systemctl daemon-reload 2>/dev/null || true

if [ -n "$binary" ]; then
    run_root rm -f "$binary"
fi

run_root rm -rf "$state_dir"
echo "removed: $state_dir"

if [ -n "$conf_dir" ] && [ -d "$conf_dir" ]; then
    run_root rm -rf "$conf_dir"
    echo "removed: $conf_dir"
fi

echo "uninstalled: binary, unit, and state"
