#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Negative safety tests for the privileged scripts. These checks run without
# root: every case must be refused by argument validation or `require_safe_path`
# before the script reaches a system-changing command.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/lanpull-guard.XXXXXX")
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT INT TERM

fail=0
pass() { printf 'ok   - %s\n' "$1"; }
fail_msg() {
    printf 'FAIL - %s\n' "$1" >&2
    fail=1
}
expect_fail() { # <description> <command...>
    description=$1
    shift
    if "$@" >/dev/null 2>&1; then
        fail_msg "$description (unexpected success)"
    else
        pass "$description"
    fi
}

expect_success() { # <description> <command...>
    description=$1
    shift
    if "$@" >/dev/null 2>&1; then
        pass "$description"
    else
        fail_msg "$description (unexpected failure)"
    fi
}

client=$tmp/client
unit=$tmp/lanpull.service
mkdir -p "$client"
printf '[Unit]\n' >"$unit"

expect_fail "install refuses a system path as STATE_DIR" \
    "$script_dir/install.sh" --binary /bin/true --bin-dir "$tmp/bin" \
    --unit-src "$unit" --unit-dest "$tmp/unit" --state-dir / \
    --operator "$(id -un)" --group "$(id -gn)" --conf-dir "$tmp/conf" \
    --client-dir "$client"

expect_fail "install refuses HOME as STATE_DIR" \
    "$script_dir/install.sh" --binary /bin/true --bin-dir "$tmp/bin" \
    --unit-src "$unit" --unit-dest "$tmp/unit" --state-dir "$HOME" \
    --operator "$(id -un)" --group "$(id -gn)" --conf-dir "$tmp/conf" \
    --client-dir "$client"

expect_fail "uninstall refuses a system path as STATE_DIR" \
    "$script_dir/uninstall.sh" --binary "$tmp/lanpull" --unit "$tmp/unit" \
    --state-dir / --conf-dir "$tmp/conf" --service lanpull-test.service

expect_fail "uninstall refuses HOME as CONFIG_DIR" \
    "$script_dir/uninstall.sh" --binary "$tmp/lanpull" --unit "$tmp/unit" \
    --state-dir "$tmp/state" --conf-dir "$HOME" --service lanpull-test.service

expect_fail "uninstall requires the unit path" \
    "$script_dir/uninstall.sh" --state-dir "$tmp/state" --conf-dir "$tmp/conf"

expect_fail "distclean refuses a system path" "$script_dir/distclean.sh" /
expect_fail "distclean refuses HOME" "$script_dir/distclean.sh" "$HOME"
expect_fail "distclean refuses a missing directory" "$script_dir/distclean.sh" "$tmp/missing"

expect_success "install --dry-run accepts a temporary layout" \
    "$script_dir/install.sh" --binary /bin/true --bin-dir "$tmp/bin" \
    --unit-src "$unit" --unit-dest "$tmp/unit" --state-dir "$tmp/state" \
    --operator "$(id -un)" --group "$(id -gn)" --conf-dir "$tmp/conf" \
    --client-dir "$client" --dry-run

expect_success "uninstall --dry-run accepts a temporary layout" \
    "$script_dir/uninstall.sh" --binary "$tmp/lanpull" --unit "$tmp/unit" \
    --state-dir "$tmp/state" --conf-dir "$tmp/conf" \
    --service lanpull-test.service --dry-run

if [ "$fail" -eq 0 ]; then
    echo "guard tests passed"
else
    echo "guard tests failed" >&2
    exit 1
fi
