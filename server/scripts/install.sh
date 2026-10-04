#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Install the lanpull binary, the systemd unit, and the staged client bundle.
# Build the binary first (`make build`); this script performs only the
# file-system and systemd steps and escalates them as needed.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

binary=""
bin_dir=""
unit_src=""
unit_dest=""
state_dir=""
operator=""
group=""
conf_dir=""
client_dir=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --binary) binary=$2; shift 2 ;;
        --bin-dir) bin_dir=$2; shift 2 ;;
        --unit-src) unit_src=$2; shift 2 ;;
        --unit-dest) unit_dest=$2; shift 2 ;;
        --state-dir) state_dir=$2; shift 2 ;;
        --operator) operator=$2; shift 2 ;;
        --group) group=$2; shift 2 ;;
        --conf-dir) conf_dir=$2; shift 2 ;;
        --client-dir) client_dir=$2; shift 2 ;;
        *) die "install: unknown argument: $1" ;;
    esac
done

[ -n "$binary" ] || die "install: --binary is required"
[ -f "$binary" ] || die "install: binary not found: $binary (run 'make build')"
[ -n "$bin_dir" ] || die "install: --bin-dir is required"
[ -n "$unit_src" ] || die "install: --unit-src is required"
[ -n "$unit_dest" ] || die "install: --unit-dest is required"
[ -n "$state_dir" ] || die "install: --state-dir is required"
[ -n "$operator" ] || die "install: --operator is required"
[ -n "$group" ] || die "install: --group is required"
[ -n "$conf_dir" ] || die "install: --conf-dir is required"
[ -n "$client_dir" ] || die "install: --client-dir is required"
require_safe_path "$state_dir" STATE_DIR
require_safe_path "$conf_dir" CONFIG_DIR

# 1. State directory, owned by the operator.
run_root install -d -m700 -o "$operator" -g "$group" "$state_dir"

# 2. Configuration directory. Root owns it; the operator group may traverse it so
#    the service (running as the operator) and the operator's CLI can read and
#    replace the mutable policy and account files. The configuration file itself
#    is created by `sudo lanpull init`, never by install; if it already exists,
#    install only normalizes its ownership and mode.
run_root install -d -m770 -o root -g "$group" "$conf_dir"
if [ -f "$conf_dir/lanpull.conf" ]; then
    run_root chown root:"$group" "$conf_dir/lanpull.conf"
    run_root chmod 640 "$conf_dir/lanpull.conf"
    echo "kept:      $conf_dir/lanpull.conf"
fi

# 3. Binary.
run_root install -Dm755 "$binary" "$bin_dir/lanpull"

# 4. systemd unit with the placeholders substituted. The unit always reads the
#    canonical configuration, never the development path.
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
sed -e "s|__LANPULL_USER__|$operator|g" \
    -e "s|__LANPULL_GROUP__|$group|g" \
    -e "s|__LANPULL_CONF__|$conf_dir/lanpull.conf|g" \
    -e "s|__LANPULL_STATE_DIR__|$state_dir|g" "$unit_src" >"$tmp"
run_root install -Dm644 "$tmp" "$unit_dest"
rm -f "$tmp"
trap - EXIT

# 5. Give the operator ownership of the state directory, then reload systemd.
run_root chown -R "$operator:$group" "$state_dir"
run_root systemctl daemon-reload

# 6. Stage the client bundle (the state directory is operator-owned now).
install -Dm755 "$client_dir/pull.py" "$state_dir/client/pull.py"
install -Dm644 "$client_dir/VERSION" "$state_dir/client/VERSION"

echo "installed: $bin_dir/lanpull"
echo "installed: $unit_dest"
echo "staged:    $state_dir/client"
