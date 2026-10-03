#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Control the lanpull systemd service. Invoked by `make up|down|restart|logs`.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib.sh
. "$script_dir/lib.sh"

action=${1:-}
service=${2:-lanpull.service}
case "$action" in
    start | stop | restart)
        run_root systemctl "$action" "$service"
        ;;
    logs)
        run_root journalctl -u "$service" -f
        ;;
    *)
        die "usage: service.sh start|stop|restart|logs [service]"
        ;;
esac
