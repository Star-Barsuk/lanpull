#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# lanpull-core must stay free of the HTTP stack; lanpull-store and lanpull-cli
# must not depend on axum directly. The rule is project policy, so it lives
# here, not in the shared catalogue.

set -eu

CARGO="${CARGO:-cargo}"
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
[ -x "$CARGO_BIN/cargo" ] && CARGO="$CARGO_BIN/cargo"

deps=$("$CARGO" tree -p lanpull-core --edges all --prefix none) || {
	echo "lanpull: cargo tree failed; cannot verify lanpull-core" >&2
	exit 1
}
if printf '%s\n' "$deps" | grep -Eq '^(axum|axum-server|tower|tower-http|hyper|hyper-util) v'; then
	echo "lanpull: lanpull-core must not depend on the HTTP stack" >&2
	printf '%s\n' "$deps" | grep -E '^(axum|axum-server|tower|tower-http|hyper|hyper-util) v' >&2
	exit 1
fi

for crate in lanpull-store lanpull-cli; do
	deps=$("$CARGO" tree -p "$crate" --depth 1 --edges normal --prefix none) || {
		echo "lanpull: cargo tree failed for $crate" >&2
		exit 1
	}
	if printf '%s\n' "$deps" | grep -Eq '^(axum|axum-server) v'; then
		echo "lanpull: $crate must not depend on axum directly" >&2
		printf '%s\n' "$deps" | grep -E '^(axum|axum-server) v' >&2
		exit 1
	fi
done
