# lanpull reference

Detailed reference for the lanpull server and client. See
[README.md](../README.md) for the project overview and a quick start.

## Table of contents

- [How it works](#how-it-works)
- [Command reference](#command-reference)
- [Configuration](#configuration)
- [Worked example](#worked-example)
- [Teardown](#teardown)

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
           account arm      authorize clients for a short window
           report           who pulled what (from the audit log)
           cert             self-signed certificate (SAN = IPs + localhost)
           network          per-network address + certificate profiles
           status           warn on stale manifests / symlinks / policy
         /etc/lanpull/lanpull.conf     configuration (root:group, mode 640)
         /etc/lanpull/lanpull.{clients,access.json,networks.json}   accounts + policy (mode 600)
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

## Command reference

### Common flags

Result data goes to **stdout**; diagnostics, warnings, and prompts go to
**stderr**.

- `--config <path>` — configuration file; overrides `$LANPULL_CONFIG` and `/etc/lanpull/lanpull.conf`.
- `--json` — print one machine-readable JSON envelope on stdout instead of text (schema below).
- `-v`, `--verbose` — increase verbosity (repeatable); overrides `$RUST_LOG`.
- `-q`, `--quiet` — suppress everything but errors; overrides `$RUST_LOG`.
- `--yes` — skip a confirmation.
- `--force` — overwrite an existing target.
- `--dry-run` — report what a mutation would change without writing it.
- `-h`, `--help` — print help.
- `-V`, `--version` — print the version.

With `--json`, one of two envelopes is printed:

```json
{"status":"ok","command":"<path>","data":…,"warnings":[…]}
{"status":"error","command":…,"code":…,"message":…,"hint":…}
```

The first is success, the second failure.

Global options are accepted before or after the subcommand. `--json`, `--yes`,
`--force`, and `--dry-run` are not universal: each command accepts only the ones
documented for it. The configuration path resolves from `--config`, then
`$LANPULL_CONFIG`, then the canonical `/etc/lanpull/lanpull.conf`, so no flag is
needed in normal use.

Exit codes: `0` success, `1` operational failure, `2` usage error, `3` not
initialized, `4` denied, `5` busy. A declined confirmation exits `1`. Run
`lanpull --help` or `lanpull <command> --help` for the full tree.

### `lanpull init`

Create the configuration and state for this machine.

| Option | Meaning |
| --- | --- |
| `--share <name>=<path>` | A share to declare; repeatable. Defaults to a single `default` share at `$HOME/lanpull-share`. |
| `--state-dir <dir>` | Internal state directory (default `/var/lib/lanpull`). |
| `--bind <addr>` | Listen address (default `0.0.0.0`). |
| `--port <port>` | Listen port (default `8000`). |
| `--server-ip <ip>` | Address clients use; embedded in the certificate. Derived from the routing table when omitted. |
| `--force` | Overwrite an existing configuration file. |
| `--dry-run` | Print what would be created without writing anything. |

`init` creates the configuration directory, the share directories, the state
directory (when permitted), and a commented `.lanpullignore` template in each
share. Against the canonical path it must run as root and applies the
`root:<operator-group>` ownership described in the README. A malformed `--share`
or an invalid share name is a usage error.

### `lanpull serve`

Run the HTTPS server in the foreground. Production uses the systemd unit, which
runs `lanpull serve --config /etc/lanpull/lanpull.conf`. `serve` does not use
the JSON envelope; it logs to stderr and stops on `SIGINT`/`SIGTERM`.

### `lanpull status`

Show per-share manifest summaries and the armed accounts, and warn (on stderr)
about a stale manifest, symlinks, reserved-prefix entries, and policy problems.
Exits `0`; with `--json` the warnings are in the envelope's `warnings`. "Clean"
prints `status is clean`.

| Option | Meaning |
| --- | --- |
| `--arm` | Show only the armed accounts and their remaining windows. This is a cheap check that reads the arm state and never walks the shares. |

### `lanpull cert`

Generate the self-signed certificate and key (`CERT_PATH`, `KEY_PATH`; the key
is mode `600`). The SAN carries `SERVER_IP`, `127.0.0.1`, and `localhost`.

| Option | Meaning |
| --- | --- |
| `--force` | Regenerate an existing certificate and key. |

Without `--force`, an existing certificate is a usage error.

### `lanpull share`

| Command | Purpose |
| --- | --- |
| `share list` | List the declared shares and their directories. |
| `share rescan` | Regenerate the per-share and per-account manifests. |
| `share add <name> <dir>` | Append a `SHARE_<name>` line and create the `.lanpullignore` template (`--dry-run`). |

A share is never removed by a command: delete its `SHARE_<name>` line by hand,
or repoint it with `config set`. Against the canonical configuration, `share
add` needs root. Any command that regenerates manifests prints one line per
share; `-v` adds a per-account breakdown.

### `lanpull access`

The JSON policy maps each share to a `public` set plus per-client `add` and
`remove` deltas. The effective set for an account is `(public − remove) ∪ add`,
sorted by path; the default is deny. A `<path>` is share-relative glob: `*`
matches within a segment, `**` across segments.

| Command | Purpose |
| --- | --- |
| `access public add <share>:<path>...` | Add paths visible to every account (`--dry-run`). |
| `access public remove <share>:<path>...` | Remove paths from the public set (`--yes`, `--dry-run`). |
| `access public list` | Show the public set (`--share <share>`). |
| `access client add <name> <share>:<path>...` | Add paths for one account (`--dry-run`). |
| `access client remove <name> <share>:<path>...` | Remove paths (including public ones) for one account (`--yes`, `--dry-run`). |
| `access client list [<name>]` | Show the effective set per account (or one). |
| `access import` | One-time migration from the old flat `lanpull.access` file (`--from <file>`, `--dry-run`). |
| `access doctor` | Flag unknown shares, missing files, empty accounts, and stale removals. |
| `access apply` | Regenerate the manifests after a manual policy edit. |

Adding a rule for an account that does not exist yet is allowed; it takes effect
when the account is created. A rule that names an unknown share is rejected.

### `lanpull account`

| Command | Purpose |
| --- | --- |
| `account add <name>` | Create an account and stage a client folder (`--output <dir>`, `--ip <ip>`, `--local`, `--dry-run`). |
| `account remove <name>` | Revoke an account, drop its policy deltas, and clear its arm window (`--yes`, `--dry-run`). |
| `account list` | List accounts with their IP, scope, and effective rules. |
| `account passwd <name>` | Rotate the password and refresh the staged `auth` file (`--output <dir>`). |
| `account export <name>` | Copy the staged folder for transfer to the client (`--to <dir>`, `--move`, `--force`, `--dry-run`). |
| `account arm [<name>...]` | Authorize accounts (or all non-local accounts with `--all`) for a window (`--ttl <duration>`, default `15m`). |
| `account disarm [<name>...]` | Clear arm windows (or every one with `--all`, which prompts unless `--yes`). |

`account add` requires, in order: a staged client bundle (`make install`), at
least one access rule for the account, and a certificate. It generates a random
password, stores only its argon2 hash in `lanpull.clients`, and stages a folder
(`pull.py`, `lanpull.conf`, `auth`, `server.crt`) under
`$STATE_DIR/client-ready/<name>` with one `MIRROR_<share>` line per share the
account can see. `--local` marks a self-share account (exempt from arming);
`--ip` binds the account to a source address.

`account passwd` prints the new password to stderr, never to stdout or the JSON
envelope. `account export` copies the folder to `<dir>` (or into `<dir>/<name>`
when `<dir>` already exists); `--force` overwrites, `--move` moves the staging
copy, and exporting to another filesystem prints a warning that `auth` and
`server.crt` now live on that device. Omitting `--ip` leaves the account usable
from any LAN address.

### `lanpull config`

| Command | Purpose |
| --- | --- |
| `config show` | Print the resolved configuration and its paths. |
| `config get <key>` | Print one value; `SHARE_<name>` returns a share directory. |
| `config set <key> <value>` | Set one value (`--dry-run`). |
| `config path` | Print the resolved configuration file path. |

`config set` accepts `STATE_DIR`, `BIND`, `PORT`, `SERVER_IP`, `CERT_PATH`,
`KEY_PATH`, `CLIENTS_PATH`, `ACCESS_PATH`, `NETWORKS_PATH`, and `AUDIT_LOG`; it
refuses `SHARE_<name>` (use `share add`) and an empty value. Against the
canonical configuration it needs root.

### `lanpull network`

Manage named per-network profiles, so one server can move between LANs without
editing the configuration by hand. Each profile records the server address and
the certificate and key used on that network; profiles live in `NETWORKS_PATH`
(default `lanpull.networks.json`, mode 600, never committed).

| Command | Purpose |
| --- | --- |
| `network add <name>` | Register a profile (`--ip <ip>`, `--cert <path>`, `--key <path>`, `--generate`, `--force`). |
| `network list` | List profiles and mark the active one. |
| `network show [<name>]` | Show one profile (the active one by default). |
| `network use <name>` | Apply the profile and ensure its certificate (`--dry-run`, `--regenerate-cert`, `--restart`). |
| `network remove <name>` | Drop a profile, optionally deleting its TLS material (`--delete-cert`, `--yes`). |

`network add` defaults the certificate and key to `<STATE_DIR>/net-<name>.crt`
and `.key`. `network use` sets `SERVER_IP`, `CERT_PATH`, and `KEY_PATH`, and
generates the certificate only when it is missing; it never regenerates an
existing certificate (pass `--regenerate-cert` to rotate it, then redistribute
`server.crt`) and never touches another network's material — so the clients of
one network keep working while the server runs on another. `--restart` restarts
the service after applying the profile. Changing the server address still
requires exporting that network's account folders once (`lanpull account
export`), and every round still needs `lanpull account arm`.

```bash
lanpull network add home   --ip <server-ip-home>
lanpull network add office --ip <server-ip-office> --generate
sudo lanpull network use office --restart
lanpull account arm --all --ttl 15m
```

### `lanpull service`

Thin wrappers over `systemctl`. `start`/`stop`/`restart` report one line;
`status` and `logs` pass the `systemctl` output through (an inactive unit is a
normal result, not an error). The unit does not autostart on boot and has no
`[Install]` section, so the operator starts it explicitly for a round.

| Command | Purpose |
| --- | --- |
| `service start` | Start the unit. |
| `service stop` | Stop the unit. |
| `service restart` | Restart the unit. |
| `service status` | Pass `systemctl status` through. |
| `service logs` | Forward to `journalctl` (`-n`, `--since`, `--no-follow`, `--priority`). |

### `lanpull audit`

Print every artifact lanpull created on this host (configuration, accounts,
policy, certificate, key, state, manifests, arm state, audit log, bundle, and
each share directory) with `present`/`missing`.

### `lanpull report`

Summarize the audit log as a table: per account, files, size, and rejections.

| Option | Meaning |
| --- | --- |
| `--user <name>` | Restrict to one account (`--account` is a hidden alias). |
| `--since <duration>` | Only requests newer than the duration, for example `7d` or `1h`. |
| `--reasons` | Break the rejections down by reason. |
| `--rejected` | Show only accounts with rejected requests. |
| `--tail <n>` | Show the newest `<n>` raw records instead of the summary. |

An empty or missing log prints `no requests recorded`.

### `lanpull clean`

Remove the server's runtime leftovers: orphaned `client-ready/<name>` folders
for accounts that no longer exist. It does not touch shares, the state directory
itself, or installed artifacts.

| Option | Meaning |
| --- | --- |
| `--dry-run` | List what would be removed without removing it. |
| `--yes` | Do not ask for confirmation. |

Without `--yes` and without a terminal, `clean` is a usage error.

### Client commands

The client is `<client-folder>/pull.py`. It reads `lanpull.conf`, `auth`, and
`server.crt` from its own directory and writes `state.json` there.

- `[<share>]` — mirror only this configured share (default: every share).
- `--check` — compare sizes and mtimes; download nothing; prints `generated_at` and the update/unchanged counts.
- `--mirror` — exact mirror: after the pull, delete everything in the destination that is not in the manifest (files you created included) and remove the directories left empty; prompts once, `--yes` skips the prompt; a mirror that resolves to `/` is refused.
- `--dry-run` — print the plan as a table; change nothing.
- `--delete` — delete tracked stale files without prompting (normal pull only).
- `--all` — list every file in a plan or deletion list (default: the first 50 rows).
- `--self-update` — replace `pull.py` from the served bundle when the versions differ.
- `--clean` — remove this client's runtime leftovers offline.
- `--yes` — skip the `--mirror`/`--clean`/`--self-update` confirmation.
- `--quiet` — suppress the progress bar and the per-share summary.
- `--version` — print the client version.

Modes (`--check`, `--mirror`, `--self-update`, `--clean`) are mutually exclusive.
`--dry-run` works with a normal pull, `--mirror`, and `--clean`; `--delete` is a
normal-pull modifier only; `--all` requires `--dry-run` or `--mirror`; `--yes` is
only meaningful with `--mirror`, `--clean`, or `--self-update`. A normal pull and
`--delete` limit deletion to files lanpull delivered earlier, recorded in
`state.json`; files you created in the mirror folder are never touched. Only
`--mirror` deletes those files too, which is what makes the destination an exact
copy of the share.

> [!CAUTION]
> `--mirror` deletes files you created in the destination as well, making it an
> exact copy of the share.

During a pull, each file shows a progress line with percentage, transfer rate,
and ETA; each share ends with a summary; and when more than one share is
mirrored a final `total:` line combines them.

| Mode | Behavior | Exit code |
| --- | --- | --- |
| `pull.py` | Download changed files, verify, replace atomically; prompt once to delete tracked stale files. | `0` clean, `1` per-file errors, `2` fatal |
| `pull.py --check` | Compare sizes and mtimes only; download nothing. | `0` up to date, `1` updates available |
| `pull.py --dry-run` | Print the `NEED`/`VERIFY`/`STALE`/`EXTRA` plan as a table; change nothing. | `0` |
| `pull.py --mirror` | Like a normal pull, then delete every file not in the manifest and remove emptied directories. Prompts once unless `--yes`. | as `pull.py` |
| `pull.py --delete` | Like a normal pull, but delete tracked stale files without prompting. | as `pull.py` |
| `pull.py --self-update` | Compare versions and replace `pull.py` from the served bundle. | `0` current/declined/updated, `2` fatal |
| `pull.py --clean` | Remove `state.json`, per-mirror `.lanpull.lock`/`.lanpull.partials.json`, and every `*.part`. Never contacts the server. | `0` removed/declined, `1` per-file errors, `2` fatal |
| `pull.py --version` | Print the client version. | `0` |

## Configuration

> [!WARNING]
> `/etc/lanpull/lanpull.conf`, `lanpull.clients`, `lanpull.access.json`,
> `lanpull.networks.json`, and the client's `auth` and `server.key` hold
> credentials, access policy, or private keys. They are mode `600`/`640` and are
> never committed.

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
| `NETWORKS_PATH` | JSON network profiles. Relative paths resolve against the config file. | `lanpull.networks.json` |
| `AUDIT_LOG` | JSON-lines audit log. | `/var/lib/lanpull/access.log` |

The real file is mode 640 and is never committed. Blank lines and `#` comments
are ignored, values may be quoted, and `$NAME`/`${NAME}` environment references
are expanded. At least one `SHARE_<name>` key is required. Nothing lanpull
generates (`STATE_DIR`, `CERT_PATH`, `KEY_PATH`, `CLIENTS_PATH`, `ACCESS_PATH`,
`NETWORKS_PATH`, `AUDIT_LOG`) may live inside a share root.

A commented template lives in [`config/lanpull.conf.example`](../config/lanpull.conf.example).

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
`sub/target.pdf` (target). The server is already installed and initialized (see
[Quick start](../README.md#quick-start)).

```bash
# --- access policy ---
lanpull access public add cube:sub/common.pdf
lanpull access client add client-01 cube:sub/target.pdf
lanpull access client list                   # client-01 sees both; others see common only
lanpull access doctor

# --- accounts + staged folders ---
lanpull account add client-01 --ip <client-ip> --output <output-dir>
lanpull account add client-02 --output <output-dir>
lanpull account export client-01 --to <dir>
lanpull account export client-02 --to <dir>

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

> [!CAUTION]
> `make uninstall` and `make wipe` delete `/etc/lanpull` (configuration,
> accounts, policy) and `$STATE_DIR` (manifests, TLS material, arm state, audit
> log, staged bundle). They are irreversible.

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
