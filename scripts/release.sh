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
#
# Usage: release.sh [--check]
#
# `--check` performs the same staging and validation from the working tree
# (which may be dirty) without requiring a commit. Use it as a release
# rehearsal; it still runs `make ci-full` and still writes dist/.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
cd "$root"

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

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' server/Cargo.toml | head -n 1)
[ -n "$version" ] || die "cannot read the workspace version from server/Cargo.toml"
tag="v$version"
target=${TARGET:-x86_64-unknown-linux-musl}
binary="server/target/$target/release/lanpull"
dist="$root/dist"
stage="$dist/lanpull-$tag"

command -v python3 >/dev/null 2>&1 || die "python3 is required for the release notes"
command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required"
if [ "$check" = 1 ]; then
    printf 'release: rehearsal mode; a dirty tree is allowed and nothing is published\n' >&2
else
    [ -z "$(git status --porcelain)" ] || die "the working tree is not clean"
fi

printf 'release: running the gate\n'
make ci-full

printf 'release: building %s\n' "$target"
make -C server build
[ -x "$binary" ] || die "binary not found at $binary"

rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/client" "$stage/systemd" "$stage/config" "$stage/docs" "$stage/server"
install -m755 "$binary" "$stage/bin/lanpull"
install -m755 client/pull.py "$stage/client/"
install -m644 client/VERSION client/pyproject.toml "$stage/client/"
install -m644 systemd/lanpull.service "$stage/systemd/"
install -m644 config/lanpull.conf.example "$stage/config/"
install -m644 server/Cargo.toml "$stage/server/"
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

tar -C "$dist" -czf "$dist/lanpull-$tag.tar.gz" "lanpull-$tag"
(cd "$dist" && sha256sum "lanpull-$tag.tar.gz" > SHA256SUMS && sha256sum -c SHA256SUMS >/dev/null)
hash=$(cd "$dist" && sha256sum "lanpull-$tag.tar.gz" | cut -d' ' -f1)
git rev-parse HEAD >"$dist/COMMIT"

if tar -tzf "$dist/lanpull-$tag.tar.gz" | grep -Eq \
    '(^|/)(auth|lanpull\.conf|lanpull\.clients|lanpull\.access\.json|lanpull\.networks\.json|server\.key)$|\.(key|pem|p12|pfx|crt)$|(^|/)id_(rsa|dsa|ecdsa|ed25519)$|(^|/)\.env(\.|$)'; then
    die "the archive contains a configuration or credential file"
fi

printf '\nrelease: %s\n' "$dist/lanpull-$tag.tar.gz"
printf 'sha256:  %s\n' "$hash"
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
