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
this script from the server bundle.
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
from pathlib import Path
from typing import Any, cast
from urllib.parse import quote, urlsplit

__version__ = "2.0.1"

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
    self_update: bool


@dataclasses.dataclass(frozen=True)
class Progress:
    """Progress reporting for one transfer in the current run."""

    index: int
    total: int

    def show(self, path: str, done: int, size: int) -> None:
        """Print the percentage of the current file."""
        percent = 100 if size <= 0 else min(100, done * 100 // size)
        sys.stdout.write(f"\r[{self.index}/{self.total}] {path} {percent}%")
        sys.stdout.flush()

    def finish(self) -> None:
        """Terminate the progress line."""
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
    path.write_text(json.dumps(partials, sort_keys=True), encoding="utf-8")


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


def delete_stale(output: Path, stale: list[str], delivered: set[str]) -> int:
    """Delete stale files and forget them from the delivered set."""
    deleted = 0
    for rel in stale:
        target = output / rel
        with contextlib.suppress(FileNotFoundError):
            target.unlink()
            deleted += 1
        delivered.discard(rel)
    return deleted


def prompt_delete(stale: list[str]) -> bool:
    """List stale files and ask once whether to delete them."""
    for path in stale:
        print(f"STALE: {path}")
    answer = input("Delete these files? [y/N] ").strip().lower()
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
    for share, output in selected.items():
        result = run_share(client, share, output, states.get(share, set()), options)
        states[share] = result.delivered
        if not (options.check or options.dry_run):
            save_state(state_path, states)
        if result.code != 0:
            exit_code = result.code
        if options.check and result.code == 1:
            exit_code = 1
    return exit_code


@dataclasses.dataclass
class _ShareResult:
    """The outcome of pulling one share."""

    code: int
    delivered: set[str]


def run_share(
    client: HttpClient,
    share: str,
    output: Path,
    delivered: set[str],
    options: Options,
) -> _ShareResult:
    """Mirror one share into its output directory."""
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
        stale = sorted(path for path in delivered - manifest_paths if not is_protected(path))

        if options.check:
            updates = len(need) + len(unsure)
            print(f"[{share}] generated_at: {generated_at!s}")
            print(f"[{share}] updates available: {updates}; unchanged: {len(unchanged)}")
            return _ShareResult(1 if updates else 0, delivered)

        if options.dry_run:
            for item in planned:
                if item.action != OK:
                    print(f"[{share}] {item.action}: {item.entry.path}")
            for path in stale:
                print(f"[{share}] STALE: {path}")
            print(f"[{share}] planned: {len(need)} need, {len(unsure)} verify, {len(stale)} stale")
            return _ShareResult(0, delivered)

        if need:
            check_space(output, sum(item.entry.size for item in need))

        partials = load_partials(partials_path)
        downloaded = 0
        total_bytes = 0
        errors: list[str] = []
        total = len(need)
        for index, item in enumerate(need, start=1):
            entry = item.entry
            try:
                fetch_entry(client, share, entry, output, partials, Progress(index, total))
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
        if stale and (options.delete or prompt_delete(stale)):
            deleted = delete_stale(output, stale, delivered)

        print(f"[{share}] downloaded: {downloaded} files, {format_bytes(total_bytes)}")
        print(f"[{share}] unchanged:  {len(unchanged)} files")
        print(f"[{share}] deleted:    {deleted} files")
        print(f"[{share}] errors:     {len(errors)} files")
        for path in errors:
            print(f"  {path}", file=sys.stderr)
        return _ShareResult(1 if errors else 0, delivered)


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


def run_self_update(cdir: Path, client: HttpClient) -> int:
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

    answer = input(f"Update client {local} -> {version}? [y/N] ").strip().lower()
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


def parse_args(argv: list[str]) -> Options:
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser(prog="pull.py", description="lanpull pull client")
    parser.add_argument("share", nargs="?", help="mirror only this share (default: all)")
    parser.add_argument("--check", action="store_true", help="report updates; change nothing")
    parser.add_argument("--dry-run", action="store_true", help="show the plan; change nothing")
    parser.add_argument("--delete", action="store_true", help="delete stale files without asking")
    parser.add_argument("--self-update", action="store_true", help="update pull.py from the server")
    parser.add_argument("--version", action="version", version=__version__)
    parsed = parser.parse_args(argv)

    share = parsed.share if isinstance(parsed.share, str) else None
    return Options(
        share=share,
        check=bool(parsed.check),
        dry_run=bool(parsed.dry_run),
        delete=bool(parsed.delete),
        self_update=bool(parsed.self_update),
    )


def main(argv: list[str] | None = None) -> int:
    """Entry point."""
    options = parse_args(sys.argv[1:] if argv is None else argv)
    directory = client_dir()
    try:
        conf_path = directory / CONF_NAME
        conf = parse_conf(conf_path.read_text(encoding="utf-8")) if conf_path.is_file() else {}
        server_url = conf.get("SERVER_URL")
        if not server_url:
            raise FatalError("ERROR: SERVER_URL is not set in lanpull.conf")
        host, port = parse_server_url(server_url)
        credentials = read_auth(directory / AUTH_NAME)
        context = load_ssl_context(directory / CERT_NAME)
        client = HttpClient(host, port, context, credentials, socket.gethostname())

        if options.self_update:
            return run_self_update(directory, client)
        return run_pull(directory, client, conf, options)
    except FatalError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    except PerFileError as exc:
        print(str(exc), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
