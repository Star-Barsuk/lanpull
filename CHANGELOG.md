# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [1.0.0] - 2026-10-24

The first supported release. The client script keeps its own technical version
(2.3.1) for self-update; the product version is 1.0.0.

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
  sides, loopback e2e), parser fuzzing, criterion baselines, and a local DAST
  (`testssl.sh` plus HTTP probes).
- Typed `--json` payloads and envelopes; a minimal `serve_tls` example that
  proves the transport is reusable.

### Changed

- The client gate runs on Python 3.11 with a hash-locked tool set.
- The release profile keeps overflow checks, thin LTO, one codegen unit, and
  strips symbols; the binary is built for `x86_64-unknown-linux-musl`.
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
- `cargo-deny`, `cargo-audit`, `cargo audit bin`, `cargo-geiger`, `gitleaks`,
  and `pip-audit` are part of the routine gates.
- The engineering standard, threat model, and decision log are maintained with
  the code; the public tree is checked for private references.
