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

# Refuse a path that is empty, "/", a system root, or the invoking user's home,
# so a destructive command cannot escalate by a misconfigured variable.
require_safe_path() {
    value=$1
    label=$2
    [ -n "$value" ] || die "refusing to touch empty $label"
    case "$value" in
        /|/bin|/boot|/dev|/etc|/home|/lib|/lib64|/opt|/proc|/root|/run|/sbin|/srv|/sys|/tmp|/usr|/var)
            die "refusing to touch system path $label=$value"
            ;;
    esac
    if [ -n "${HOME:-}" ] && [ "$value" = "$HOME" ]; then
        die "refusing to touch $label=\$HOME"
    fi
}
