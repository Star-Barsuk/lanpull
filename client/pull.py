#!/usr/bin/env python3
"""lanpull pull client.

Mirrors one or more lanpull shares into local directories over pinned HTTPS,
replacing changed files, resuming interrupted transfers, verifying sha256, and
optionally deleting files that lanpull delivered earlier but that are gone from
the server. Uses only the Python standard library.

The client lives in one folder: this script, ``lanpull.conf`` (``SERVER_URL``
and one ``MIRROR_<share>=<dir>`` per mirrored share), ``auth``
(``user:password``), ``server.crt`` (the pinned self-signed certificate), and
``state.json`` (delivered paths, keyed by share). ``--self-update`` refreshes
this script from the server bundle. ``--clean`` removes this client's runtime
leftovers (``state.json``, per-mirror lock/validator files, and ``*.part``)
without contacting the server. ``--mirror`` makes the destination an exact copy
of the share, deleting everything not in the manifest (files created locally
included), except the client's own runtime files. ``--dry-run`` prints the plan
as a table, capped at ``LIST_CAP`` rows unless ``--all`` is given.
"""

from __future__ import annotations

import argparse
import base64
import contextlib
import dataclasses
import fcntl
import hashlib
import http.client
import json
import os
import socket
import ssl
import sys
import time
from pathlib import Path
from typing import Any, cast
from urllib.parse import quote, urlsplit

__version__ = "2.3.0"

SCHEME = "whole-file-v1"
SHARE_PREFIX = "/_lanpull/share/"
BUNDLE_MANIFEST_PATH = "/_lanpull/client/manifest.json"
BUNDLE_PREFIX = "/_lanpull/client/"
MIRROR_KEY = "MIRROR_"

CONF_NAME = "lanpull.conf"
AUTH_NAME = "auth"
CERT_NAME = "server.crt"
STATE_NAME = "state.json"

LOCK_NAME = ".lanpull.lock"
PARTIALS_NAME = ".lanpull.partials.json"
INTERNAL = frozenset({LOCK_NAME, PARTIALS_NAME})

RESERVED = "_lanpull"
CHUNK = 64 * 1024
TIMEOUT = 120
LIST_CAP = 50

OK = "OK"
NEED = "NEED"
VERIFY = "VERIFY"


class FatalError(Exception):
    """A fatal error: nothing was changed and the run exits with code 2."""


class PerFileError(Exception):
    """A per-file error: other files continue and the run exits with code 1."""


@dataclasses.dataclass(frozen=True)
class Entry:
    """One manifest entry."""

    path: str
    size: int
    mtime: float
    sha256: str


@dataclasses.dataclass(frozen=True)
class Planned:
    """A manifest entry together with the action it requires."""

    entry: Entry
    action: str


@dataclasses.dataclass(frozen=True)
class Options:
    """Parsed command-line options."""

    share: str | None
    check: bool
    dry_run: bool
    delete: bool
    mirror: bool
    self_update: bool
    clean: bool
    yes: bool
    all: bool
    quiet: bool


@dataclasses.dataclass
class Progress:
    """Progress reporting for one transfer in the current run."""

    index: int
    total: int
    quiet: bool = False
    started: float = dataclasses.field(default_factory=time.monotonic)

    def show(self, path: str, done: int, size: int) -> None:
        """Print the percentage, transfer rate, and ETA of the current file."""
        if self.quiet:
            return
        percent = 100 if size <= 0 else min(100, done * 100 // size)
        line = f"\r[{self.index}/{self.total}] {path} {percent}%"
        elapsed = time.monotonic() - self.started
        if done > 0 and elapsed > 0:
            rate = done / elapsed
            line += f" {format_bytes(int(rate))}/s"
            if size > done:
                line += f" ETA {format_duration((size - done) / rate)}"
        sys.stdout.write(line)
        sys.stdout.flush()

    def finish(self) -> None:
        """Terminate the progress line."""
        if self.quiet:
            return
        sys.stdout.write("\n")
        sys.stdout.flush()


def client_dir() -> Path:
    """Return the directory containing this script."""
    return Path(__file__).resolve().parent


def parse_conf(text: str) -> dict[str, str]:
    """Parse a ``KEY=VALUE`` configuration file."""
    result: dict[str, str] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        result[key.strip()] = value.strip().strip('"').strip("'")
    return result


def load_mirrors(conf: dict[str, str]) -> dict[str, Path]:
    """Return the share-to-directory mapping from the config."""
    mirrors: dict[str, Path] = {}
    for key, value in conf.items():
        if not key.startswith(MIRROR_KEY):
            continue
        share = key[len(MIRROR_KEY) :]
        if not share or not value:
            raise FatalError(f"ERROR: invalid mirror mapping: {key}")
        if not _valid_share(share):
            raise FatalError(f"ERROR: invalid share name: {share}")
        mirrors[share] = Path(value).expanduser()
    return mirrors


def _valid_share(name: str) -> bool:
    """Return whether a share name matches the server's rule."""
    if not name or not (name[0].islower() or name[0].isdigit()):
        return False
    return all(ch.islower() or ch.isdigit() or ch in "_-" for ch in name)


def read_auth(path: Path) -> str:
    """Read ``user:password`` from the credential file."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise FatalError(f"ERROR: no credentials in {path}") from exc
    value = text.strip()
    if not value or ":" not in value:
        raise FatalError(f"ERROR: no credentials in {path}")
    return value


def load_ssl_context(cert: Path) -> ssl.SSLContext:
    """Build a TLS context pinned to the given certificate."""
    if not cert.is_file():
        raise FatalError(f"ERROR: no pinned certificate at {cert}")
    try:
        return ssl.create_default_context(cafile=str(cert))
    except ssl.SSLError as exc:
        raise FatalError(f"ERROR: cannot read the pinned certificate at {cert}: {exc}") from exc


def parse_server_url(url: str) -> tuple[str, int]:
    """Parse an ``https://host[:port]`` URL into host and port."""
    parts = urlsplit(url)
    if parts.scheme != "https":
        raise FatalError(f"ERROR: SERVER_URL must use https: {url}")
    host = parts.hostname
    if host is None:
        raise FatalError(f"ERROR: SERVER_URL has no host: {url}")
    return host, parts.port or 443


def validate_path(path: str) -> None:
    """Reject an unsafe manifest path, aborting the whole manifest."""
    if not path or path.startswith("/") or "\x00" in path:
        raise FatalError(f"ERROR: unsafe path in manifest: {path}")
    for segment in path.split("/"):
        if segment in ("", ".", "..", RESERVED):
            raise FatalError(f"ERROR: unsafe path in manifest: {path}")


def parse_manifest(data: dict[str, Any]) -> tuple[str, list[Entry]]:
    """Validate a data manifest and return its entries."""
    scheme = data.get("scheme")
    if scheme != SCHEME:
        raise FatalError(f"ERROR: unsupported manifest scheme: {scheme!r}")
    raw_files = data.get("files")
    if not isinstance(raw_files, list):
        raise FatalError("ERROR: invalid manifest: files is not a list")

    entries: list[Entry] = []
    for raw in raw_files:
        if not isinstance(raw, dict):
            raise FatalError("ERROR: invalid manifest: bad entry")
        path = raw.get("path")
        size = raw.get("size")
        mtime = raw.get("mtime")
        digest = raw.get("sha256")
        if not isinstance(path, str):
            raise FatalError("ERROR: invalid manifest: bad path")
        validate_path(path)
        if not isinstance(size, int) or isinstance(size, bool):
            raise FatalError(f"ERROR: invalid manifest: bad size for {path}")
        if not isinstance(mtime, (int, float)) or isinstance(mtime, bool):
            raise FatalError(f"ERROR: invalid manifest: bad mtime for {path}")
        if not isinstance(digest, str):
            raise FatalError(f"ERROR: invalid manifest: bad sha256 for {path}")
        entries.append(Entry(path=path, size=size, mtime=float(mtime), sha256=digest))
    return scheme, entries


def is_ignored(rel: str) -> bool:
    """Return whether a path matches the built-in ignore patterns."""
    name = rel.rsplit("/", 1)[-1]
    return (
        name.startswith("~lock.") or name.startswith(".~lock.") or name.endswith((".tmp", ".part"))
    )


def is_protected(rel: str) -> bool:
    """Return whether a path is internal state or an ignored pattern."""
    return rel in INTERNAL or rel.rsplit("/", 1)[-1] in INTERNAL or is_ignored(rel)


def is_internal(rel: str) -> bool:
    """Return whether a path is the client's own runtime state or staging."""
    name = rel.rsplit("/", 1)[-1]
    return name in INTERNAL or name.endswith(".part")


def sha256_file(path: Path) -> str:
    """Compute the hex sha256 of a file, streaming it."""
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(CHUNK), b""):
            digest.update(chunk)
    return digest.hexdigest()


def basic_header(credentials: str) -> str:
    """Encode ``user:password`` as an HTTP Basic header value."""
    token = base64.b64encode(credentials.encode("utf-8")).decode("ascii")
    return f"Basic {token}"


def auth_failure_message(response: http.client.HTTPResponse, credentials: str) -> str:
    """Map a ``401`` response to a clear operator message."""
    user = credentials.split(":", 1)[0]
    reason = response.getheader("X-Lanpull-Reason", "")
    if reason == "not_armed":
        hint = f"ask the operator to run 'lanpull account arm {user}'"
        return f"ERROR: account {user} is not armed (401); {hint}"
    if reason == "foreign_ip":
        return f"ERROR: account {user} is not allowed from this address (401)"
    if reason == "rate_limited":
        return "ERROR: too many failed attempts (401); wait a minute and retry"
    return "ERROR: authentication failed (401)"


class HttpClient:
    """Small HTTPS client carrying the pinned certificate and credentials."""

    def __init__(
        self,
        host: str,
        port: int,
        context: ssl.SSLContext,
        credentials: str,
        hostname: str,
    ) -> None:
        self._host = host
        self._port = port
        self._context = context
        self._credentials = credentials
        self._hostname = hostname

    @property
    def credentials(self) -> str:
        """Return the configured ``user:password`` string."""
        return self._credentials

    def headers(self, extra: dict[str, str] | None = None) -> dict[str, str]:
        """Build the request headers, including Basic auth and the hostname."""
        result = {
            "Authorization": basic_header(self._credentials),
            "X-Lanpull-Host": self._hostname,
        }
        if extra:
            result.update(extra)
        return result

    def connect(self) -> http.client.HTTPSConnection:
        """Open a new HTTPS connection."""
        return http.client.HTTPSConnection(
            self._host, self._port, context=self._context, timeout=TIMEOUT
        )

    def get_json(self, path: str, unavailable: str) -> dict[str, Any]:
        """Fetch and parse a small JSON document."""
        connection = self.connect()
        try:
            connection.request("GET", path, headers=self.headers())
            response = connection.getresponse()
            if response.status == 401:
                raise FatalError(auth_failure_message(response, self._credentials))
            if response.status == 403:
                raise FatalError("ERROR: access denied (403); ask the operator to grant access")
            if response.status == 503:
                raise FatalError(f"ERROR: {unavailable}")
            if response.status != 200:
                raise FatalError(f"ERROR: server returned {response.status} for {path}")
            body = response.read()
        except ssl.SSLError as exc:
            raise FatalError("ERROR: server certificate does not match server.crt") from exc
        except (OSError, http.client.HTTPException) as exc:
            raise FatalError(f"ERROR: cannot reach {self._host}: {exc}") from exc
        finally:
            connection.close()

        try:
            data = json.loads(body)
        except json.JSONDecodeError as exc:
            raise FatalError(f"ERROR: invalid manifest: {exc}") from exc
        if not isinstance(data, dict):
            raise FatalError("ERROR: invalid manifest: not a JSON object")
        return cast("dict[str, Any]", data)


class Lock:
    """An exclusive ``flock`` on the mirror lock file."""

    def __init__(self, path: Path) -> None:
        self._path = path
        self._fd: int | None = None

    def __enter__(self) -> Lock:
        """Acquire the lock or fail with a clear message."""
        self._fd = os.open(self._path, os.O_CREAT | os.O_RDWR, 0o600)
        try:
            fcntl.flock(self._fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as exc:
            os.close(self._fd)
            self._fd = None
            raise FatalError("ERROR: another pull is already running") from exc
        return self

    def __exit__(self, *args: object) -> None:
        """Release the lock."""
        if self._fd is not None:
            with contextlib.suppress(OSError):
                fcntl.flock(self._fd, fcntl.LOCK_UN)
            os.close(self._fd)
            self._fd = None


def load_partials(path: Path) -> dict[str, str]:
    """Load stored resume validators keyed by relative path."""
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    if not isinstance(data, dict):
        return {}
    return {str(key): value for key, value in data.items() if isinstance(value, str)}


def save_partials(path: Path, partials: dict[str, str]) -> None:
    """Persist the resume validators."""
    path.write_text(json.dumps(partials, sort_keys=True, indent=2) + "\n", encoding="utf-8")


def load_state(path: Path) -> dict[str, set[str]]:
    """Load delivered paths keyed by share."""
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    if not isinstance(data, dict):
        return {}
    result: dict[str, set[str]] = {}
    for share, files in data.items():
        if isinstance(share, str) and isinstance(files, list):
            result[share] = {item for item in files if isinstance(item, str)}
    return result


def save_state(path: Path, shares: dict[str, set[str]]) -> None:
    """Persist the delivered paths, keyed by share."""
    payload = {share: sorted(files) for share, files in sorted(shares.items())}
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def manifest_path(share: str) -> str:
    """Return the manifest endpoint for a share."""
    return f"{SHARE_PREFIX}{quote(share, safe='')}/manifest.json"


def remote_file_path(share: str, rel: str) -> str:
    """Percent-encode a share file path for the file endpoint."""
    encoded = "/".join(quote(segment, safe="") for segment in rel.split("/"))
    return f"{SHARE_PREFIX}{quote(share, safe='')}/file/{encoded}"


def plan(entries: list[Entry], output: Path, verify: bool) -> list[Planned]:
    """Build the work list by comparing the manifest with the local mirror."""
    result: list[Planned] = []
    for entry in entries:
        local = output / entry.path
        if not local.is_file():
            result.append(Planned(entry, NEED))
            continue
        stat = local.stat()
        if stat.st_size != entry.size:
            result.append(Planned(entry, NEED))
            continue
        if int(stat.st_mtime) == int(entry.mtime):
            result.append(Planned(entry, OK))
            continue
        if verify and sha256_file(local) == entry.sha256:
            result.append(Planned(entry, OK))
        elif verify:
            result.append(Planned(entry, NEED))
        else:
            result.append(Planned(entry, VERIFY))
    return result


def format_bytes(value: int) -> str:
    """Format a byte count compactly."""
    size = float(value)
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024:
            return f"{size:.1f} {unit}"
        size /= 1024
    return f"{size:.1f} TB"


def format_duration(seconds: float) -> str:
    """Format a duration in seconds compactly, for example ``1m05s``."""
    total = int(seconds)
    if total < 60:
        return f"{total}s"
    minutes, secs = divmod(total, 60)
    if minutes < 60:
        return f"{minutes}m{secs:02d}s"
    hours, minutes = divmod(minutes, 60)
    return f"{hours}h{minutes:02d}m"


def format_rows(rows: list[list[str]]) -> list[str]:
    """Format rows as an aligned table, padding every column but the last."""
    columns = max((len(row) for row in rows), default=0)
    widths = [0] * columns
    for row in rows:
        for index, cell in enumerate(row):
            if index < columns:
                widths[index] = max(widths[index], len(cell))
    lines: list[str] = []
    for row in rows:
        last = len(row) - 1
        parts: list[str] = []
        for index, cell in enumerate(row):
            if index == last:
                parts.append(cell)
            else:
                parts.append(cell + " " * (widths[index] - len(cell) + 2))
        lines.append("".join(parts))
    return lines


def render_changes(
    planned: list[Planned],
    stale: list[str],
    extras: list[str],
    show_all: bool,
) -> list[str]:
    """Render the non-OK plan, stale, and extra rows as a capped table."""
    items: list[tuple[str, str, str]] = [
        (item.action, format_bytes(item.entry.size), item.entry.path)
        for item in planned
        if item.action != OK
    ]
    items.extend(("STALE", "-", path) for path in stale)
    items.extend(("EXTRA", "-", path) for path in extras)
    if not items:
        return []
    limit = len(items) if show_all else LIST_CAP
    rows = [["ACTION", "SIZE", "PATH"]]
    rows.extend([action, size, path] for action, size, path in items[:limit])
    lines = format_rows(rows)
    if not show_all and len(items) > LIST_CAP:
        lines.append(f"... and {len(items) - LIST_CAP} more (use --all to list every file)")
    return lines


def check_space(output: Path, needed: int) -> None:
    """Warn when the mirror may not have room for the download."""
    stat = os.statvfs(output)
    available = stat.f_bavail * stat.f_frsize
    if available < needed:
        print(
            f"WARNING: only {format_bytes(available)} free; about {format_bytes(needed)} needed",
            file=sys.stderr,
        )


def download(
    client: HttpClient,
    share: str,
    entry: Entry,
    output: Path,
    partials: dict[str, str],
    progress: Progress,
) -> None:
    """Download one file to ``<path>.part``, resuming safely when possible."""
    part = output / (entry.path + ".part")
    part.parent.mkdir(parents=True, exist_ok=True)
    remote = remote_file_path(share, entry.path)

    attempts = 0
    while True:
        attempts += 1
        resume_from = part.stat().st_size if part.exists() else 0
        if resume_from > entry.size:
            part.unlink(missing_ok=True)
            partials.pop(entry.path, None)
            resume_from = 0
        validator = partials.get(entry.path)
        if resume_from and not validator:
            part.unlink(missing_ok=True)
            resume_from = 0

        extra: dict[str, str] = {}
        if resume_from and validator is not None:
            extra["Range"] = f"bytes={resume_from}-"
            extra["If-Range"] = validator

        connection = client.connect()
        try:
            connection.request("GET", remote, headers=client.headers(extra))
            response = connection.getresponse()
            if response.status == 401:
                raise FatalError(auth_failure_message(response, client.credentials))
            if response.status == 403:
                raise PerFileError(f"ERROR: {entry.path}: access denied (403)")
            if response.status == 416:
                response.read()
                part.unlink(missing_ok=True)
                partials.pop(entry.path, None)
                if attempts < 2:
                    continue
                raise PerFileError(f"ERROR: {entry.path}: range not satisfiable")
            if response.status not in (200, 206):
                raise PerFileError(f"ERROR: {entry.path}: server returned {response.status}")
            if response.status == 200 and resume_from:
                part.unlink(missing_ok=True)
                partials.pop(entry.path, None)
                resume_from = 0

            new_validator = response.getheader("ETag") or response.getheader("Last-Modified")
            if new_validator:
                partials[entry.path] = new_validator

            mode = "ab" if response.status == 206 else "wb"
            written = resume_from
            with part.open(mode) as handle:
                while True:
                    chunk = response.read(CHUNK)
                    if not chunk:
                        break
                    handle.write(chunk)
                    written += len(chunk)
                    progress.show(entry.path, written, entry.size)
            progress.finish()
            return
        except ssl.SSLError as exc:
            raise FatalError("ERROR: server certificate does not match server.crt") from exc
        except (OSError, http.client.HTTPException) as exc:
            raise PerFileError(f"ERROR: {entry.path}: {exc}") from exc
        finally:
            connection.close()


def fetch_entry(
    client: HttpClient,
    share: str,
    entry: Entry,
    output: Path,
    partials: dict[str, str],
    progress: Progress,
) -> None:
    """Download, verify, and atomically replace one file."""
    part = output / (entry.path + ".part")
    download(client, share, entry, output, partials, progress)

    staged = part.stat().st_size if part.is_file() else 0
    if staged != entry.size:
        # The connection ended before the whole file arrived (for example the
        # server was stopped or the link dropped without raising). Keep the
        # partial and its validator so the next run resumes with Range/If-Range;
        # deleting them here would force a full re-download.
        raise PerFileError(
            f"ERROR: {entry.path}: incomplete ({staged} of {entry.size} bytes); re-run to resume"
        )

    if sha256_file(part) != entry.sha256:
        part.unlink(missing_ok=True)
        partials.pop(entry.path, None)
        raise PerFileError(f"ERROR: {entry.path}: checksum mismatch, not replaced")
    os.utime(part, (entry.mtime, entry.mtime))
    part.replace(output / entry.path)
    partials.pop(entry.path, None)


def delete_stale(output: Path, stale: list[str], delivered: set[str]) -> tuple[int, list[str]]:
    """Delete stale files, returning the count deleted and the paths that failed.

    A file that cannot be removed stays in the delivered set so the next pull
    retries it; only a real removal forgets it.
    """
    deleted = 0
    failed: list[str] = []
    for rel in stale:
        try:
            (output / rel).unlink()
        except FileNotFoundError:
            delivered.discard(rel)
            continue
        except OSError as exc:
            failed.append(rel)
            print(f"ERROR: cannot remove {rel}: {exc}", file=sys.stderr)
            continue
        deleted += 1
        delivered.discard(rel)
    return deleted, failed


def delete_paths(output: Path, rels: list[str]) -> tuple[int, list[str]]:
    """Delete files or symlinks under the mirror, returning deleted and failed."""
    deleted = 0
    failed: list[str] = []
    for rel in rels:
        try:
            (output / rel).unlink()
        except FileNotFoundError:
            continue
        except OSError as exc:
            failed.append(rel)
            print(f"ERROR: cannot remove {rel}: {exc}", file=sys.stderr)
            continue
        deleted += 1
    return deleted, failed


def scan_extras(output: Path, manifest_paths: set[str]) -> list[str]:
    """Return every mirror entry absent from the manifest, symlinks included.

    Regular files, symlinks to files, and symlinks to directories are all
    candidates; real directories are left to ``remove_empty_dirs``. The
    client's own runtime files are never candidates.
    """
    extras: list[str] = []
    for dirpath, dirnames, filenames in os.walk(output, followlinks=False):
        base = Path(dirpath)
        for name in filenames:
            rel = (base / name).relative_to(output).as_posix()
            if rel not in manifest_paths and not is_internal(rel):
                extras.append(rel)
        for name in dirnames:
            candidate = base / name
            if candidate.is_symlink():
                rel = candidate.relative_to(output).as_posix()
                if rel not in manifest_paths and not is_internal(rel):
                    extras.append(rel)
    return sorted(extras)


def remove_empty_dirs(root: Path) -> int:
    """Remove directories left empty under the mirror, deepest first."""
    removed = 0
    for dirpath, _dirnames, _filenames in os.walk(root, topdown=False, followlinks=False):
        path = Path(dirpath)
        if path == root or path.is_symlink():
            continue
        try:
            path.rmdir()
        except OSError:
            continue
        removed += 1
    return removed


def prompt_delete(stale: list[str]) -> bool:
    """List stale files and ask once whether to delete them."""
    for path in stale:
        print(f"STALE: {path}")
    print("Delete these files? [y/N] ", end="", file=sys.stderr)
    answer = input().strip().lower()
    return answer in ("y", "yes")


def confirm_mirror(extras: list[str], show_all: bool) -> bool:
    """List extra files as a table and ask once whether to delete them."""
    for line in render_changes([], [], extras, show_all):
        print(line)
    print(f"Delete these {len(extras)} extra file(s)? [y/N] ", end="", file=sys.stderr)
    answer = input().strip().lower()
    return answer in ("y", "yes")


def run_pull(cdir: Path, client: HttpClient, conf: dict[str, str], options: Options) -> int:
    """Mirror every configured share (or the one named on the command line)."""
    mirrors = load_mirrors(conf)
    if not mirrors:
        raise FatalError("ERROR: no MIRROR_<share> entries in lanpull.conf")

    if options.share is not None:
        if options.share not in mirrors:
            raise FatalError(f"ERROR: share not configured: {options.share}")
        selected = {options.share: mirrors[options.share]}
    else:
        selected = mirrors

    state_path = cdir / STATE_NAME
    states = load_state(state_path)

    exit_code = 0
    downloaded = 0
    total_bytes = 0
    deleted = 0
    errors = 0
    for share, output in selected.items():
        result = run_share(client, share, output, states.get(share, set()), options)
        states[share] = result.delivered
        if not (options.check or options.dry_run):
            save_state(state_path, states)
        if result.code != 0:
            exit_code = result.code
        if options.check and result.code == 1:
            exit_code = 1
        downloaded += result.downloaded
        total_bytes += result.total_bytes
        deleted += result.deleted
        errors += result.errors
    if len(selected) > 1 and not options.quiet and not (options.check or options.dry_run):
        print(
            f"total: downloaded {downloaded} files, {format_bytes(total_bytes)}; "
            f"deleted {deleted}; errors {errors}"
        )
    return exit_code


@dataclasses.dataclass
class _ShareResult:
    """The outcome of pulling one share."""

    code: int
    delivered: set[str]
    downloaded: int = 0
    total_bytes: int = 0
    unchanged: int = 0
    deleted: int = 0
    errors: int = 0


def run_share(
    client: HttpClient,
    share: str,
    output: Path,
    delivered: set[str],
    options: Options,
) -> _ShareResult:
    """Mirror one share into its output directory."""
    if _unsafe_mirror(output):
        raise FatalError(f"ERROR: refusing to use an unsafe mirror path: {output}")
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    partials_path = output / PARTIALS_NAME

    with Lock(output / LOCK_NAME):
        data = client.get_json(
            manifest_path(share),
            "server has no manifest; ask the operator to run 'lanpull share rescan'",
        )
        generated_at = data.get("generated_at")
        _scheme, entries = parse_manifest(data)

        verify = not options.check and not options.dry_run
        planned = plan(entries, output, verify)
        need = [item for item in planned if item.action == NEED]
        unsure = [item for item in planned if item.action == VERIFY]
        unchanged = [item for item in planned if item.action == OK]

        manifest_paths = {entry.path for entry in entries}
        if options.mirror:
            stale: list[str] = []
            extras = scan_extras(output, manifest_paths)
        else:
            stale = sorted(path for path in delivered - manifest_paths if not is_protected(path))
            extras = []

        if options.check:
            updates = len(need) + len(unsure)
            print(f"[{share}] generated_at: {generated_at!s}")
            print(f"[{share}] updates available: {updates}; unchanged: {len(unchanged)}")
            return _ShareResult(1 if updates else 0, delivered, unchanged=len(unchanged))

        if options.dry_run:
            print(f"[{share}] plan (generated_at: {generated_at!s})")
            for line in render_changes(planned, stale, extras, options.all):
                print(line)
            print(
                f"[{share}] planned: {len(need)} need, {len(unsure)} verify, "
                f"{len(stale)} stale, {len(extras)} extra"
            )
            return _ShareResult(0, delivered, unchanged=len(unchanged))

        if need:
            check_space(output, sum(item.entry.size for item in need))

        partials = load_partials(partials_path)
        downloaded = 0
        total_bytes = 0
        errors: list[str] = []
        total = len(need)
        for index, item in enumerate(need, start=1):
            entry = item.entry
            progress = Progress(index, total, options.quiet)
            try:
                fetch_entry(client, share, entry, output, partials, progress)
            except PerFileError as exc:
                errors.append(entry.path)
                print(str(exc), file=sys.stderr)
            else:
                delivered.add(entry.path)
                downloaded += 1
                total_bytes += entry.size
            finally:
                save_partials(partials_path, partials)

        deleted = 0
        if options.mirror:
            if extras and (options.yes or confirm_mirror(extras, options.all)):
                deleted, mirror_failed = delete_paths(output, extras)
                errors.extend(mirror_failed)
                remove_empty_dirs(output)
            delivered = {entry.path for entry in entries if (output / entry.path).is_file()}
        elif stale and (options.delete or prompt_delete(stale)):
            deleted, stale_failed = delete_stale(output, stale, delivered)
            errors.extend(stale_failed)

        if not options.quiet:
            print(f"[{share}] downloaded: {downloaded} files, {format_bytes(total_bytes)}")
            print(f"[{share}] unchanged:  {len(unchanged)} files")
            print(f"[{share}] deleted:    {deleted} files")
            print(f"[{share}] errors:     {len(errors)} files")
        for path in errors:
            print(f"  {path}", file=sys.stderr)
        return _ShareResult(
            1 if errors else 0,
            delivered,
            downloaded=downloaded,
            total_bytes=total_bytes,
            unchanged=len(unchanged),
            deleted=deleted,
            errors=len(errors),
        )


def download_bundle_file(client: HttpClient, name: str, part: Path) -> None:
    """Download one client bundle file to ``<name>.part``."""
    remote = f"{BUNDLE_PREFIX}{quote(name, safe='')}"
    connection = client.connect()
    try:
        connection.request("GET", remote, headers=client.headers())
        response = connection.getresponse()
        if response.status == 401:
            raise FatalError(auth_failure_message(response, client.credentials))
        if response.status == 503:
            raise FatalError(
                "ERROR: server has no client bundle; ask the operator to run make client-bundle"
            )
        if response.status != 200:
            raise FatalError(f"ERROR: cannot download {name}: {response.status}")
        with part.open("wb") as handle:
            while True:
                chunk = response.read(CHUNK)
                if not chunk:
                    break
                handle.write(chunk)
    except ssl.SSLError as exc:
        raise FatalError("ERROR: server certificate does not match server.crt") from exc
    except (OSError, http.client.HTTPException) as exc:
        raise FatalError(f"ERROR: cannot reach the server: {exc}") from exc
    finally:
        connection.close()


def run_self_update(cdir: Path, client: HttpClient, assume_yes: bool = False) -> int:
    """Update this script from the server's client bundle."""
    local = __version__
    if not local:
        raise FatalError("ERROR: cannot determine local client version")

    data = client.get_json(
        BUNDLE_MANIFEST_PATH,
        "server has no client bundle; ask the operator to run make client-bundle",
    )
    version = data.get("version")
    if not isinstance(version, str) or not version:
        raise FatalError("ERROR: invalid bundle manifest")
    if version == local:
        print("client already up to date")
        return 0

    if not assume_yes:
        print(f"Update client {local} -> {version}? [y/N] ", end="", file=sys.stderr)
        answer = input().strip().lower()
        if answer not in ("y", "yes"):
            return 0

    files = data.get("files")
    if not isinstance(files, list):
        raise FatalError("ERROR: invalid bundle manifest")
    for raw in files:
        if not isinstance(raw, dict):
            raise FatalError("ERROR: invalid bundle manifest")
        name = raw.get("path")
        digest = raw.get("sha256")
        if (
            not isinstance(name, str)
            or not name
            or "/" in name
            or name in (".", "..")
            or not isinstance(digest, str)
        ):
            raise FatalError("ERROR: invalid bundle manifest")
        part = cdir / f"{name}.part"
        download_bundle_file(client, name, part)
        if sha256_file(part) != digest:
            part.unlink(missing_ok=True)
            raise FatalError(f"ERROR: {name}: update checksum mismatch, client not replaced")
        part.chmod(0o755)
        part.replace(cdir / name)

    print(f"client updated {local} -> {version}")
    return 0


def _is_regular_file(path: Path) -> bool:
    """Return whether ``path`` is an existing regular file, not a symlink."""
    try:
        return path.is_file() and not path.is_symlink()
    except OSError:
        return False


def _within(path: Path, root: Path) -> bool:
    """Return whether ``path`` resolves to a location inside ``root``."""
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except (OSError, ValueError):
        return False


def _discover_parts(root: Path) -> list[Path]:
    """Return every regular ``*.part`` file under ``root`` without following symlinks."""
    found: list[Path] = []
    for dirpath, _dirnames, filenames in os.walk(root, followlinks=False):
        for name in filenames:
            if not name.endswith(".part"):
                continue
            candidate = Path(dirpath) / name
            if _is_regular_file(candidate) and _within(candidate, root):
                found.append(candidate)
    return sorted(found)


def clean_targets(cdir: Path, mirrors: dict[str, Path]) -> list[Path]:
    """Return this client's runtime leftovers, and nothing else.

    Included: ``state.json`` and any ``*.part`` in the client folder, and per
    mirror the lock file, the resume validators, and every ``*.part``. Delivered
    files, files the operator created, and directories are never included.
    """
    targets: list[Path] = []
    state = cdir / STATE_NAME
    if _is_regular_file(state):
        targets.append(state)
    targets.extend(path for path in sorted(cdir.glob("*.part")) if _is_regular_file(path))

    for output in mirrors.values():
        root = output.expanduser().resolve()
        if not root.is_dir():
            continue
        for name in (LOCK_NAME, PARTIALS_NAME):
            candidate = root / name
            if _is_regular_file(candidate):
                targets.append(candidate)
        targets.extend(_discover_parts(root))

    unique: list[Path] = []
    seen: set[Path] = set()
    for path in targets:
        key = path.resolve()
        if key not in seen:
            seen.add(key)
            unique.append(path)
    return unique


def _unsafe_mirror(path: Path) -> bool:
    """Return whether a mirror path is too broad to scan."""
    text = str(path).strip()
    if not text or text == "/":
        return True
    try:
        return path.expanduser().resolve() == Path("/")
    except OSError:
        return True


def _mirror_busy(root: Path) -> bool:
    """Return whether another pull holds the lock on this mirror."""
    lock = root / LOCK_NAME
    if not _is_regular_file(lock):
        return False
    try:
        fd = os.open(lock, os.O_RDWR)
    except OSError:
        return False
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    except OSError:
        return True
    finally:
        os.close(fd)


def run_clean(cdir: Path, conf: dict[str, str], options: Options) -> int:
    """Remove this client's runtime leftovers without contacting the server."""
    mirrors = load_mirrors(conf)
    if options.share is not None:
        if options.share not in mirrors:
            raise FatalError(f"ERROR: share not configured: {options.share}")
        mirrors = {options.share: mirrors[options.share]}

    active: dict[str, Path] = {}
    for share, output in mirrors.items():
        if _unsafe_mirror(output):
            print(
                f"WARNING: {share}: refusing to scan unsafe mirror path: {output}",
                file=sys.stderr,
            )
            continue
        if _mirror_busy(output.expanduser().resolve()):
            print(f"WARNING: {share}: another pull is running; skipped", file=sys.stderr)
            continue
        active[share] = output

    targets = clean_targets(cdir, active)
    if not targets:
        print("[clean] nothing to remove")
        return 0

    if options.dry_run:
        for path in targets:
            print(f"[clean] would remove: {path}")
        print(f"[clean] would remove: {len(targets)} files")
        return 0

    if not options.yes:
        for path in targets:
            print(f"REMOVE: {path}")
        print(
            f"Remove these {len(targets)} lanpull artifact(s)? [y/N] ",
            end="",
            file=sys.stderr,
        )
        answer = input().strip().lower()
        if answer not in ("y", "yes"):
            print("[clean] nothing removed")
            return 0

    removed = 0
    failures = 0
    for path in targets:
        try:
            path.unlink()
            removed += 1
        except OSError as exc:
            failures += 1
            print(f"ERROR: cannot remove {path}: {exc}", file=sys.stderr)
    print(f"[clean] removed: {removed} files")
    if failures:
        return 1
    return 0


def parse_args(argv: list[str]) -> Options:
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser(prog="pull.py", description="lanpull pull client")
    parser.add_argument("share", nargs="?", help="mirror only this share (default: all)")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--check", action="store_true", help="report updates; change nothing")
    modes.add_argument(
        "--mirror",
        action="store_true",
        help="exact mirror: delete everything not in the manifest (files you created too)",
    )
    modes.add_argument("--self-update", action="store_true", help="update pull.py from the server")
    modes.add_argument(
        "--clean",
        action="store_true",
        help="remove this client's runtime leftovers (no server needed)",
    )
    parser.add_argument("--dry-run", action="store_true", help="show the plan; change nothing")
    parser.add_argument("--delete", action="store_true", help="delete stale files without asking")
    parser.add_argument(
        "--all", action="store_true", help="list every file in a plan or deletion list"
    )
    parser.add_argument(
        "--yes",
        action="store_true",
        help="do not ask before --mirror, --clean, or --self-update",
    )
    parser.add_argument(
        "--quiet", action="store_true", help="suppress the progress bar and per-share summary"
    )
    parser.add_argument("--version", action="version", version=__version__)
    parsed = parser.parse_args(argv)

    share = parsed.share if isinstance(parsed.share, str) else None
    check = bool(parsed.check)
    dry_run = bool(parsed.dry_run)
    delete = bool(parsed.delete)
    mirror = bool(parsed.mirror)
    self_update = bool(parsed.self_update)
    clean = bool(parsed.clean)
    yes = bool(parsed.yes)
    show_all = bool(parsed.all)
    quiet = bool(parsed.quiet)
    if dry_run and (check or self_update):
        parser.error("--dry-run cannot be combined with --check or --self-update")
    if delete and (check or mirror or self_update or clean or dry_run):
        parser.error("--delete is only valid for a normal pull")
    if yes and not (mirror or clean or self_update):
        parser.error("--yes is only meaningful with --mirror, --clean, or --self-update")
    if show_all and not (dry_run or mirror):
        parser.error("--all is only meaningful with --dry-run or --mirror")
    if self_update and share is not None:
        parser.error("--self-update does not take a share")
    return Options(
        share=share,
        check=check,
        dry_run=dry_run,
        delete=delete,
        mirror=mirror,
        self_update=self_update,
        clean=clean,
        yes=yes,
        all=show_all,
        quiet=quiet,
    )


def main(argv: list[str] | None = None) -> int:
    """Entry point."""
    options = parse_args(sys.argv[1:] if argv is None else argv)
    directory = client_dir()
    try:
        conf_path = directory / CONF_NAME
        conf = parse_conf(conf_path.read_text(encoding="utf-8")) if conf_path.is_file() else {}

        # Cleaning is fully offline: it never reads the certificate or credentials.
        if options.clean:
            return run_clean(directory, conf, options)

        server_url = conf.get("SERVER_URL")
        if not server_url:
            raise FatalError("ERROR: SERVER_URL is not set in lanpull.conf")
        host, port = parse_server_url(server_url)
        credentials = read_auth(directory / AUTH_NAME)
        context = load_ssl_context(directory / CERT_NAME)
        client = HttpClient(host, port, context, credentials, socket.gethostname())

        if options.self_update:
            return run_self_update(directory, client, options.yes)
        return run_pull(directory, client, conf, options)
    except FatalError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    except PerFileError as exc:
        print(str(exc), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
