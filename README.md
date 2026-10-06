# lanpull

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](server/Cargo.toml)
[![Python](https://img.shields.io/badge/python-3.11%2B-blue.svg)](client/pyproject.toml)
[![Platform](https://img.shields.io/badge/platform-Linux-lightgrey.svg)](#requirements)

A minimal, manual file-distribution tool for a local network.

The server publishes one or more shared directories over HTTPS; clients pull the
latest version on demand, replacing changed files and removing files that no
longer exist on the server. There is no automatic synchronization, no version
history, and no rollback.

## Table of contents

- [Why lanpull](#why-lanpull)
- [Features](#features)
- [How it works](#how-it-works)
- [Requirements](#requirements)
- [Installation](#installation)
- [Full pipeline](#full-pipeline)
- [Server command reference](#server-command-reference)
- [Client command reference](#client-command-reference)
- [Configuration](#configuration)
- [Worked example](#worked-example)
- [Teardown](#teardown)
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

`lanpull` takes the opposite approach. The owner edits the files on one machine
(the server), and the operator walks to each client and runs one explicit
command that fetches only what changed, resumably, over a pinned TLS channel.
Nothing runs in the background, nothing is stored twice, and every pull is an
intentional act.

It is not a replacement for Git, Syncthing, or Nextcloud. It is a deliberately
small pull-only mirror for a trusted local network.

## Features

- **Pull-only and explicit** — no daemon, no watcher, no auto-sync.
- **Resumable transfers** — a dropped connection at 350 MB resumes from where it
  stopped, guarded by `If-Range`.
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
- **Selective shares** — per-share, per-account access with a `public` set and
  per-client additions/removals; default deny.
- **Sensible ignores** — a built-in set skips VCS, build, cache, and editor
  artifacts (`target/`, `__pycache__/`, `.venv/`, `node_modules/`, `.git/`, …),
  with an optional per-share `.lanpullignore` for overrides.
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
           serve            HTTPS on $BIND:$PORT, auth + arm + access, Range
           share rescan     regenerate per-share and per-account manifests
           account add      create an account + stage a ready client folder
           access           manage the JSON policy (public set + per-client deltas)
           account arm      authorize a client for a short window
           report           who pulled what (from the audit log)
           cert             self-signed certificate (SAN = IPs + localhost)
           status           warn on stale manifests / symlinks / policy
         /etc/lanpull/lanpull.conf      configuration (mode 640, root:group)
         /etc/lanpull/lanpull.clients   accounts (argon2, mode 600)
         /etc/lanpull/lanpull.access.json access policy (mode 600)
         $STATE_DIR/                manifests, caches, TLS material, log
                     |
                     |  HTTPS GET (Range, HTTP Basic), LAN only
                     v
         Client (no sudo, Wi-Fi)
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

- A stock `python3` (3.11 or newer, standard library only). No `sudo`, no `apt`,
  no `git`, and no internet access are required.

## Installation

### Server

```bash
git clone <repo> lanpull
cd lanpull
make setup       # Rust target and the quality-gate tooling
make build       # static binary (x86_64-unknown-linux-musl)
make install     # binary + unit + /etc/lanpull + client bundle (escalates as needed)
```

`make` only works with the source tree: it builds and installs, nothing else.
After `make install`, the operator drives the server with the installed
`lanpull` binary, and the configuration is created by the binary itself:

```bash
sudo lanpull init            # create /etc/lanpull/lanpull.conf
```

`lanpull init` derives `SERVER_IP` from the main routing table, so it ignores
proxy/tunnel addresses and never names an interface. Because the target is the
canonical `/etc/lanpull/lanpull.conf`, it must run as root; it sets
`/etc/lanpull` to `root:<operator-group>` mode `0770` and the file to
`root:<operator-group>` mode `0640`, so the service (running as the operator)
can read it and the operator can manage accounts and policy without `sudo`. All
other commands run as the installing user, not with `sudo`. The canonical
layout is:

```text
/usr/local/bin/lanpull                  the binary
/etc/lanpull/                           root:<operator-group>, mode 0770
  lanpull.conf                          configuration (root:group, 0640)
  lanpull.clients                       accounts (operator-owned, 0600)
  lanpull.access.json                   access policy (operator-owned, 0600)
/var/lib/lanpull/                       state, owned by the operator
/etc/systemd/system/lanpull.service     unit (reads /etc/lanpull/lanpull.conf)
```

### Client folder

The client is a single self-contained directory. Create it once on the client
machine with `lanpull account add` on the server (it stages a ready folder) and
copy it over with `lanpull account export` (or `scp` the staged folder). The
folder holds `pull.py`, `lanpull.conf`, `auth`, and `server.crt`; see
[Register a client](#register-a-client).

### Command-line conventions

Every server command follows the same conventions:

- result data goes to **stdout**, diagnostics, warnings, and prompts to
  **stderr**;
- `--json` prints one envelope on stdout instead of text:
  `{"status":"ok","command":"<path>","data":…,"warnings":[…]}` on success,
  `{"status":"error","command":…,"code":…,"message":…,"hint":…}` on failure;
- the configuration path resolves from `--config`, then `$LANPULL_CONFIG`, then
  the canonical `/etc/lanpull/lanpull.conf`, so no flag is needed in normal use;
- `--yes` skips a confirmation, `--force` overwrites an existing target, and
  `--dry-run` reports what a mutation would change without writing it.

Exit codes: `0` success, `1` operational failure, `2` usage error, `3` not
initialized, `4` denied, `5` busy. A declined confirmation exits `1`.

Run `lanpull --help` or `lanpull <command> --help` for the full tree.

## Full pipeline

The end-to-end lifecycle on the server, in order. Steps that need root are
marked; everything else runs as the operator.

```bash
# 1. Build and install the server (source tree).
make setup && make build && make install

# 2. Create the canonical configuration (root; derives SERVER_IP).
sudo lanpull init --share default=<share-dir>

# 3. Generate the TLS certificate (SAN = SERVER_IP, 127.0.0.1, localhost).
lanpull cert

# 4. Declare who may see what (the policy drives the manifests).
lanpull access public add <share>:**              # everyone sees the whole share
lanpull access client add <client-name> <share>:<path>   # or one client, one path
lanpull access doctor

# 5. Create accounts; each one stages a ready-to-copy client folder.
lanpull account add <client-name> --ip <client-ip> --output <output-dir>
lanpull account export <client-name> --to <dir>   # copy this folder to the client

# 6. Build the manifests and start serving.
lanpull share rescan                              # or: lanpull access apply
lanpull service start

# 7. Open the arm window, then pull on each client.
lanpull account arm --all --ttl 15m
# on the client, from its folder:
./pull.py --check
./pull.py

# 8. Inspect, then close the window.
lanpull report --since 1h
lanpull account disarm <client-name>

# 9. Later rounds: edit the share, rescan, re-arm, pull again.
lanpull share rescan && lanpull account arm --all --ttl 15m
```

Notes:

- The **certificate** must exist before an account is created (it is copied into
  the staged folder); **access rules** must exist before an account is created
  (an account with no rules is rejected); and a **manifest** must exist before
  the first pull.
- The server refuses to start (`serve`) without accounts and a certificate.
- `share rescan` rebuilds the per-share and per-account manifests. Any command
  that changes accounts or the policy regenerates them automatically, so an
  explicit `rescan` is only needed after you edit files in a share by hand.
- The arm window is the network-side barrier: without it an account's requests
  are rejected with `401`. `local` (self-share) accounts are exempt.

## Server command reference

### Global options

| Option | Meaning |
| --- | --- |
| `--config <path>` | Configuration file. Overrides `$LANPULL_CONFIG` and the canonical `/etc/lanpull/lanpull.conf`. |
| `--json` | Print one machine-readable envelope on stdout instead of text. |
| `-v`, `--verbose` | Increase verbosity (repeatable). Overrides `$RUST_LOG`. |
| `-q`, `--quiet` | Suppress everything but errors. Overrides `$RUST_LOG`. |
| `-h`, `--help` | Print help. |
| `-V`, `--version` | Print the version. |

Global options are accepted before or after the subcommand.

### `lanpull init`

Create the configuration and state for this machine.

| Argument / option | Meaning |
| --- | --- |
| `--share <name>=<path>` | A share to declare; repeatable. Defaults to a single `default` share at `$HOME/lanpull-share`. |
| `--state-dir <dir>` | Internal state directory (default `/var/lib/lanpull`). |
| `--bind <addr>` | Listen address (default `0.0.0.0`). |
| `--port <port>` | Listen port (default `8000`). |
| `--server-ip <ip>` | Address clients use; embedded in the certificate. Derived from the routing table when omitted. |
| `--force` | Overwrite an existing configuration file. |

`init` creates the configuration directory, the share directories, the state
directory (when permitted), and a commented `.lanpullignore` template in each
share. Against the canonical path it must run as root and applies the
`root:<operator-group>` ownership described above. A malformed `--share` or an
invalid share name is a usage error.

### `lanpull serve`

Run the HTTPS server in the foreground. Production uses the systemd unit, which
runs `lanpull serve --config /etc/lanpull/lanpull.conf`. `serve` does not use
the JSON envelope; it logs to stderr and stops on `SIGINT`/`SIGTERM`.

### `lanpull status`

Warn on a stale manifest, symlinks, reserved-prefix entries, and policy
problems, and show the armed accounts. Exits `0`; the warnings are on stderr
(with `--json`, in the envelope's `warnings`). "Clean" prints
`status is clean`.

### `lanpull cert`

Generate the self-signed certificate and key (`CERT_PATH`, `KEY_PATH`; the key
is mode `600`). The SAN carries `SERVER_IP`, `127.0.0.1`, and `localhost`.

| Option | Meaning |
| --- | --- |
| `--force` | Regenerate an existing certificate and key. |

Without `--force`, an existing certificate is a usage error.

### `lanpull share`

| Command | Arguments | Options | Purpose |
| --- | --- | --- | --- |
| `share list` | — | — | List the declared shares and their directories. |
| `share rescan` | — | — | Regenerate the per-share and per-account manifests. |
| `share add` | `<name> <dir>` | `--dry-run` | Append a `SHARE_<name>` line and create the `.lanpullignore` template. |

A share is never removed by a command: delete its `SHARE_<name>` line by hand,
or repoint it with `config set`. Against the canonical configuration, `share
add` needs root.

### `lanpull access`

The JSON policy maps each share to a `public` set plus per-client `add` and
`remove` deltas. The effective set for an account is `(public − remove) ∪ add`,
sorted by path; the default is deny. A `<path>` is share-relative glob:
`*` matches within a segment, `**` across segments.

| Command | Arguments | Options | Purpose |
| --- | --- | --- | --- |
| `access public add` | `<share>:<path>...` | `--dry-run` | Add paths visible to every account. |
| `access public remove` | `<share>:<path>...` | `--yes`, `--dry-run` | Remove paths from the public set. |
| `access public list` | — | `--share <share>` | Show the public set. |
| `access client add` | `<name> <share>:<path>...` | `--dry-run` | Add paths for one account. |
| `access client remove` | `<name> <share>:<path>...` | `--yes`, `--dry-run` | Remove paths (including public ones) for one account. |
| `access client list` | `[<name>]` | — | Show the effective set per account (or one). |
| `access import` | — | `--from <file>`, `--dry-run` | One-time migration from the old flat `lanpull.access` file. |
| `access doctor` | — | — | Flag unknown shares, missing files, empty accounts, and stale removals. |
| `access apply` | — | — | Regenerate the manifests after a manual policy edit. |

Adding a rule for an account that does not exist yet is allowed; it takes effect
when the account is created. A rule that names an unknown share is rejected.

### `lanpull account`

| Command | Arguments | Options | Purpose |
| --- | --- | --- | --- |
| `account add` | `<name>` | `--output <dir>`, `--ip <ip>`, `--local`, `--dry-run` | Create an account and stage a client folder. |
| `account remove` | `<name>` | `--yes`, `--dry-run` | Revoke an account and drop its policy deltas. |
| `account list` | — | — | List accounts with their IP, scope, and effective rules. |
| `account passwd` | `<name>` | `--output <dir>` | Rotate the password and refresh the staged `auth` file. |
| `account export` | `<name>` | `--to <dir>`, `--move`, `--force`, `--dry-run` | Copy the staged folder for transfer to the client. |
| `account arm` | `[<name>]` | `--all`, `--ttl <duration>` (default `15m`) | Authorize one or all accounts for a window. |
| `account disarm` | `<name>` | — | Clear an account's arm window. |

`account add` requires, in order: a staged client bundle (`make install`), at
least one access rule for the account, and a certificate. It generates a random
password, stores only its argon2 hash in `lanpull.clients`, and stages a folder
(`pull.py`, `lanpull.conf`, `auth`, `server.crt`) under
`$STATE_DIR/client-ready/<name>` with one `MIRROR_<share>` line per share the
account can see. `--local` marks a self-share account (exempt from arming);
`--ip` binds the account to a source address.

`account export` copies the folder to `<dir>` (or into `<dir>/<name>` when
`<dir>` already exists). `--force` overwrites, `--move` moves the staging copy,
and exporting to another filesystem prints a warning that `auth` and
`server.crt` now live on that device. Omitting `--ip` leaves the account usable
from any LAN address.

### `lanpull config`

| Command | Arguments | Options | Purpose |
| --- | --- | --- | --- |
| `config show` | — | — | Print the resolved configuration and its paths. |
| `config get` | `<key>` | — | Print one value. |
| `config set` | `<key> <value>` | `--dry-run` | Set one value. |
| `config path` | — | — | Print the resolved configuration file path. |

`config set` accepts `STATE_DIR`, `BIND`, `PORT`, `SERVER_IP`, `CERT_PATH`,
`KEY_PATH`, `CLIENTS_PATH`, `ACCESS_PATH`, and `AUDIT_LOG`; it refuses
`SHARE_<name>` (use `share add`) and an empty value. Against the canonical
configuration it needs root.

### `lanpull service`

Thin wrappers over `systemctl`. `start`/`stop`/`restart` report one line;
`status` and `logs` pass the `systemctl` output through (an inactive unit is a
normal result, not an error). The unit does not autostart on boot and has no
`[Install]` section, so the operator starts it explicitly for a round.

### `lanpull audit`

Print every artifact lanpull created on this host (configuration, accounts,
policy, certificate, key, state, manifests, arm state, audit log, bundle, and
each share directory) with `present`/`missing`.

### `lanpull report`

Summarize the audit log: per account, requests/bytes/rejections.

| Option | Meaning |
| --- | --- |
| `--user <name>` | Restrict to one account (`--account` is a hidden alias). |
| `--since <duration>` | Only requests newer than the duration, for example `7d` or `1h`. |

An empty or missing log prints `no requests recorded`.

### `lanpull clean`

Remove the server's runtime leftovers: orphaned `client-ready/<name>` folders
for accounts that no longer exist. It does not touch shares, the state
directory itself, or installed artifacts.

| Option | Meaning |
| --- | --- |
| `--dry-run` | List what would be removed without removing it. |
| `--yes` | Do not ask for confirmation. |

Without `--yes` and without a terminal, `clean` is a usage error.

## Client command reference

The client is `<client-folder>/pull.py`. It reads `lanpull.conf`, `auth`, and
`server.crt` from its own directory and writes `state.json` there.

| Option | Meaning |
| --- | --- |
| `[<share>]` | Mirror only this configured share (default: every share). |
| `--check` | Compare sizes and mtimes; download nothing. Prints `generated_at` and the update/unchanged counts. |
| `--dry-run` | Print the `NEED`/`VERIFY`/`STALE` plan; change nothing. |
| `--delete` | Delete tracked stale files without prompting. |
| `--self-update` | Replace `pull.py` from the served bundle when the versions differ. |
| `--clean` | Remove this client's runtime leftovers offline. |
| `--yes` | Skip the `--clean`/`--self-update` confirmation. |
| `--version` | Print the client version. |

| Mode | Behavior | Exit code |
| --- | --- | --- |
| `pull.py` | Download changed files, verify, replace atomically; prompt once to delete tracked stale files. | `0` clean, `1` per-file errors, `2` fatal |
| `pull.py --check` | Compare sizes and mtimes only; download nothing. | `0` up to date, `1` updates available |
| `pull.py --dry-run` | Print `NEED`/`VERIFY`/`STALE` actions; change nothing. | `0` |
| `pull.py --delete` | Like a normal pull, but delete tracked stale files without prompting. | as `pull.py` |
| `pull.py --self-update` | Compare versions and replace `pull.py` from the served bundle. | `0` current/declined/updated, `2` fatal |
| `pull.py --clean` | Remove `state.json`, per-mirror `.lanpull.lock`/`.lanpull.partials.json`, and every `*.part`. Never contacts the server. | `0` removed/declined, `1` per-file errors, `2` fatal |
| `pull.py --version` | Print the client version. | `0` |

`--yes` is only meaningful with `--clean` or `--self-update`; `--clean` cannot be
combined with `--check`, `--delete`, or `--self-update`. Deletion is limited to
files lanpull delivered earlier, recorded in `state.json`; files you created in
the mirror folder are never touched.

## Configuration

### Server — `/etc/lanpull/lanpull.conf`

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

The real file is mode 640 and is never committed. Blank lines and `#` comments
are ignored, values may be quoted, and `$NAME`/`${NAME}` environment references
are expanded. At least one `SHARE_<name>` key is required. Nothing lanpull
generates (`STATE_DIR`, `CERT_PATH`, `KEY_PATH`, `CLIENTS_PATH`, `ACCESS_PATH`,
`AUDIT_LOG`) may live inside a share root.

### Per-share ignore — `<share-dir>/.lanpullignore`

Each share directory may hold a `.lanpullignore` file; `lanpull init` and
`lanpull share add` create a commented template when one is absent, and never
overwrite an existing file. It works like a share-local `.gitignore` that
lanpull itself reads at `lanpull share rescan`:

- `#` starts a comment; blank lines are skipped;
- `!` re-includes a path ignored by an earlier rule (last match wins);
- `*` matches within a path segment, `**` across segments;
- a leading `/` anchors to the share root; a pattern without `/` matches its name
  at any depth; a pattern that names a directory also ignores its contents;
- a leading `\#` or `\!` is an escaped literal; a UTF-8 BOM is ignored.

Character classes (`[abc]`), `?`, and other escapes are not supported. An
invalid pattern is a warning, not a failure, and the `.lanpullignore` file
itself is never distributed.

Lanpull already ignores the VCS, build, cache, and editor/OS artifacts of common
languages — including `.git/`, `target/`, `__pycache__/`, `.venv/`,
`node_modules/`, `dist/`, `build/`, `vendor/`, `*.o`, `*.so`, `*.class`,
`*.swp`, `*~`, `.DS_Store`, and `~$*`. That default set is applied to every
share and can be overridden per path with `!`, for example `!build/keep.txt`.
A file that was already delivered and is then ignored becomes stale on the next
rescan, so the next pull removes it from the mirror like any server-side
deletion.

### Accounts — `/etc/lanpull/lanpull.clients`

One line per machine:

```text
<client-name>:$argon2$...[:<client-ip>][:local]
```

The fields after the hash are positional. The IP field binds the account to a
source IP; omit it (or use `*`) to accept any address on the LAN. The `local`
field marks a self-share account, exempt from the arming window; because the
fields are positional, a local account without an IP is written with an empty IP
field, for example `<client-name>:$argon2$...::local`. Managed with
`lanpull account add`, `account remove`, and `account passwd`; never committed.

Pick `<client-name>` as a meaningful label for the machine or transfer direction
(for example `pc-to-laptop`); it is the account identity and the key of the
access policy. The client hostname (`X-Lanpull-Host`) is self-reported, shown by
`lanpull report`, and never grants access.

### Access policy — `/etc/lanpull/lanpull.access.json`

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
- The effective set is `(public − remove) ∪ add`, sorted by path. The default is
  deny: a path named nowhere is invisible.
- A `<path>` is share-relative and uses the glob grammar described above, so a
  literal file name grants exactly that file and `**` grants the whole share.

Manage it with `lanpull access`; the server reads the policy per request, so
changes take effect without a restart.

### Client — `<client-folder>/lanpull.conf`

```text
SERVER_URL=https://<server-ip>:8000
MIRROR_reports=<output-dir>
MIRROR_media=<other-output-dir>
```

`SERVER_URL` must use the IP embedded in `server.crt` (the SAN includes
`<server-ip>`, `127.0.0.1`, and `localhost`). Each `MIRROR_<share>` maps a share
to a local mirror directory; the operator chooses the base with
`lanpull account add … --output <output-dir>`, and the staged file maps each
granted share to a subdirectory of it. There is no default: without a
`MIRROR_<share>` entry, `pull.py` fails. This is separate from the server's
share directories, which are the sources the manifests are built from and are
never written by a pull.

### Register a client

Grant access in the policy, then create the account:

```bash
# a file visible to every account, and a file only for one client
lanpull access public add <share>:<path>
lanpull access client add <client-name> <share>:<path>

# create the account and stage a ready-to-copy folder
lanpull account add <client-name> [--ip <client-ip>] [--local] --output <output-dir>

# copy the staged folder to the client machine
lanpull account export <client-name> --to <dir>
```

Copy the exported folder to the client into a single directory of your choice —
for example `~/lanpull/`. Changing the scope later is a `lanpull access`
command; the operator never edits rules per client.

## Worked example

Two clients, one file everyone gets, one file only for `client-01`. The share
`cube` is `<share-dir>`, containing `sub/common.pdf` (common) and
`sub/target.pdf` (target).

```bash
# --- one-time server setup ---
make setup && make build && make install
sudo lanpull init --share cube=<share-dir>   # create the canonical configuration
lanpull cert                                 # SAN includes <server-ip> and loopback

# --- access policy ---
lanpull access public add cube:sub/common.pdf
lanpull access client add client-01 cube:sub/target.pdf
lanpull access client list                   # client-01 sees both; others see common only
lanpull access doctor

# --- accounts + staged folders ---
lanpull account add client-01 --ip <client-ip> --output <output-dir>
lanpull account add client-02 --output <output-dir>

# --- manifests + serve ---
lanpull share rescan                         # or: lanpull access apply
lanpull service start

# --- round: arm on the server, pull on each client ---
lanpull account arm --all --ttl 15m
# on each client, from ~/lanpull/
./pull.py --check
./pull.py                                    # client-02 never sees sub/target.pdf
lanpull report --since 1h                    # on the server

# --- later changes ---
lanpull access public add cube:sub/new.pdf         # new file for everyone
lanpull access client remove client-01 cube:sub/target.pdf   # hide it from one client
lanpull access client add client-02 cube:sub/only-02.pdf     # personal file
lanpull access public remove cube:sub/common.pdf   # withdraw a file from everyone
```

Every `lanpull access` mutation saves the JSON policy (mode 600) and regenerates
the per-account manifests immediately, so the next pull sees the change.

## Teardown

The removal levels are cumulative, and the destructive ones require `CONFIRM=1`:

```bash
make clean                      # build artifacts and caches only (no root)
make distclean CONFIRM=1        # clean + repository runtime leftovers
make uninstall CONFIRM=1        # binary, systemd unit, /etc/lanpull, and $STATE_DIR
make wipe CONFIRM=1             # distclean + uninstall: everything lanpull created
```

Each target escalates only the steps that need root, so no `sudo` prefix is
required on the command line; run them without `sudo`.

`distclean` removes only repository-local runtime leftovers (`state.json`,
`*.part`, `.lanpull.lock`, `.lanpull.partials.json`) under the source tree; it
does not touch `/etc/lanpull` or the installed state. `uninstall` removes the
installed artifacts: the binary, the unit, the whole `/etc/lanpull` directory
(configuration, accounts, policy), and `$STATE_DIR` (manifests, TLS material,
arm state, audit log, staged bundle). The share directories are the operator's
data and are never removed by a teardown; delete them by hand once nothing in
them is needed.

Together these targets erase every artifact lanpull itself created on the
server, at any stage after the service has been stopped. The gate tools that
`make setup` installs, the systemd journal, the share directories, and client
folders on remote machines are not lanpull artifacts, so they are left in place;
on each client, `./pull.py --clean` removes that client's own runtime leftovers.

## Security model

Access is controlled by four per-request checks:

1. a per-client account password (argon2, verified in constant time);
2. if the account has a registered IP, the request must come from it;
3. the account must be armed by the operator for the current window (a `local`
   self-share account is exempt);
4. the requested share and path must be allowed by the account's access rules.

For an IP-bound account these checks stop a copied client folder from being used
on another machine to bulk-download a share. For an account without an IP
binding, the `arm` window is the only barrier on the network side, while the
access mapping limits what any account can see at all (a `403` otherwise). Every
request is logged, and `lanpull report` shows who pulled what and any rejected
attempts.

**What this does not do.** It does not protect data already mirrored on a
compromised client: anyone with access to that machine can copy the mirror
directories. Credentials live in files and are never copy-proof, and a
determined attacker on the same subnet could steal a registered IP — or use an
unbound account — during the short `arm` window. The client hostname
(`X-Lanpull-Host`) is self-reported and grants nothing; the certificate is
public and grants nothing. Protecting the mirrored data itself is an OS concern:
full-disk encryption, screen locking, and separate accounts. Access should also
be limited to the LAN by the host firewall, since `serve` binds to `0.0.0.0` by
default.

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
make -C server ci    # shell scripts, rustfmt, Clippy, rustdoc, tests, CLI test, deny, audit, audit bin, geiger
make -C client ci    # ruff check/format, mypy --strict, pytest, pip-audit
make -C server e2e   # loopback end-to-end round of the real server and pull.py
make -C server cli-test  # operator-style CLI checks (binary only)
make ci              # both sides' quality gates
```

`make -C server check-scripts` always parses the shell scripts and also runs
`shellcheck` when it is installed.

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
| `ERROR: account <name> is not armed (401)` | Run `lanpull account arm <name> --ttl 15m` on the server, then retry. |
| `ERROR: account <name> is not allowed from this address (401)` | The account is IP-bound and the machine is not at its registered address. Re-add it without `--ip`, or use the registered address. |
| `ERROR: server certificate does not match server.crt` | Copy the current `server.crt` to the client. The client never falls back to an unverified connection. |
| `ERROR: server has no manifest; ask the operator to run 'lanpull share rescan'` | Run `lanpull share rescan` on the server. |
| `ERROR: no credentials in <client-folder>/auth` | The staged `auth` file is missing; re-export the account folder. |
| `ERROR: share not configured: <share>` | The client's `lanpull.conf` has no `MIRROR_<share>` entry for that share. |
| Stale files are not removed | Deletion is limited to files lanpull delivered earlier; answer the prompt or use `--delete`. |
| `ERROR: another pull is already running` | A second pull for the same destination was refused by the lock file. |
| `lanpull status` warns the manifest is stale | Files in the share are newer than the manifest; run `lanpull share rescan`. |
| `lanpull access doctor` warns a listed path is missing | The policy names a file that is not in the share; add it or remove the rule. |

## License

[MIT](LICENSE) © 2026 Star-Barsuk
