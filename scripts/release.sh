#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Build the release archive. Refuses a dirty tree, runs the full gate, checks
# that the third-party notices match the dependency graph, and writes
# dist/lanpull-v<version>.tar.gz plus SHA256SUMS.
#
# It does not tag or push: review the archive, then create the annotated tag
# with the printed hash in its message.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
cd "$root"

# The toolchain and cargo-about live in the user's Cargo home; locate them
# without requiring the caller to edit PATH.
CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}
PATH="$CARGO_HOME/bin:$PATH"
export PATH

die() {
    printf 'release: %s\n' "$*" >&2
    exit 1
}

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' server/Cargo.toml | head -n 1)
[ -n "$version" ] || die "cannot read the workspace version from server/Cargo.toml"
tag="v$version"
target=${TARGET:-x86_64-unknown-linux-musl}
binary="server/target/$target/release/lanpull"
dist="$root/dist"
stage="$dist/lanpull-$tag"

command -v cargo-about >/dev/null 2>&1 || die "cargo-about is missing; run 'make -C server release-setup'"
command -v python3 >/dev/null 2>&1 || die "python3 is required for the release notes"
command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required"
[ -z "$(git status --porcelain)" ] || die "the working tree is not clean"

printf 'release: running the gate\n'
make ci-full

printf 'release: building %s\n' "$target"
make -C server build
[ -x "$binary" ] || die "binary not found at $binary"

printf 'release: checking third-party notices\n'
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
cargo about generate --manifest-path server/Cargo.toml --config server/about.toml \
    server/about.hbs -o "$tmp"
cmp -s "$tmp" THIRD_PARTY_LICENSES.md \
    || die "THIRD_PARTY_LICENSES.md is stale; run 'make -C server licenses'"

rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/client" "$stage/systemd" "$stage/config"
install -m755 "$binary" "$stage/bin/lanpull"
install -m644 client/pull.py client/VERSION "$stage/client/"
install -m644 systemd/lanpull.service "$stage/systemd/"
install -m644 config/lanpull.conf.example "$stage/config/"
install -m644 README.md LICENSE THIRD_PARTY_LICENSES.md "$stage/"

python3 - "$version" "$stage/RELEASE_NOTES.md" <<'PY'
import re
import sys

version, output = sys.argv[1], sys.argv[2]
text = open("CHANGELOG.md", encoding="utf-8").read()
heading = re.search(rf"^## \[{re.escape(version)}\].*$", text, re.MULTILINE)
if heading is None:
    body = "See CHANGELOG.md in the source repository.\n"
else:
    rest = text[heading.end():]
    following = re.search(r"^## \[", rest, re.MULTILINE)
    body = rest[: following.start()] if following else rest
with open(output, "w", encoding="utf-8") as handle:
    handle.write(f"# lanpull {version}\n")
    handle.write(body.rstrip() + "\n")
PY

tar -C "$dist" -czf "$dist/lanpull-$tag.tar.gz" "lanpull-$tag"
(cd "$dist" && sha256sum "lanpull-$tag.tar.gz" > SHA256SUMS && sha256sum -c SHA256SUMS >/dev/null)
hash=$(cd "$dist" && sha256sum "lanpull-$tag.tar.gz" | cut -d' ' -f1)

if tar -tzf "$dist/lanpull-$tag.tar.gz" | grep -Eq '(^|/)(auth|lanpull\.conf|lanpull\.clients|lanpull\.access\.json|lanpull\.networks\.json|server\.key)$'; then
    die "the archive contains a configuration or credential file"
fi

printf '\nrelease: %s\n' "$dist/lanpull-$tag.tar.gz"
printf 'sha256:  %s\n' "$hash"
printf 'contents:\n'
tar -tzf "$dist/lanpull-$tag.tar.gz" | sed 's/^/  /'
printf '\nnext: review the archive, then tag it:\n'
printf '  git tag -a %s -m "lanpull %s" -m "SHA256 (lanpull-%s.tar.gz): %s"\n' \
    "$tag" "$version" "$tag" "$hash"
