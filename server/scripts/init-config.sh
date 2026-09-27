#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Generate a real config/lanpull.conf for the current machine.
#
# The script is generic: it derives the server address from the routing table
# instead of hardcoding interface names or addresses, so the same logic works
# on any Linux host. Values may be overridden with flags or interactively.

set -eu

CONFIG="config/lanpull.conf"
SHARE_DIR=""
STATE_DIR="/var/lib/lanpull"
BIND="0.0.0.0"
PORT="8000"
SERVER_IP=""
INTERACTIVE=1

usage() {
    cat <<'EOF'
Usage: init-config.sh [options]

Options:
  --config <path>    Configuration file to write (default: config/lanpull.conf).
  --share <dir>      SHARE_DIR: directory to distribute (default: $HOME/lanpull-share).
  --state <dir>      STATE_DIR: manifest, TLS material, log (default: /var/lib/lanpull).
  --bind <addr>      BIND listen address (default: 0.0.0.0).
  --port <port>      PORT listen port (default: 8000).
  --server-ip <ip>   SERVER_IP embedded in the certificate SAN (default: detected).
  --non-interactive  Accept the detected values without prompting.
  -h, --help         Show this help.
EOF
}

# True when the address is in an RFC1918 private range. This is the single
# acceptance rule and it excludes special-use ranges (198.18.0.0/15,
# 100.64.0.0/10, 169.254.0.0/16, loopback) by construction.
is_private() {
    case "$1" in
        10.*) return 0 ;;
        192.168.*) return 0 ;;
        172.1[6-9].*|172.2[0-9].*|172.3[01].*) return 0 ;;
        *) return 1 ;;
    esac
}

# True when the string looks like an IPv4 address.
is_ipv4() {
    case "$1" in
        *[!0-9.]*|.*|*.) return 1 ;;
    esac
    old_ifs=$IFS
    IFS=.
    # shellcheck disable=SC2086
    set -- $1
    IFS=$old_ifs
    [ "$#" -eq 4 ] || return 1
    for octet in "$@"; do
        [ -n "$octet" ] || return 1
        [ "$octet" -ge 0 ] 2>/dev/null && [ "$octet" -le 255 ] || return 1
    done
    return 0
}

# Pick the host's LAN address without naming any interface. The main routing
# table's default route is preferred; policy-routing tables used by proxies do
# not appear there, so their synthetic addresses are never chosen.
detect_server_ip() {
    cand=$(ip -4 route show default 2>/dev/null \
        | awk '{for (i=1;i<=NF;i++) if ($i=="src") {print $(i+1); exit}}')
    if is_private "$cand"; then
        printf '%s' "$cand"
        return 0
    fi

    dev=$(ip -4 route show default 2>/dev/null \
        | awk '{for (i=1;i<=NF;i++) if ($i=="dev") {print $(i+1); exit}}')
    if [ -n "$dev" ]; then
        cand=$(ip -4 -o addr show dev "$dev" scope global 2>/dev/null \
            | awk '{print $4}' | cut -d/ -f1 | head -n1)
        if is_private "$cand"; then
            printf '%s' "$cand"
            return 0
        fi
    fi

    ip -4 -o addr show scope global 2>/dev/null \
        | awk '{print $4}' | cut -d/ -f1 \
        | while IFS= read -r addr; do
            if is_private "$addr"; then
                printf '%s' "$addr"
                break
            fi
        done
}

# Print the current value, or a replacement typed on the terminal.
#
# The prompt is skipped when there is no usable controlling terminal (for
# example when run non-interactively), which keeps the defaults.
prompt_value() {
    current=$1
    label=$2
    if [ "$INTERACTIVE" = 1 ] && ( : </dev/tty ) 2>/dev/null; then
        printf '%s [%s]: ' "$label" "$current" >/dev/tty
        answer=""
        IFS= read -r answer </dev/tty || answer=""
        if [ -n "$answer" ]; then
            current=$answer
        fi
    fi
    printf '%s' "$current"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --config) CONFIG=$2; shift 2 ;;
        --share) SHARE_DIR=$2; shift 2 ;;
        --state) STATE_DIR=$2; shift 2 ;;
        --bind) BIND=$2; shift 2 ;;
        --port) PORT=$2; shift 2 ;;
        --server-ip) SERVER_IP=$2; shift 2 ;;
        --non-interactive|-y) INTERACTIVE=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "init-config: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

[ -n "$SHARE_DIR" ] || SHARE_DIR="${HOME:-/home}/lanpull-share"
[ -n "$SERVER_IP" ] || SERVER_IP=$(detect_server_ip)

SHARE_DIR=$(prompt_value "$SHARE_DIR" "SHARE_DIR (files to distribute)")
STATE_DIR=$(prompt_value "$STATE_DIR" "STATE_DIR (manifest, TLS, log)")
BIND=$(prompt_value "$BIND" "BIND (listen address)")
PORT=$(prompt_value "$PORT" "PORT (listen port)")
SERVER_IP=$(prompt_value "$SERVER_IP" "SERVER_IP (LAN address, in the certificate)")

if ! is_ipv4 "$SERVER_IP"; then
    echo "init-config: SERVER_IP is not a valid IPv4 address: '$SERVER_IP'" >&2
    echo "             pass --server-ip <ip> (the address clients use)" >&2
    exit 1
fi
case "$PORT" in
    ''|*[!0-9]*) echo "init-config: PORT is not numeric: '$PORT'" >&2; exit 1 ;;
esac

CERT_PATH="$STATE_DIR/server.crt"
KEY_PATH="$STATE_DIR/server.key"
AUDIT_LOG="$STATE_DIR/access.log"
CLIENTS_PATH="lanpull.clients"

mkdir -p "$(dirname "$CONFIG")"
if [ ! -d "$SHARE_DIR" ]; then
    mkdir -p "$SHARE_DIR"
fi

if [ ! -d "$STATE_DIR" ]; then
    if [ "$(id -u)" = 0 ]; then
        install -d -m700 "$STATE_DIR"
    elif command -v sudo >/dev/null 2>&1; then
        sudo install -d -m700 -o "$(id -un)" -g "$(id -gn)" "$STATE_DIR"
    else
        echo "init-config: create '$STATE_DIR' owned by $(id -un) (mode 700)" >&2
        exit 1
    fi
fi

umask 077
tmp="$CONFIG.tmp.$$"
trap 'rm -f "$tmp"' EXIT
{
    printf '# lanpull server configuration.\n'
    printf '# Generated by server/scripts/init-config.sh. Mode 600; not committed.\n\n'
    printf 'SHARE_DIR=%s\n' "$SHARE_DIR"
    printf 'STATE_DIR=%s\n' "$STATE_DIR"
    printf 'BIND=%s\n' "$BIND"
    printf 'PORT=%s\n' "$PORT"
    printf 'SERVER_IP=%s\n' "$SERVER_IP"
    printf 'CERT_PATH=%s\n' "$CERT_PATH"
    printf 'KEY_PATH=%s\n' "$KEY_PATH"
    printf 'CLIENTS_PATH=%s\n' "$CLIENTS_PATH"
    printf 'AUDIT_LOG=%s\n' "$AUDIT_LOG"
} >"$tmp"
chmod 600 "$tmp"
mv -f "$tmp" "$CONFIG"
trap - EXIT

echo "wrote $CONFIG"
echo "  SHARE_DIR=$SHARE_DIR"
echo "  STATE_DIR=$STATE_DIR"
echo "  SERVER_IP=$SERVER_IP"

if ip -4 -o addr show scope global 2>/dev/null | grep -F " $SERVER_IP/" | grep -q dynamic; then
    echo "warning: $SERVER_IP is a dynamic (DHCP) lease."
    echo "         It is embedded in the certificate; if it changes, run"
    echo "         'make cert' and copy server.crt to every client."
fi
