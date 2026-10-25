# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

The planned first supported release. The client script keeps its own technical
version (2.3.1) for self-update; the product version is 1.0.0. The 1.0.0 tag
and GitHub Release were withdrawn before publication; the content below is the
release candidate.

### Added

- Named shares (`SHARE_<name>`), a per-client JSON access policy with a public
  set and per-client additions/removals, and per-account filtered manifests.
- Per-client argon2 accounts, an optional source-IP binding, an operator `arm`
  window, and an audit log with `lanpull report`.
- Self-share: the server can mirror a share onto another local disk over
  loopback with a `local` account.
- A single `lanpull` CLI with a unified output contract (`--json`, streams,
  exit codes, `--dry-run`, `--yes`).
- Network profiles for a server that moves between LANs, each with its own
  certificate.
- Client: resumable transfers with `Range`/`If-Range`, sha256 verification,
  `--check`, `--mirror`, `--clean`, `--self-update`, and one shared connection
  per run with a short retry on a dropped transfer.
- Systemd unit, install/uninstall/wipe lifecycle, and a full teardown boundary.
- Quality and security gates: `make ci-full` (secret scan, docs check, both
  sides, release-binary audit, loopback e2e), parser fuzzing, criterion
  baselines, and a local DAST (`testssl.sh` plus HTTP probes).
- Typed `--json` payloads and envelopes; a minimal `serve_tls` example that
  proves the transport is reusable.

### Changed

- The client gate runs on Python 3.11 with a hash-locked tool set and now
  checks that `requirements-dev.txt` matches `requirements-dev.in`;
  `make client-format` formats the client.
- Unused license allowances were removed from `deny.toml`/`about.toml`, and the
  detached fuzz lockfile was refreshed to match the workspace crates.
- The release profile keeps overflow checks, thin LTO, one codegen unit, and
  strips symbols; the binary is built for `x86_64-unknown-linux-musl`.
- The pipeline is split into `make check`, `make test-all`, `make ci`, and
  `make ci-full`; the CLI test uses the debug binary and the release-binary
  audit belongs to the full gate. `cargo-geiger` is advisory, not blocking.
- `make ci-full` requires gitleaks and shellcheck, so the full gate cannot be
  silently weaker on a machine without them.
- `make ci-full` checks the committed third-party notices; `cargo-deny` blocks
  advisories, while the full `cargo audit` scan is a manual `security` step.
- `make setup` records the installed gate-tool versions and skips the reinstall
  when they still match; `make -C server msrv` checks the declared MSRV 1.88
  when that toolchain is installed; `make doctor` prints the toolchain and
  optional-tool status.
- The archive ships the public docs and build metadata so its README links
  resolve; `make release-check` rehearses the layout from a working tree.
- Release and publication are guarded: `release.sh` checks the staged versions
  and the static link and records `dist/COMMIT`; `publish.sh` requires an
  existing tag at HEAD with the matching hash and never creates or pushes tags.
- `docs-check` resolves links from the document directory, derives the public
  file set from the git index, keeps generated trees out, and checks that
  private documents and `tools/` are untracked.
- The gate compiles benches and examples (`check-targets`), checks that
  `cargo-audit` and `cargo-deny` ignore the same advisories, and runs negative
  safety tests for the privileged scripts (`guard-test`).
- Coverage commands have low regression floors (53% Rust test-only lines, 75%
  with CLI/e2e instrumentation, 80% Python combined) while staying advisory.
  `make -C server load` and `soak` run concurrent loopback clients, and the
  periodic DAST passed against the release binary.
- The release archive is reproducible: deterministic tar metadata, commit-time
  `SOURCE_DATE_EPOCH`, gzip without timestamps, and `make release-repro` builds
  it twice and compares the hashes.
- `make -C server test-musl` runs the suite against the musl target, and
  `make -C server fuzz-check` runs stable Clippy over the detached fuzz crate.
- LLVM coverage profiles are written under `target/coverage-e2e/`; the target
  cleans the instrumented build and profiles automatically, and
  `make -C server coverage-clean` removes leftovers.
- The systemd unit drops all capabilities, uses a `@system-service` syscall
  allowlist, private devices, clock/log protection, and `UMask=0077`
  (`systemd-analyze security` exposure 1.7 OK).
- The audit log rotates at 16 MiB, and `report` sanitizes control characters.
- Client progress is shown only on a terminal; redirected output gets
  summaries.

### Fixed

- Closed RUSTSEC-2026-0382 (`rustls-webpki`) and RUSTSEC-2026-0386 (`tokio`).
- An unknown account and a wrong password are indistinguishable (`auth_failed`);
  password verification is cached for 30 s instead of running argon2 per file.
- The client bundle serves only `pull.py` and `VERSION`, so a stray `auth` or
  configuration file cannot overwrite a client.
- `sudo lanpull init` hands a newly created share to the operator; address
  detection accepts only RFC1918 or loopback.
- `account remove` deletes the staged folder that held a plaintext password.
- The client no longer prints a traceback for an `OSError`, Ctrl-C, or a
  missing terminal, and writes `state.json` atomically.

### Security

- TLS is pinned to a self-signed certificate; there is no unverified fallback.
- `cargo-deny`, `cargo-audit`, `cargo audit bin`, `gitleaks`, and `pip-audit`
  are part of the routine gates; `cargo-geiger` is an advisory inventory.
- The engineering standard, threat model, and decision log are maintained with
  the code; the public tree is checked for private references.
