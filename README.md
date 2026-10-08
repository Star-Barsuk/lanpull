<div align="center">

# lanpull

[![Rust](https://img.shields.io/badge/Rust-1.88%2B-orange?style=flat&logo=rust&logoColor=white)](server/Cargo.toml) [![Python](https://img.shields.io/badge/Python-3.11%2B-3776AB?style=flat&logo=python&logoColor=white)](client/pyproject.toml) [![TLS](https://img.shields.io/badge/TLS-pinned-brightgreen?style=flat&logo=letsencrypt&logoColor=white)](#security-model)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE) [![Platform](https://img.shields.io/badge/Platform-Linux-blueviolet?style=flat&logo=linux&logoColor=white)](#requirements) [![Reference](https://img.shields.io/badge/docs-REFERENCE-blue?style=flat)](docs/REFERENCE.md)

A minimal, manual file-distribution tool for a local network.

<a href="docs/REFERENCE.md"><strong>Explore the reference ▶</strong></a>

</div>

The server publishes one or more shared directories over HTTPS; clients pull the
latest version on demand, replacing changed files and removing files that no
longer exist on the server. There is no automatic synchronization, no version
history, and no rollback.

Sometimes a small group of machines on one LAN needs to exchange large office
files that are updated rarely and replaced whole. A background sync daemon is
then more machinery than the job needs. `lanpull` takes the opposite approach:
the owner edits the files on one machine (the server), and the operator walks to
each client and runs one explicit command that fetches only what changed,
resumably, over a pinned TLS channel. Nothing runs in the background, nothing is
stored twice, and every pull is an intentional act. It is not a replacement for
Git, Syncthing, or Nextcloud — it is a deliberately small pull-only mirror for a
trusted local network.

<p align="center">
  <a href="#quick-start">Quick start</a> •
  <a href="#security-model">Security</a> •
  <a href="#troubleshooting">Troubleshooting</a>
</p>

## ✨ Features

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

## 🎬 Demo

Arm the clients on the server, watch the log, then pull on each client.

```console
$ lanpull account arm --all --ttl 15m
armed client-01 for 15m
armed client-02 for 15m

$ lanpull report --since 1h
ACCOUNT    LAST SEEN             HOST          FILES  SIZE      REJECTED
client-01  2026-10-09T10:12:04Z  client-01-pc     42  1.4 GB           0
client-02  2026-10-09T10:11:58Z  <unknown>        17  812.3 MB         0
```

```console
$ ./pull.py
[reports] downloaded: 3 files, 42.1 MB
[reports] unchanged:  39 files
[reports] deleted:    0 files
[reports] errors:     0 files
[media] downloaded: 1 files, 1.8 GB
[media] unchanged:  12 files
[media] deleted:    1 files
[media] errors:     0 files
total: downloaded 4 files, 1.9 GB; deleted 1; errors 0
```

## 🎯 Goals and non-goals

**Goals**

- One explicit, resumable pull from a single server to many clients on a trusted
  LAN — nothing runs in the background.
- Whole-file replace semantics: the mirror matches the share, with no history and
  no rollback.
- A standard-library `python3` client that needs no `sudo`, no `pip`, no `git`,
  and no internet access.
- Per-account, per-share access with default deny, plus an explicit arm window
  around each round.

**Non-goals**

- Not bidirectional: clients never push, and there is no conflict resolution.
- Not version control or backup: no history, deduplication, or restore.
- Not internet-facing or multi-tenant: it assumes a LAN and a trusted operator.
- Not a replacement for Git, Syncthing, or Nextcloud.

<a id="requirements"></a>
## 📋 Requirements

**Server**

- Linux with administrative access, a Rust toolchain, and `systemd`.
- A stable address: a static IP or a DHCP reservation. The address is embedded
  in the certificate, so it must not change while a certificate is in use.

**Clients**

- A stock `python3` (3.11 or newer, standard library only). No `sudo`, no `apt`,
  no `git`, and no internet access are required.

---

<a id="quick-start"></a>
## 🚀 Quick start

The end-to-end lifecycle. Steps that need root are marked; everything else runs
as the operator.

### 1. Build and install the server

```bash
git clone <repo> lanpull
cd lanpull
make setup       # Rust target and the quality-gate tooling
make build       # static binary (x86_64-unknown-linux-musl)
make install     # binary + unit + /etc/lanpull + client bundle (escalates as needed)
```

`make` only works with the source tree: it builds and installs, nothing else.
After `make install`, the operator drives the server with the installed
`lanpull` binary, and the configuration is created by the binary itself.

### 2. Create the configuration and certificate

```bash
sudo lanpull init --share default=<share-dir>   # root; derives SERVER_IP
lanpull cert                                    # SAN = SERVER_IP, 127.0.0.1, localhost
```

`lanpull init` derives `SERVER_IP` from the main routing table. Because the
target is the canonical `/etc/lanpull/lanpull.conf`, it must run as root; it sets
`/etc/lanpull` to `root:<operator-group>` mode `0770` and the file to mode
`0640`, so the service (running as the operator) can read it and the operator
can manage accounts and policy without `sudo`. All other commands run as the
installing user. The canonical layout is:

```text
/usr/local/bin/lanpull                  the binary
/etc/lanpull/                           root:<operator-group>, mode 0770
  lanpull.conf                          configuration (root:group, 0640)
  lanpull.clients                       accounts (operator-owned, 0600)
  lanpull.access.json                   access policy (operator-owned, 0600)
  lanpull.networks.json                 network profiles (operator-owned, 0600)
/var/lib/lanpull/                       state, owned by the operator
/etc/systemd/system/lanpull.service     unit (reads /etc/lanpull/lanpull.conf)
```

### 3. Declare access and create accounts

```bash
lanpull access public add <share>:**                        # everyone sees the whole share
lanpull access client add <client-name> <share>:<path>      # or one client, one path
lanpull access doctor

lanpull account add <client-name> --ip <client-ip> --output <output-dir>
lanpull account export <client-name> --to <dir>             # copy this folder to the client
```

The client is a single self-contained directory. `account add` stages a ready
folder (`pull.py`, `lanpull.conf`, `auth`, `server.crt`) and `account export`
copies it to the client machine; see [Register a client](docs/REFERENCE.md#register-a-client).

> [!IMPORTANT]
> The certificate and at least one access rule must exist before an account is
> created (an account with no rules is rejected); a manifest must exist before
> the first pull. The server refuses to start (`serve`) without accounts and a
> certificate.

### 4. Build manifests and serve

```bash
lanpull share rescan       # or: lanpull access apply
lanpull service start
```

> [!WARNING]
> `serve` binds to `0.0.0.0` by default. Restrict access to the LAN with the
> host firewall.

### 5. Arm the window and pull

```bash
lanpull account arm --all --ttl 15m
# on the client, from its folder:
./pull.py --check
./pull.py
```

> [!IMPORTANT]
> The arm window is the network-side barrier: without it an account's requests
> are rejected with `401`. `local` (self-share) accounts are exempt.

### 6. Inspect and close the window

```bash
lanpull status --arm            # remaining time per account
lanpull report --since 1h
lanpull account disarm --all    # or: lanpull account disarm <client-name>...
```

### 7. Later rounds

Edit the share, then rescan and re-arm, and pull again on each client:

```bash
lanpull share rescan && lanpull account arm --all --ttl 15m
```

`share rescan` rebuilds the per-share and per-account manifests. Any command that
changes accounts or the policy regenerates them automatically, so an explicit
`rescan` is only needed after you edit files in a share by hand.

---

<a id="security-model"></a>
## 🔒 Security model

Access is controlled by four per-request checks:

1. a per-client account password (argon2, verified in constant time);
2. if the account has a registered IP, the request must come from it;
3. the account must be armed by the operator for the current window (a `local`
   self-share account is exempt);
4. the requested share and path must be allowed by the account's access rules.

For an IP-bound account these checks stop a copied client folder from being used
on another machine. For an account without an IP binding, the `arm` window is the
only barrier on the network side, while the access mapping limits what any
account can see at all (a `403` otherwise). Every request is logged, and
`lanpull report` shows who pulled what and any rejected attempts.

> [!WARNING]
> **What this does not do.** It does not protect data already mirrored on a
> compromised client, and a determined attacker on the same subnet could steal a
> registered IP — or use an unbound account — during the short `arm` window.
> Credentials and the certificate live in files and are never copy-proof.
> Protecting the mirrored data itself is an OS concern: full-disk encryption,
> screen locking, and separate accounts. Access should also be limited to the
> LAN by the host firewall, since `serve` binds to `0.0.0.0` by default.

To report a vulnerability, follow [docs/SECURITY.md](docs/SECURITY.md).

## 📁 Repository layout

```text
client/     # Python client, config examples, and Python tooling
server/     # Rust crate and Rust tooling
config/     # server configuration examples
systemd/    # service unit
Makefile    # thin pattern forwarder to server/ and client/
```

`client/` and `server/` can be checked out independently with
`git sparse-checkout set client` or `set server`.

## 🛠️ Development

Each side carries its own Makefile and its own static-analysis configuration;
the root `Makefile` forwards to both.

```bash
make -C server ci    # shell scripts, rustfmt, Clippy, rustdoc, tests, machete, CLI test, deny, audit, audit bin, geiger
make -C client ci    # ruff check/format, mypy --strict, pytest, pip-audit
make -C server e2e   # loopback end-to-end round of the real server and pull.py
make -C server cli-test  # operator-style CLI checks (binary only)
make ci              # both sides' quality gates
```

`make setup` installs the Rust target and the pinned gate tools; the client gates
create their uv-managed `.venv` on first run (`make -C client setup`). The
toolchain (`server/rust-toolchain.toml`) and the gate tools are pinned to
known-good versions; bump them deliberately, never inline. There is intentionally
**no CI workflow and no git hook** — the gates are run deliberately with
`make ci`.

---

<a id="troubleshooting"></a>
## 🩺 Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| `ERROR: account <name> is not armed (401)` | Run `lanpull account arm <name> --ttl 15m` on the server, then retry. |
| `ERROR: account <name> is not allowed from this address (401)` | The account is IP-bound and the machine is not at its registered address. Re-add it without `--ip`, or use the registered address. |
| `ERROR: server certificate does not match server.crt` | Copy the current `server.crt` to the client. |
| `ERROR: server has no manifest; ask the operator to run 'lanpull share rescan'` | Run `lanpull share rescan` on the server. |
| `ERROR: no credentials in <client-folder>/auth` | The staged `auth` file is missing; re-export the account folder. |
| `ERROR: share not configured: <share>` | The client's `lanpull.conf` has no `MIRROR_<share>` entry for that share. |
| Stale files are not removed | A normal pull deletes only files lanpull delivered earlier; answer the prompt, use `--delete`, or use `--mirror`. |
| `ERROR: another pull is already running` | A second pull for the same destination was refused by the lock file. |
| `lanpull status` warns the manifest is stale | Files in the share are newer than the manifest; run `lanpull share rescan`. |
| `ERROR: refusing to use an unsafe mirror path: <dir>` | The client refuses a `MIRROR_<share>` that resolves to `/`; point it at a real directory. |
| `warning: unrecognized configuration key: <key>` | A typo in `/etc/lanpull/lanpull.conf`; fix the key. |

More symptoms and every command flag are in [docs/REFERENCE.md](docs/REFERENCE.md).

---

## 🤝 Contributing

Bug reports, ideas, and patches are welcome; the guidelines are in
[docs/CONTRIBUTING.md](docs/CONTRIBUTING.md). Run `make ci` before opening a
pull request.

## 📄 License

MIT — see [LICENSE](LICENSE).

---

<div align="center">

**© 2026 Star-Barsuk**

</div>
