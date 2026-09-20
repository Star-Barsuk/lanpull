# lanpull

A minimal, manual file-distribution tool for a local network.

The server publishes a shared folder over HTTPS; clients pull the latest
version on demand, replacing changed files and removing files that no longer
exist on the server. There is no automatic synchronization, no version history,
and no rollback.

## Features

- Pull-only, explicit updates — no background daemon
- Resumable downloads with `sha256` verification
- Atomic replacement; partial files are never visible
- HTTPS with a pinned self-signed certificate
- Per-client authentication, bound to the client's address
- Self-updating client — no `git` or internet access required on clients

## Requirements

- **Server**: Linux with administrative access, a Rust toolchain, and `systemd`
- **Clients**: Python 3 (standard library only)

## Repository layout

```text
client/    # Python client
server/    # Rust server
config/    # configuration examples
systemd/   # service unit
Makefile
```

## Quick start

```bash
# Server
make deps
make build
make install
make cert
make rescan
make up

# Client (on a client machine)
./pull.py --check
./pull.py
```

## License

[MIT](LICENSE) © 2026 Star-Barsuk
