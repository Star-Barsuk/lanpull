#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Build the debug server and drive the loopback end-to-end round. This is the
# gate entry point; `scripts/e2e.sh` stays the focused scenario.

set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

CARGO="${CARGO:-cargo}"
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
[ -x "$CARGO_BIN/cargo" ] && CARGO="$CARGO_BIN/cargo"

"$CARGO" build --locked
exec "$root/scripts/e2e.sh" "$root/target/debug/lanpull" "$root/client"
