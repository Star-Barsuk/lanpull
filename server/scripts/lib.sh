#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Shared helpers for the lanpull server scripts. Sourced, not executed.

# Print an error and exit.
die() {
    echo "lanpull: $*" >&2
    exit 1
}

# Run a privileged command. Inside a terminal, sudo may prompt for a password;
# without one, `sudo -n` fails fast instead of hanging on a prompt.
run_root() {
    if [ "$(id -u)" = "0" ]; then
        "$@"
    elif [ -t 0 ]; then
        sudo "$@"
    else
        sudo -n "$@"
    fi
}

# Refuse a path that is empty or "/", so a destructive command cannot escalate.
require_safe_path() {
    value=$1
    label=$2
    [ -n "$value" ] || die "refusing to touch empty $label"
    [ "$value" != "/" ] || die "refusing to touch $label=/"
}
