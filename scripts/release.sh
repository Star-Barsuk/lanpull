#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Build the release archive. Refuses a dirty tree, requires a gate receipt
# that verifies for HEAD (set GATE_RECEIPT to hand over an out-of-tree one),
# checks that the third-party notices match the dependency graph, and
# writes dist/lanpull-v<version>.tar.gz plus a signed SHA256SUMS.
#
# It does not tag or push: review the archive, then create the annotated tag
# with the printed hash in its message.
#
# Usage: release.sh [--check]
#
# `--check` performs the same staging and validation from the working tree
# (which may be dirty) without requiring a commit. Use it as a release
# rehearsal; it still requires a verifying gate receipt and writes dist/.

set -eu
# The signing tool installs user-local through cargo.
PATH="$HOME/.cargo/bin:$PATH"

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
cd "$root"

# Reproducible archive timestamps: use the commit time unless the caller pins
# SOURCE_DATE_EPOCH explicitly.
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}
export SOURCE_DATE_EPOCH

check=0
for arg in "$@"; do
    case "$arg" in
        --check) check=1 ;;
        *) printf 'release: unknown argument: %s\n' "$arg" >&2; exit 1 ;;
    esac
done

# The toolchain and cargo-about live in the user's Cargo home; locate them
# without requiring the caller to edit PATH.
CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}
PATH="$CARGO_HOME/bin:$PATH"
export PATH

die() {
    printf 'release: %s\n' "$*" >&2
    exit 1
}

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
[ -n "$version" ] || die "cannot read the workspace version from Cargo.toml"
tag="v$version"
target=${TARGET:-x86_64-unknown-linux-musl}
binary="target/$target/release/lanpull"
dist="$root/dist"
stage="$dist/lanpull-$tag"

command -v python3 >/dev/null 2>&1 || die "python3 is required for the release notes"
command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required"
if [ "$check" = 1 ]; then
    printf 'release: rehearsal mode; a dirty tree is allowed and nothing is published\n' >&2
else
    [ -z "$(git status --porcelain)" ] || die "the working tree is not clean"
fi

printf 'release: verifying the gate receipt\n'
if [ -n "${GATE_RECEIPT:-}" ]; then
    # An out-of-tree receipt travels as a file; it must land in place to be
    # verified against this HEAD.
    mkdir -p target/kantei
    install -m644 "$GATE_RECEIPT" target/kantei/latest.json
fi
command -v kantei >/dev/null 2>&1 || die "kantei is required to verify the gate receipt"
kantei receipt verify --for push ||
    die "the gate receipt does not verify for HEAD; run 'kantei run --receipt' first"
# The receipt must be signed (GATE_SIGNING_KEY=<dir> kantei run --receipt);
# the public key is committed as release.pub, so anyone can check it.
if grep -qE '"signature" *: *null|"signature":null' target/kantei/latest.json; then
    die "the gate receipt is unsigned; take it with GATE_SIGNING_KEY=<dir> kantei run --receipt"
fi

printf 'release: building %s\n' "$target"
make build
[ -x "$binary" ] || die "binary not found at $binary"

rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/client" "$stage/systemd" "$stage/config" "$stage/docs" "$stage/server"
install -m755 "$binary" "$stage/bin/lanpull"
install -m755 client/pull.py "$stage/client/"
install -m644 client/VERSION client/pyproject.toml "$stage/client/"
install -m644 systemd/lanpull.service "$stage/systemd/"
install -m644 config/lanpull.conf.example "$stage/config/"
install -m644 Cargo.toml "$stage/server/"
install -m644 docs/REFERENCE.md docs/SECURITY.md docs/CONTRIBUTING.md docs/RELEASE.md "$stage/docs/"
install -m644 README.md LICENSE THIRD_PARTY_LICENSES.md "$stage/"

python3 - "$version" "$stage/RELEASE_NOTES.md" <<'PY'
import re
import sys

version, output = sys.argv[1], sys.argv[2]
text = open("CHANGELOG.md", encoding="utf-8").read()
heading = re.search(rf"^## \[{re.escape(version)}\].*$", text, re.MULTILINE)
if heading is None:
    heading = re.search(r"^## \[Unreleased\].*$", text, re.MULTILINE)
    if heading is None:
        raise SystemExit(
            f"release: CHANGELOG.md has no section for {version} and no Unreleased section"
        )
    print(
        f"release: warning: no CHANGELOG section for {version}; using Unreleased",
        file=sys.stderr,
    )
rest = text[heading.end():]
following = re.search(r"^## \[", rest, re.MULTILINE)
body = rest[: following.start()] if following else rest
with open(output, "w", encoding="utf-8") as handle:
    handle.write(f"# lanpull {version}\n")
    handle.write(body.rstrip() + "\n")
PY

# The staged artifact must identify itself as the version being released.
staged_version=$("$stage/bin/lanpull" --version | sed 's/^lanpull //')
[ "$staged_version" = "$version" ] \
    || die "staged binary reports $staged_version, expected $version"
staged_client=$(python3 "$stage/client/pull.py" --version)
expected_client=$(tr -d '[:space:]' < client/VERSION)
[ "$staged_client" = "$expected_client" ] \
    || die "staged client reports $staged_client, expected $expected_client"
if command -v file >/dev/null 2>&1; then
    file "$stage/bin/lanpull" | grep -q 'static' \
        || die "staged binary is not statically linked"
fi

# Every relative link in the staged Markdown must resolve inside the archive.
for document in $(find "$stage" -name '*.md' ! -name 'THIRD_PARTY_LICENSES.md' | sort); do
    directory=$(dirname "$document")
    links=$(grep -oE '\]\([^)]+' "$document" 2>/dev/null | sed 's/^](//' || true)
    while IFS= read -r link; do
        [ -n "$link" ] || continue
        case "$link" in
            http://*|https://*|mailto:*|'#'*) continue ;;
        esac
        link=${link%%[[:space:]]*}
        target=${link%%#*}
        [ -n "$target" ] || continue
        case "$target" in
            /*) candidate=$stage$target ;;
            *) candidate=$directory/$target ;;
        esac
        [ -e "$candidate" ] \
            || die "staged document has a broken link: ${document#"$stage"/}: $link"
    done <<EOF
$links
EOF
done

tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 \
    --numeric-owner -C "$dist" -cf "$dist/lanpull-$tag.tar" "lanpull-$tag"
gzip -n -f "$dist/lanpull-$tag.tar"
(cd "$dist" && sha256sum "lanpull-$tag.tar.gz" > SHA256SUMS && sha256sum -c SHA256SUMS >/dev/null)
# Sign the checksum list so a download can be authenticated, not merely
# checksummed: the checksum file alone travels over the same channel as the
# archive, and a compromised channel rewrites both. Anyone verifies with the
# committed release.pub and a standalone rsign; kantei is not involved.
command -v rsign >/dev/null 2>&1 ||
    die "rsign is required to sign SHA256SUMS (cargo install --locked rsign2)"
signing_key=${GATE_SIGNING_KEY:-$HOME/.config/kantei/keys/lanpull-release}
[ -f "$signing_key/release.key" ] ||
    die "release key not found at $signing_key/release.key; set GATE_SIGNING_KEY"
(cd "$dist" && rsign sign -W -s "$signing_key/release.key" -x SHA256SUMS.minisig SHA256SUMS) ||
    die "signing SHA256SUMS failed"
(cd "$dist" && rsign verify -p "$root/release.pub" SHA256SUMS) ||
    die "the SHA256SUMS signature does not verify against release.pub"
hash=$(cd "$dist" && sha256sum "lanpull-$tag.tar.gz" | cut -d' ' -f1)
git rev-parse HEAD >"$dist/COMMIT"

if tar -tzf "$dist/lanpull-$tag.tar.gz" | grep -Eq \
    '(^|/)(auth|lanpull\.conf|lanpull\.clients|lanpull\.access\.json|lanpull\.networks\.json|server\.key)$|\.(key|pem|p12|pfx|crt)$|(^|/)id_(rsa|dsa|ecdsa|ed25519)$|(^|/)\.env(\.|$)'; then
    die "the archive contains a configuration or credential file"
fi

printf '\nrelease: %s\n' "$dist/lanpull-$tag.tar.gz"
printf 'sha256:  %s\n' "$hash"
printf 'epoch:   %s\n' "$SOURCE_DATE_EPOCH"
printf 'contents:\n'
tar -tzf "$dist/lanpull-$tag.tar.gz" | sed 's/^/  /'
if [ "$check" = 1 ]; then
    printf '\nrelease-check: staging and checks passed; nothing was tagged or published\n'
    exit 0
fi
printf '\nnext: review the archive, then tag and push it:\n'
printf '  git tag -a %s -m "lanpull %s" -m "SHA256 (lanpull-%s.tar.gz): %s"\n' \
    "$tag" "$version" "$tag" "$hash"
printf '  git push origin %s\n' "$tag"
