# lanpull

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](server/Cargo.toml)
[![Python](https://img.shields.io/badge/python-3.11%2B-blue.svg)](client/pyproject.toml)
[![Platform](https://img.shields.io/badge/platform-Linux-lightgrey.svg)](#requirements)

A minimal, manual file-distribution tool for a local network.

The server publishes a shared folder over HTTPS; clients pull the latest
version on demand, replacing changed files and removing files that no longer
exist on the server. There is no automatic synchronization, no version history,
and no rollback.

`lanpull` is a contraction of **LAN** + **pull**.

## Table of contents

- [Why lanpull](#why-lanpull)
- [Features](#features)
- [How it works](#how-it-works)
- [Requirements](#requirements)
- [Installation](#installation)
- [Client usage](#client-usage)
- [Configuration](#configuration)
- [Security model](#security-model)
- [Repository layout](#repository-layout)
- [Development](#development)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)
- [License](#license)

## Why lanpull

Sometimes a small group of machines on one LAN needs to exchange large office
files — presentations, documents, PDFs — that are updated rarely and are
replaced whole. A background sync daemon is then more machinery than the job
needs: continuous watching, conflict resolution, and unbounded version history
that no one asked for.

`lanpull` takes the opposite approach. The owner edits the files on one
machine (the server), and the operator walks to each client and runs one
explicit command that fetches only what changed, resumably, over a pinned TLS
channel. Nothing runs in the background, nothing is stored twice, and every
pull is an intentional act.

It is not a replacement for Git, Syncthing, or Nextcloud. It is a deliberately
small pull-only mirror for a trusted local network.

## Features

- **Pull-only and explicit** — no daemon, no watcher, no auto-sync.
- **Resumable transfers** — a dropped connection at 350 MB resumes from where
  it stopped, guarded by `If-Range`.
- **Verified** — every file is checked against its `sha256` before it replaces
  anything.
- **Atomic replacement** — partial downloads live in `.part` files and are
  renamed into place, so a partial file is never visible.
- **Pinned HTTPS** — the client trusts exactly one self-signed certificate and
  never falls back to an unverified connection.
- **Per-client accounts** — argon2-hashed passwords, optionally bound to the
  client's IP address.
- **Arm window** — an account is accepted only while the operator has armed it,
  so a copied client folder does nothing on another machine.
- **Self-updating client** — `pull.py` refreshes itself from the server, so
  clients need no `git` and no internet access.
- **Auditable** — every request, including rejections, is logged and summarized
  with `lanpull report`.
- **Standard-library client** — the client is one `python3` script.

## How it works

```text
         Owner's PC (server, full admin, Linux)
         ---------------------------------------
         $SHARE_DIR/                files to distribute (any path)
         lanpull (single Rust binary)
           serve        HTTPS on $BIND:$PORT, per-client auth + arm, Range
           manifest     regenerate $STATE_DIR/manifest.json atomically
           add-client   create an account + stage a ready client folder
           arm/disarm   authorize a client for a short window
           report       who pulled what (from the audit log)
           cert         self-signed certificate (SAN = IP:$SERVER_IP)
           status       warn on a stale manifest / symlinks / armed clients
         config/lanpull.clients     accounts (argon2, mode 600)
         $STATE_DIR/                manifest, cache, TLS material, log
                     |
                     |  HTTPS GET (Range, HTTP Basic), LAN only
                     v
         Client (Astra Linux, no sudo, Wi-Fi)
         ------------------------------------
         <client-folder>/lanpull.conf   SERVER_URL, OUTPUT
         <client-folder>/auth            user:password (mode 600)
         <client-folder>/server.crt      pinned public certificate
         <client-folder>/pull.py         standard-library pull client
         <client-folder>/state.json      paths lanpull delivered
```

The server exposes exactly four routes under the reserved `_lanpull/` prefix:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET`/`HEAD` | `/_lanpull/manifest.json` | Data manifest (small, always fetched in full). |
| `GET`/`HEAD` | `/_lanpull/file/<path>` | Raw file bytes, with `Range` support. |
| `GET`/`HEAD` | `/_lanpull/client/manifest.json` | Client bundle manifest (`version`, sizes, hashes). |
| `GET`/`HEAD` | `/_lanpull/client/<file>` | Client bundle bytes (`pull.py`, `VERSION`). |

Directory listing is disabled and there is no browser UI.

## Requirements

**Server**

- Linux with administrative access, a Rust toolchain, and `systemd`.
- A stable address: a static IP or a DHCP reservation. The address is embedded
  in the certificate, so it must not change while a certificate is in use.

**Clients**

- A stock `python3` (3.11 or newer, standard library only). No `sudo`, no
  `apt`, no `git`, and no internet access are required.

## Installation

### Server

```bash
git clone <repo> lanpull
cd lanpull
cp config/lanpull.conf.example config/lanpull.conf
$EDITOR config/lanpull.conf

make deps        # Rust target and the quality-gate tooling
make build       # static binary (x86_64-unknown-linux-musl)
make install     # install the binary, the unit, and the client bundle
make cert        # self-signed certificate (SAN = IP:<server-ip>)
make rescan      # generate the manifest from <share-dir>
make up          # start the service (no autostart on boot)
```

The order matters: the certificate must exist before clients are registered
(it is included in the staged client folder), and a manifest must exist before
the first pull. The server refuses to start without accounts and a certificate.

### Register a client (once per machine)

```bash
make add-client NAME=<client-name> [IP=<client-ip>] OUTPUT=<output-dir>
```

This generates a random password, stores only its argon2 hash in
`config/lanpull.clients`, and stages a ready-to-copy folder (`pull.py`,
`lanpull.conf`, `auth`, `server.crt`). Copy that folder to the client machine
into a single directory of your choice — for example `~/lanpull/`. Omitting
`IP` leaves the account usable from any address on the LAN.

### Client

Copy the staged folder to the machine once, then drive it from the terminal:

```bash
./pull.py --check        # report whether updates exist; change nothing
./pull.py                # download/resume changed files, then handle stale files
./pull.py --dry-run      # show the plan; change nothing
./pull.py --self-update  # refresh pull.py from the server (asks to confirm)
```

## Client usage

| Mode | Behavior | Exit code |
| --- | --- | --- |
| `pull.py` | Download changed files, verify, replace atomically; prompt once to delete tracked stale files. | `0` clean, `1` per-file errors, `2` fatal |
| `pull.py --check` | Compare sizes and mtimes only; download nothing. Prints the manifest `generated_at`. | `0` up to date, `1` updates available |
| `pull.py --dry-run` | Print `NEED`/`VERIFY`/`STALE` actions; change nothing. | `0` |
| `pull.py --delete` | Like a normal pull, but delete tracked stale files without prompting. | as `pull.py` |
| `pull.py --self-update` | Compare versions and replace `pull.py` from the served bundle. | `0` current/declined/updated, `2` fatal |
| `pull.py --version` | Print the client version. | `0` |

A path argument overrides `OUTPUT`:

```bash
./pull.py /path/to/mirror
```

`--check`, `--dry-run`, `--delete`, and `--self-update` may be combined with a
path where it makes sense. Deletion is limited to files lanpull itself
delivered earlier, recorded in `state.json`; files you created in the mirror
folder are never touched.

## Configuration

### Server — `config/lanpull.conf`

| Key | Meaning | Example |
| --- | --- | --- |
| `SHARE_DIR` | Directory whose contents are distributed. | `/srv/lanpull/share` |
| `STATE_DIR` | Manifest, cache, TLS material, arm state, audit log (outside the share). | `/var/lib/lanpull` |
| `BIND` | Listen address. | `0.0.0.0` |
| `PORT` | Listen port. | `8000` |
| `SERVER_IP` | Address clients use; embedded in the certificate SAN. | `<server-ip>` |
| `CERT_PATH` | PEM certificate path. | `/var/lib/lanpull/server.crt` |
| `KEY_PATH` | PEM private key path (mode 600). | `/var/lib/lanpull/server.key` |
| `CLIENTS_PATH` | Account file. Relative paths resolve against the config file. | `lanpull.clients` |
| `AUDIT_LOG` | JSON-lines audit log. | `/var/lib/lanpull/access.log` |

The real file is mode 600 and is never committed. Blank lines and `#` comments
are ignored, values may be quoted, and `$NAME`/`${NAME}` environment
references are expanded.

### Accounts — `config/lanpull.clients`

One line per machine:

```text
<client-name>:$argon2$...[:<client-ip>]
```

The optional third field binds the account to a source IP; omit it (or use
`*`) to accept any address on the LAN. Managed with `make add-client`,
`make remove-client`, and `make passwd`; never committed.

### Client — `<client-folder>/lanpull.conf`

```text
SERVER_URL=https://<server-ip>:8000
OUTPUT=<output-dir>
```

`SERVER_URL` must use the IP embedded in `server.crt` (`SAN = IP:<server-ip>`).

## Security model

Access to the share is controlled by three per-request checks:

1. a per-client account password (argon2, verified in constant time);
2. if the account has a registered IP, the request must come from it;
3. the account must be armed by the operator for the current window.

For an IP-bound account these checks stop a copied client folder from being
used on another machine to bulk-download the share. For an account without an
IP binding, the `arm` window is the only barrier. Every request is logged, and
`lanpull report` shows who pulled what and any rejected attempts.

**What this does not do.** It does not protect data already mirrored on a
compromised client: anyone with access to that machine can copy `OUTPUT`.
Credentials live in files and are never copy-proof, and a determined attacker
on the same subnet could steal a registered IP — or use an unbound account —
during the short `arm` window. The client hostname (`X-Lanpull-Host`) is
self-reported and grants nothing; the certificate is public and grants nothing.
Protecting the mirrored data itself is an OS concern: full-disk encryption,
screen locking, and separate accounts. Access should also be limited to the LAN
by the host firewall, since `serve` binds to `0.0.0.0` by default.

## Repository layout

```text
client/     # Python client, config examples, and Python tooling
server/     # Rust crate and Rust tooling
config/     # server configuration examples
systemd/    # service unit
Makefile    # thin forwarder to server/ and client/
```

`client/` and `server/` can be checked out independently with
`git sparse-checkout set client` or `set server`.

## Development

Each side carries its own Makefile and its own static-analysis configuration;
the root `Makefile` forwards to both.

```bash
make -C server ci    # rustfmt, Clippy, rustdoc, tests, deny, audit, audit bin, geiger
make -C client ci    # ruff check/format, mypy --strict, pytest, pip-audit
make -C server ci && make -C client ci
```

The client tooling lives in a uv-managed virtual environment:

```bash
make -C client setup   # create client/.venv and install pinned tools
```

Quality and security rules are machine-checkable and live in the configuration
files (`server/rustfmt.toml`, `server/clippy.toml`, `server/deny.toml`,
`server/.cargo/audit.toml`, `[lints.*]` in `server/Cargo.toml`, and
`client/pyproject.toml`), not in prose. There is intentionally **no CI workflow
and no git hook**; the gates are run deliberately with `make ci`.

## Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| `ERROR: account <name> is not armed (401)` | Run `lanpull arm <name> --ttl 15m` on the server, then retry. |
| `ERROR: account <name> is not allowed from this address (401)` | The account is IP-bound and the machine is not at its registered address. Update `allowed_ip` or re-add the account without `IP`. |
| `ERROR: server certificate does not match server.crt` | Copy the current `server.crt` to the client. The client never falls back to an unverified connection. |
| `ERROR: server has no manifest; ask the operator to run make rescan` | Run `make rescan` on the server. |
| Stale files are not removed | Deletion is limited to files lanpull delivered earlier; answer the prompt or use `--delete`. |
| `ERROR: another pull is already running` | A second pull for the same destination was refused by the lock file. |
| `make status` warns the manifest is stale | Files in the share are newer than the manifest; run `make rescan`. |

## Contributing

Issues and pull requests are welcome. Before opening a pull request:

1. run `make -C server ci` and `make -C client ci` and make sure both pass;
2. keep the client to the Python standard library;
3. keep all code, comments, and documentation in English;
4. never commit secrets, certificates, or account files.

## License

[MIT](LICENSE) © 2026 Star-Barsuk
