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

# Keep this gate's artifacts out of the shared target directory: a concurrent
# gate can replace target/debug/lanpull while the test is running. Honor an
# outer CARGO_TARGET_DIR (the hermetic run sets one outside the tree).
target="${CARGO_TARGET_DIR:-$root/target}/kantei/e2e-lanpull"
export CARGO_TARGET_DIR="$target"

"$CARGO" build --locked
exec "$root/scripts/e2e.sh" "$target/debug/lanpull" "$root/client"
