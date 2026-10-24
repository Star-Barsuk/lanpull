# Release process

lanpull releases are built locally from a tagged commit. There is no hosted
CI or artifact registry: the archive and its checksum are handed to the team
directly.

## Versioning

- The product version lives in `[workspace.package]` in `server/Cargo.toml`;
  the git tag is `v<version>` and follows Semantic Versioning.
- `client/VERSION` is the client's own self-update version (2.3.1 in the 1.0.0
  release). It changes only when `pull.py` changes, and is listed in the
  release notes.
- `CHANGELOG.md` is the source of the release notes; the archive carries the
  matching `RELEASE_NOTES.md`.

## One-time setup

```bash
make setup                       # Rust toolchain and the gate tools
make -C server release-setup     # cargo-about (release only)
```

## Build

The working tree must be clean: the release is built from a commit, and the
script refuses uncommitted changes. It runs `make ci-full`, builds the static
binary, checks that the third-party notices match the dependency graph, and
writes the archive.

```bash
git status --short               # must be empty
make release
```

Output:

```text
dist/lanpull-v1.0.0.tar.gz
dist/SHA256SUMS
dist/lanpull-v1.0.0/             # staging directory, safe to delete
```

The script prints the archive hash. After reviewing the archive, create an
annotated tag and put the hash in its message, so the checksum is available
from git as well as next to the archive:

```bash
git tag -a v1.0.0 -m "lanpull 1.0.0" \
  -m "SHA256 (lanpull-v1.0.0.tar.gz): <hash>"
git push origin main --tags
```

## Archive contents

```text
bin/lanpull                     static x86_64-unknown-linux-musl binary
client/pull.py, client/VERSION  client bundle for --self-update
systemd/lanpull.service         unit template
config/lanpull.conf.example     configuration template
README.md  LICENSE
THIRD_PARTY_LICENSES.md         licenses of the statically linked crates
RELEASE_NOTES.md                the CHANGELOG section for this version
```

The archive contains no configuration, credentials, keys, or `CHANGELOG.md`.

## Verify

```bash
sha256sum -c SHA256SUMS
tar -xzf lanpull-v1.0.0.tar.gz
./lanpull-v1.0.0/bin/lanpull --version          # 1.0.0
./lanpull-v1.0.0/client/pull.py --version       # 2.3.1
```

Compare the hash with the one in the annotated tag (`git show v1.0.0`). The
checksum detects a corrupted or replaced archive when the tag is obtained from
a different channel than the archive.

## Rollback

Delete the tag and the distributed archive, fix the problem, bump the patch
version, and rebuild. Never re-tag the same version with different bytes: the
hash in the tag message is the record of what was released.

## Explicitly out of scope

- SBOM generation (the lockfile and `cargo audit bin` cover the inventory).
- Artifact signing (checksums only for now).
- Reproducible builds byte-for-byte.
- A hosted registry or automated release pipeline.
