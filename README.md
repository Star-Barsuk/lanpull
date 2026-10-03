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

## Table of contents

- [Why lanpull](#why-lanpull)
- [Features](#features)
- [How it works](#how-it-works)
- [Requirements](#requirements)
- [Installation](#installation)
- [Teardown](#teardown)
- [Client usage](#client-usage)
- [Configuration](#configuration)
- [Security model](#security-model)
- [Repository layout](#repository-layout)
- [Development](#development)
- [Troubleshooting](#troubleshooting)
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
         $SHARE_<name>/             one dir per share (any path)
         lanpull (single Rust binary)
           serve        HTTPS on $BIND:$PORT, auth + arm + access mapping, Range
           manifest     regenerate per-share and per-account manifests
           add-client   create an account + stage a ready client folder
           access       manage the JSON policy (public set + per-client deltas)
           arm/disarm   authorize a client for a short window
           report       who pulled what (from the audit log)
           cert         self-signed certificate (SAN = IPs + localhost)
           status       warn on stale manifests / symlinks / policy
         config/lanpull.clients     accounts (argon2, mode 600)
         config/lanpull.access.json access policy (mode 600)
         $STATE_DIR/                manifests, caches, TLS material, log
                     |
                     |  HTTPS GET (Range, HTTP Basic), LAN only
                     v
         Client (Astra Linux, no sudo, Wi-Fi)
         ------------------------------------
         <client-folder>/lanpull.conf   SERVER_URL, MIRROR_<share> per share
         <client-folder>/auth            user:password (mode 600)
         <client-folder>/server.crt      pinned public certificate
         <client-folder>/pull.py         standard-library pull client
         <client-folder>/state.json      delivered paths, keyed by share
```

The server exposes share-scoped routes plus the global client bundle, under the
reserved `_lanpull/` prefix:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET`/`HEAD` | `/_lanpull/share/<share>/manifest.json` | That account's filtered manifest for the share. |
| `GET`/`HEAD` | `/_lanpull/share/<share>/file/<path>` | Raw file bytes, with `Range` support. |
| `GET`/`HEAD` | `/_lanpull/client/manifest.json` | Client bundle manifest (`version`, sizes, hashes). |
| `GET`/`HEAD` | `/_lanpull/client/<file>` | Client bundle bytes (`pull.py`, `VERSION`). |

Directory listing is disabled and there is no browser UI. An account sees only
the shares and paths it is granted; anything else is `403`.

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
make config      # generate config/lanpull.conf for this machine (prompts)
# or: cp config/lanpull.conf.example config/lanpull.conf && $EDITOR config/lanpull.conf

make setup       # Rust target and the quality-gate tooling
make build       # static binary (x86_64-unknown-linux-musl)
make install     # install the binary, the unit, and the client bundle (escalates as needed)
make cert        # self-signed certificate (SAN = IP:<server-ip>)
make rescan      # generate the manifest from <share-dir>
make up          # start the service (no autostart on boot)
```

`make config` derives `SERVER_IP` from the main routing table, so it ignores
proxy/tunnel addresses and never names an interface, then writes the file mode
600 and creates the directories. It needs `sudo` to create `STATE_DIR` outside
your home.

The order matters: the certificate must exist before clients are registered
(it is included in the staged client folder), and a manifest must exist before
the first pull. The server refuses to start without accounts and a certificate.

### Register a client (once per machine)

Grant access in the policy, then create the account:

```bash
# a file visible to every account, and a file only for one client
lanpull access public add <share>:<path>
lanpull access client add <client-name> <share>:<path>

# create the account and stage a ready-to-copy folder
lanpull add-client <client-name> [--ip <client-ip>] --output <output-dir>
```

`add-client` generates a random password, stores only its argon2 hash in
`config/lanpull.clients`, and stages a folder (`pull.py`, `lanpull.conf`,
`auth`, `server.crt`) with one `MIRROR_<share>` line per share the account can
see. Copy that folder to the client machine into a single directory of your
choice — for example `~/lanpull/`. Omitting `--ip` leaves the account usable
from any address on the LAN. Changing the scope later is a `lanpull access`
command; the operator no longer edits rules per client.

The `make` wrapper forwards an argument string:

```bash
make access ARGS='public add <share>:<path>'
make access ARGS='client add <client-name> <share>:<path>'
make access ARGS='client list'
```

### Client

Copy the staged folder to the machine once, then drive it from the terminal:

```bash
./pull.py --check        # report whether updates exist; change nothing
./pull.py                # download/resume changed files, then handle stale files
./pull.py --dry-run      # show the plan; change nothing
./pull.py --self-update  # refresh pull.py from the server (asks to confirm)
```

## Teardown

The removal levels are cumulative, and the destructive ones require `CONFIRM=1`:

```bash
make clean                      # build artifacts and caches only (no root)
make distclean CONFIRM=1        # clean + config/lanpull.conf + config/lanpull.clients
make uninstall CONFIRM=1        # binary, systemd unit, and $STATE_DIR
make uninstall CONFIRM=1 SHARE=1   # also removes every share directory
make wipe CONFIRM=1             # distclean + uninstall: everything lanpull created
make wipe CONFIRM=1 SHARE=1     # ...including the distributed files
```

Each target escalates only the steps that need root, so no `sudo` prefix is
required on the command line; run them without `sudo`.

`uninstall` removes `$STATE_DIR` (manifest, TLS material, arm state, audit log,
staged bundle); `SHARE=1` additionally removes the distributed files, so use it
only when the share holds nothing you need.

Together these targets erase every artifact lanpull itself created on the
server, at any stage after the service has been stopped. The gate tools that
`make setup` installs, the systemd journal, and client folders on remote
machines are not lanpull artifacts, so they are left in place.

## Client usage

| Mode | Behavior | Exit code |
| --- | --- | --- |
| `pull.py` | Download changed files, verify, replace atomically; prompt once to delete tracked stale files. | `0` clean, `1` per-file errors, `2` fatal |
| `pull.py --check` | Compare sizes and mtimes only; download nothing. Prints the manifest `generated_at`. | `0` up to date, `1` updates available |
| `pull.py --dry-run` | Print `NEED`/`VERIFY`/`STALE` actions; change nothing. | `0` |
| `pull.py --delete` | Like a normal pull, but delete tracked stale files without prompting. | as `pull.py` |
| `pull.py --self-update` | Compare versions and replace `pull.py` from the served bundle. | `0` current/declined/updated, `2` fatal |
| `pull.py --version` | Print the client version. | `0` |

A positional argument limits the run to one configured share:

```bash
./pull.py reports
```

`--check`, `--dry-run`, `--delete`, and `--self-update` may be combined with a
share where it makes sense. Deletion is limited to files lanpull itself
delivered earlier, recorded in `state.json`; files you created in the mirror
folder are never touched.

## Configuration

### Server — `config/lanpull.conf`

| Key | Meaning | Example |
| --- | --- | --- |
| `SHARE_<name>` | A named share (one key per share; the name matches `^[a-z0-9][a-z0-9_-]*$`). | `SHARE_reports=/srv/lanpull/reports` |
| `STATE_DIR` | Manifests, caches, TLS material, arm state, audit log (outside the shares). | `/var/lib/lanpull` |
| `BIND` | Listen address. | `0.0.0.0` |
| `PORT` | Listen port. | `8000` |
| `SERVER_IP` | Address clients use; embedded in the certificate SAN. | `<server-ip>` |
| `CERT_PATH` | PEM certificate path. | `/var/lib/lanpull/server.crt` |
| `KEY_PATH` | PEM private key path (mode 600). | `/var/lib/lanpull/server.key` |
| `CLIENTS_PATH` | Account file. Relative paths resolve against the config file. | `lanpull.clients` |
| `ACCESS_PATH` | JSON access policy. Relative paths resolve against the config file. | `lanpull.access.json` |
| `AUDIT_LOG` | JSON-lines audit log. | `/var/lib/lanpull/access.log` |

The real file is mode 600 and is never committed. Blank lines and `#` comments
are ignored, values may be quoted, and `$NAME`/`${NAME}` environment
references are expanded. At least one `SHARE_<name>` key is required.

### Accounts — `config/lanpull.clients`

One line per machine:

```text
<client-name>:$argon2$...[:<client-ip>][:local]
```

The optional third field binds the account to a source IP; omit it (or use
`*`) to accept any address on the LAN. The `local` field marks a self-share
account, exempt from the arming window. Managed with `make add-client`,
`make remove-client`, and `make passwd`; never committed.

Pick `<client-name>` as a meaningful label for the machine or transfer
direction (for example `pc-to-laptop`); it is the account identity and the key
of the access policy. The client hostname (`X-Lanpull-Host`) is self-reported,
shown by `lanpull report`, and never grants access.

### Access policy — `config/lanpull.access.json`

One JSON object; never committed and mode 600:

```json
{
  "version": 1,
  "shares": {
    "reports": {
      "public": ["quarterly.pdf"],
      "clients": {
        "laptop": { "add": ["draft.pdf"], "remove": ["quarterly.pdf"] }
      }
    }
  }
}
```

- `public` is visible to **every** account of that share; a newly created
  account inherits it automatically.
- `clients.<name>.add` adds paths for that account only; `remove` subtracts
  paths (including public ones) for that account only.
- The effective set is `(public − remove) ∪ add`, sorted by path. Default is
  deny: a path named nowhere is invisible, and an account sees only what its
  policy allows.
- A `<path>` is share-relative and uses the glob grammar (`*` within a segment,
  `**` across segments), so a literal file name grants exactly that file and
  `**` grants the whole share.

Manage it with the `lanpull access` commands; the server reads the policy per
request, so changes take effect without a restart.

```bash
lanpull access public add reports:quarterly.pdf     # add a file for everyone
lanpull access public remove reports:quarterly.pdf  # remove it for everyone
lanpull access public list                          # show the public set
lanpull access client add laptop reports:draft.pdf  # personal file
lanpull access client remove laptop reports:quarterly.pdf   # hide one file from one client
lanpull access client list [<client-name>]          # effective set per client
lanpull access import --from <old-lanpull.access>   # one-time migration from the flat file
lanpull access doctor                               # unknown shares, missing files, empty accounts
lanpull access apply                                # regenerate manifests after a manual edit
```

The old flat `config/lanpull.access` format (`<client> <share>[:<glob>]`) and
the `grant`/`revoke`/`--share` commands are gone; `access import` reads the old
file once and writes the JSON policy.

### Client — `<client-folder>/lanpull.conf`

```text
SERVER_URL=https://<server-ip>:8000
MIRROR_reports=<output-dir>
MIRROR_media=<other-output-dir>
```

`SERVER_URL` must use the IP embedded in `server.crt` (the SAN includes
`<server-ip>`, `127.0.0.1`, and `localhost`).

Each `MIRROR_<share>` maps a share to a local mirror directory; the operator
chooses the base with `make add-client … OUTPUT=<output-dir>`, and the staged
file maps each granted share to a subdirectory. A positional argument selects a
single share:

```bash
./pull.py reports
```

There is no default: if there is no `MIRROR_<share>` entry, `pull.py` fails.
This is separate from the server's share directories, which are the sources the
manifests are built from and are never written by a pull.

## Worked example

Two clients, one file everyone gets, one file only for `client-01`. The share
`cube` is `<share-dir>`, containing `sub/common.pdf` (common) and
`sub/target.pdf` (target).

```bash
# --- one-time server setup ---
make build && make install
# config/lanpull.conf: SHARE_cube=<share-dir>, ACCESS_PATH=lanpull.access.json
make cert

# --- access policy ---
lanpull access public add cube:sub/common.pdf
lanpull access client add client-01 cube:sub/target.pdf
lanpull access client list                 # client-01 sees both; others see common only
lanpull access doctor

# --- accounts + staged folders ---
lanpull add-client client-01 --ip <client-ip> --output <output-dir>
lanpull add-client client-02 --output <output-dir>

# --- manifests + serve ---
lanpull access apply                       # or: make rescan
make up

# --- round: arm on the server, pull on each client ---
lanpull arm --all --ttl 15m
# on each client, from ~/lanpull/
./pull.py --check
./pull.py                                  # client-02 never sees sub/target.pdf
lanpull report --since 1h                  # on the server

# --- later changes ---
lanpull access public add cube:sub/new.pdf         # new file for everyone
lanpull access client remove client-01 cube:sub/target.pdf   # hide it from one client
lanpull access client add client-02 cube:sub/only-02.pdf     # personal file
lanpull access public remove cube:sub/common.pdf   # withdraw a file from everyone
```

Every `lanpull access` mutation saves the JSON policy (mode 600) and regenerates
the per-account manifests immediately, so the next pull sees the change.

## Security model

Access is controlled by four per-request checks:

1. a per-client account password (argon2, verified in constant time);
2. if the account has a registered IP, the request must come from it;
3. the account must be armed by the operator for the current window (a `local`
   self-share account is exempt);
4. the requested share and path must be allowed by the account's access rules.

For an IP-bound account these checks stop a copied client folder from being
used on another machine to bulk-download a share. For an account without an
IP binding, the `arm` window is the only barrier on the network side, while the
access mapping limits what any account can see at all (a `403` otherwise).
Every request is logged, and `lanpull report` shows who pulled what and any
rejected attempts.

**What this does not do.** It does not protect data already mirrored on a
compromised client: anyone with access to that machine can copy the mirror
directories.
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
Makefile    # thin pattern forwarder to server/ and client/
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

The gates locate their tools themselves: the server Makefile finds the Rust
toolchain in `$CARGO_HOME/bin` (no `PATH` edit needed), and the client gates
create their uv-managed `.venv` on first run. `make setup` installs the Rust
target and the gate tools.

```bash
make setup            # install the Rust target and the gate tools
make -C client setup  # create client/.venv and install pinned tools
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

## License

[MIT](LICENSE) © 2026 Star-Barsuk
