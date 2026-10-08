"""Tests for the lanpull pull client."""

from __future__ import annotations

import dataclasses
import hashlib
import http.client
import os
import ssl
from pathlib import Path
from typing import cast

import pytest

import pull


def test_version_matches_version_file() -> None:
    version_file = Path(__file__).resolve().parent.parent / "VERSION"
    assert version_file.read_text(encoding="utf-8").strip() == pull.__version__


def test_parse_conf() -> None:
    conf = pull.parse_conf(
        '# comment\nSERVER_URL=https://10.0.0.1:8000\nMIRROR_reports="/srv/reports"\n'
    )
    assert conf["SERVER_URL"] == "https://10.0.0.1:8000"
    assert conf["MIRROR_reports"] == "/srv/reports"


def test_load_mirrors() -> None:
    conf = pull.load_mirrors({"MIRROR_reports": "/srv/r", "MIRROR_media": "/srv/m"})
    assert conf == {"reports": Path("/srv/r"), "media": Path("/srv/m")}
    assert pull.load_mirrors({"SERVER_URL": "x"}) == {}
    with pytest.raises(pull.FatalError):
        pull.load_mirrors({"MIRROR_Bad": "/srv/x"})


def test_parse_server_url() -> None:
    assert pull.parse_server_url("https://10.0.0.1:8443/") == ("10.0.0.1", 8443)
    assert pull.parse_server_url("https://server.example") == ("server.example", 443)
    with pytest.raises(pull.FatalError):
        pull.parse_server_url("http://10.0.0.1")


def test_load_ssl_context_missing_is_fatal(tmp_path: Path) -> None:
    with pytest.raises(pull.FatalError):
        pull.load_ssl_context(tmp_path / "server.crt")


def test_load_ssl_context_unreadable_cert_is_fatal(tmp_path: Path) -> None:
    cert = tmp_path / "server.crt"
    cert.write_text("not a certificate", encoding="utf-8")
    with pytest.raises(pull.FatalError):
        pull.load_ssl_context(cert)


def test_read_auth_strips_newlines(tmp_path: Path) -> None:
    auth = tmp_path / "auth"
    auth.write_text("alpha:secret\r\n", encoding="utf-8")
    assert pull.read_auth(auth) == "alpha:secret"


def test_read_auth_missing_is_fatal(tmp_path: Path) -> None:
    with pytest.raises(pull.FatalError):
        pull.read_auth(tmp_path / "auth")
    bad = tmp_path / "bad"
    bad.write_text("nocolon", encoding="utf-8")
    with pytest.raises(pull.FatalError):
        pull.read_auth(bad)


def test_basic_header() -> None:
    assert pull.basic_header("alpha:secret") == "Basic YWxwaGE6c2VjcmV0"


def test_validate_path() -> None:
    for safe in ("a", "a/b", "dir/sub/file.pptx"):
        pull.validate_path(safe)
    for unsafe in ("", "/abs", "..", "a/../b", "a//b", "_lanpull/x", "a/_lanpull"):
        with pytest.raises(pull.FatalError):
            pull.validate_path(unsafe)


def test_parse_manifest() -> None:
    data = {
        "scheme": "whole-file-v1",
        "generated_at": "2026-09-17T12:00:00Z",
        "files": [
            {"path": "a.txt", "size": 5, "mtime": 100, "sha256": "aa"},
            {"path": "sub/b.bin", "size": 10, "mtime": 200, "sha256": "bb"},
        ],
    }
    scheme, entries = pull.parse_manifest(data)
    assert scheme == "whole-file-v1"
    assert entries[0] == pull.Entry("a.txt", 5, 100.0, "aa")
    assert entries[1].path == "sub/b.bin"


def test_parse_manifest_rejects_bad_scheme() -> None:
    with pytest.raises(pull.FatalError):
        pull.parse_manifest({"scheme": "chunked-v1", "files": []})


def test_parse_manifest_rejects_unsafe_path() -> None:
    data = {
        "scheme": "whole-file-v1",
        "files": [{"path": "../escape", "size": 1, "mtime": 1, "sha256": "x"}],
    }
    with pytest.raises(pull.FatalError):
        pull.parse_manifest(data)


def test_remote_file_path_encoding() -> None:
    expected = "/_lanpull/share/reports/file/slides/deck%20v2.pptx"
    assert pull.remote_file_path("reports", "slides/deck v2.pptx") == expected
    assert pull.remote_file_path("reports", "a#b?c%d&e") == (
        "/_lanpull/share/reports/file/a%23b%3Fc%25d%26e"
    )
    expected = "/_lanpull/share/reports/file/%D1%84%D0%B0%D0%B9%D0%BB.txt"
    assert pull.remote_file_path("reports", "\u0444\u0430\u0439\u043b.txt") == expected


def test_manifest_path_per_share() -> None:
    assert pull.manifest_path("reports") == "/_lanpull/share/reports/manifest.json"


def test_is_ignored_and_protected() -> None:
    assert pull.is_ignored("~lock.a")
    assert pull.is_ignored("dir/a.part")
    assert not pull.is_ignored("a.txt")
    assert pull.is_protected(".lanpull.lock")
    assert pull.is_protected(".lanpull.partials.json")
    assert not pull.is_protected("a.txt")


def test_format_bytes() -> None:
    assert pull.format_bytes(0) == "0.0 B"
    assert pull.format_bytes(2048) == "2.0 KB"


def test_plan_actions(tmp_path: Path) -> None:
    local = tmp_path / "a.txt"
    local.write_text("hello", encoding="utf-8")
    stat = local.stat()
    entry = pull.Entry("a.txt", stat.st_size, stat.st_mtime, pull.sha256_file(local))

    planned = pull.plan([entry], tmp_path, verify=True)
    assert planned[0].action == pull.OK

    missing = pull.Entry("b.txt", 1, 1.0, "x")
    planned = pull.plan([missing], tmp_path, verify=True)
    assert planned[0].action == pull.NEED

    changed = dataclasses.replace(entry, size=entry.size + 1)
    planned = pull.plan([changed], tmp_path, verify=False)
    assert planned[0].action == pull.NEED

    touched = dataclasses.replace(entry, mtime=entry.mtime + 10)
    planned = pull.plan([touched], tmp_path, verify=False)
    assert planned[0].action == pull.VERIFY
    planned = pull.plan([touched], tmp_path, verify=True)
    assert planned[0].action == pull.OK


def test_state_roundtrip(tmp_path: Path) -> None:
    path = tmp_path / "state.json"
    pull.save_state(path, {"reports": {"b.txt", "a.txt"}, "media": {"x.mp3"}})
    assert pull.load_state(path) == {"reports": {"a.txt", "b.txt"}, "media": {"x.mp3"}}
    assert pull.load_state(tmp_path / "missing.json") == {}


def test_partials_roundtrip(tmp_path: Path) -> None:
    path = tmp_path / ".lanpull.partials.json"
    pull.save_partials(path, {"a.txt": 'W/"abc"'})
    assert pull.load_partials(path) == {"a.txt": 'W/"abc"'}
    assert pull.load_partials(tmp_path / "missing.json") == {}


def test_sha256_file(tmp_path: Path) -> None:
    path = tmp_path / "a.txt"
    path.write_bytes(b"hello")
    assert pull.sha256_file(path) == (
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    )


class _FakeResponse:
    """Minimal ``HTTPResponse`` stand-in returning a fixed body in chunks."""

    def __init__(self, status: int, chunks: list[bytes], headers: dict[str, str]) -> None:
        self.status = status
        self._chunks = list(chunks)
        self._headers = headers

    def read(self, amt: int | None = None) -> bytes:
        """Return the next chunk, then EOF."""
        return self._chunks.pop(0) if self._chunks else b""

    def getheader(self, name: str, default: str | None = None) -> str | None:
        """Return one response header."""
        return self._headers.get(name, default)


class _FakeConnection:
    """Minimal connection whose every request yields one scripted response."""

    def __init__(self, response: _FakeResponse) -> None:
        self._response = response

    def request(self, method: str, url: str, **kwargs: object) -> None:
        """Record the request; the response is already scripted."""

    def getresponse(self) -> _FakeResponse:
        """Return the scripted response."""
        return self._response

    def close(self) -> None:
        """No-op close."""


class _FakeClient:
    """Minimal ``HttpClient`` stand-in returning scripted responses in order."""

    def __init__(self, responses: list[_FakeResponse]) -> None:
        self._responses = list(responses)

    def connect(self) -> _FakeConnection:
        """Open the next scripted connection."""
        return _FakeConnection(self._responses.pop(0))

    def headers(self, extra: dict[str, str] | None = None) -> dict[str, str]:
        """Echo the extra headers."""
        return dict(extra or {})

    @property
    def credentials(self) -> str:
        """Return fixed credentials."""
        return "user:password"


def test_fetch_entry_keeps_partial_for_resume(tmp_path: Path) -> None:
    body = b"0123456789"
    entry = pull.Entry("a.bin", len(body), 1.0, hashlib.sha256(body).hexdigest())
    partials: dict[str, str] = {}
    progress = pull.Progress(1, 1)

    short = _FakeResponse(200, [body[:4]], {"ETag": '"v1"'})
    short_client = cast("pull.HttpClient", _FakeClient([short]))
    with pytest.raises(pull.PerFileError):
        pull.fetch_entry(short_client, "reports", entry, tmp_path, partials, progress)

    part = tmp_path / "a.bin.part"
    assert part.read_bytes() == body[:4]
    assert partials == {"a.bin": '"v1"'}

    rest = _FakeResponse(206, [body[4:]], {"ETag": '"v1"'})
    rest_client = cast("pull.HttpClient", _FakeClient([rest]))
    pull.fetch_entry(rest_client, "reports", entry, tmp_path, partials, progress)

    assert (tmp_path / "a.bin").read_bytes() == body
    assert not part.exists()
    assert partials == {}


class _DeniedClient(pull.HttpClient):
    """HttpClient whose connect() always returns a scripted 403 response."""

    def connect(self) -> http.client.HTTPSConnection:
        """Return a connection answering every request with 403."""
        return cast("http.client.HTTPSConnection", _FakeConnection(_FakeResponse(403, [], {})))


def test_get_json_access_denied() -> None:
    client = _DeniedClient(
        "example.invalid", 443, ssl.create_default_context(), "user:password", "host"
    )
    with pytest.raises(pull.FatalError, match="access denied"):
        client.get_json("/_lanpull/share/reports/manifest.json", "unavailable")


def test_download_access_denied(tmp_path: Path) -> None:
    entry = pull.Entry("a.bin", 10, 1.0, "x")
    denied = cast("pull.HttpClient", _FakeClient([_FakeResponse(403, [], {})]))
    with pytest.raises(pull.PerFileError, match="access denied"):
        pull.download(denied, "reports", entry, tmp_path, {}, pull.Progress(1, 1))


def test_delete_stale_drops_missing_and_tracked(tmp_path: Path) -> None:
    (tmp_path / "a.txt").write_text("x", encoding="utf-8")
    delivered = {"a.txt", "b.txt"}
    deleted, failed = pull.delete_stale(tmp_path, ["a.txt", "b.txt"], delivered)
    assert deleted == 1
    assert failed == []
    assert not (tmp_path / "a.txt").exists()
    assert delivered == set()


def test_delete_stale_records_failures_and_keeps_tracking(tmp_path: Path) -> None:
    # A directory cannot be unlinked; the failure is reported and the path stays
    # tracked so the next pull retries it.
    (tmp_path / "blocked").mkdir()
    delivered = {"blocked"}
    deleted, failed = pull.delete_stale(tmp_path, ["blocked"], delivered)
    assert deleted == 0
    assert failed == ["blocked"]
    assert delivered == {"blocked"}


def test_prompt_delete(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("builtins.input", lambda: "y")
    assert pull.prompt_delete(["a.txt"])
    monkeypatch.setattr("builtins.input", lambda: "n")
    assert not pull.prompt_delete(["a.txt"])


class _BundleClient:
    """Minimal client serving one bundle manifest and scripted file bodies."""

    def __init__(self, manifest: dict[str, object], bodies: list[bytes]) -> None:
        self._manifest = manifest
        self._bodies = list(bodies)

    def get_json(self, path: str, unavailable: str) -> dict[str, object]:
        """Return the scripted bundle manifest."""
        return self._manifest

    def connect(self) -> _FakeConnection:
        """Return a connection with the next scripted body."""
        return _FakeConnection(_FakeResponse(200, [self._bodies.pop(0)], {}))

    def headers(self, extra: dict[str, str] | None = None) -> dict[str, str]:
        """Return no extra headers."""
        return dict(extra or {})

    @property
    def credentials(self) -> str:
        """Return fixed credentials."""
        return "user:password"


def test_run_self_update_same_version_is_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(pull, "__version__", "2.0.1")
    manifest: dict[str, object] = {"version": "2.0.1", "files": []}
    client = cast("pull.HttpClient", _BundleClient(manifest, []))
    assert pull.run_self_update(tmp_path, client) == 0
    assert not (tmp_path / "pull.py").exists()


def test_run_self_update_downloads_new_script(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(pull, "__version__", "1.0.0")
    monkeypatch.setattr("builtins.input", lambda: "y")
    body = b"new script\n"
    manifest: dict[str, object] = {
        "version": "2.0.0",
        "files": [{"path": "pull.py", "sha256": hashlib.sha256(body).hexdigest()}],
    }
    client = cast("pull.HttpClient", _BundleClient(manifest, [body]))
    assert pull.run_self_update(tmp_path, client) == 0
    assert (tmp_path / "pull.py").read_bytes() == body


def _clean_options(**overrides: object) -> pull.Options:
    """Build a ``--clean`` Options with the given overrides."""
    values: dict[str, object] = {
        "share": None,
        "check": False,
        "dry_run": False,
        "delete": False,
        "mirror": False,
        "self_update": False,
        "clean": True,
        "yes": False,
        "all": False,
        "quiet": False,
    }
    values.update(overrides)
    return pull.Options(**values)  # type: ignore[arg-type]


def test_parse_args_clean_conflicts() -> None:
    with pytest.raises(SystemExit):
        pull.parse_args(["--clean", "--check"])
    with pytest.raises(SystemExit):
        pull.parse_args(["--yes"])
    options = pull.parse_args(["--clean", "--yes", "--dry-run"])
    assert options.clean and options.yes and options.dry_run


def test_clean_targets_only_runtime_residue(tmp_path: Path) -> None:
    cdir = tmp_path / "client"
    mirror = tmp_path / "mirror"
    (mirror / "sub").mkdir(parents=True)
    cdir.mkdir()
    (cdir / "state.json").write_text("{}", encoding="utf-8")
    (cdir / "pull.py.part").write_text("staged", encoding="utf-8")
    (mirror / ".lanpull.lock").write_text("", encoding="utf-8")
    (mirror / ".lanpull.partials.json").write_text("{}", encoding="utf-8")
    (mirror / "sub" / "b.txt.part").write_text("half", encoding="utf-8")
    (mirror / "a.txt").write_text("delivered", encoding="utf-8")
    (cdir / "auth").write_text("user:password", encoding="utf-8")
    (cdir / "lanpull.conf").write_text("SERVER_URL=https://x\n", encoding="utf-8")

    targets = pull.clean_targets(cdir, {"reports": mirror})
    names = {path.name for path in targets}
    assert names == {
        "state.json",
        "pull.py.part",
        ".lanpull.lock",
        ".lanpull.partials.json",
        "b.txt.part",
    }
    assert mirror / "a.txt" not in targets
    assert cdir / "auth" not in targets
    assert cdir / "lanpull.conf" not in targets


def test_run_clean_dry_run_removes_nothing(tmp_path: Path) -> None:
    cdir = tmp_path / "client"
    cdir.mkdir()
    state = cdir / "state.json"
    state.write_text("{}", encoding="utf-8")
    assert pull.run_clean(cdir, {}, _clean_options(dry_run=True)) == 0
    assert state.exists()


def test_run_clean_prompts_and_honors_answer(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cdir = tmp_path / "client"
    cdir.mkdir()
    state = cdir / "state.json"
    state.write_text("{}", encoding="utf-8")

    monkeypatch.setattr("builtins.input", lambda: "n")
    assert pull.run_clean(cdir, {}, _clean_options()) == 0
    assert state.exists()

    monkeypatch.setattr("builtins.input", lambda: "y")
    assert pull.run_clean(cdir, {}, _clean_options()) == 0
    assert not state.exists()


def test_run_clean_yes_needs_no_prompt(tmp_path: Path) -> None:
    cdir = tmp_path / "client"
    cdir.mkdir()
    state = cdir / "state.json"
    state.write_text("{}", encoding="utf-8")
    assert pull.run_clean(cdir, {}, _clean_options(yes=True)) == 0
    assert not state.exists()


def test_run_clean_nothing_to_remove(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    cdir = tmp_path / "client"
    cdir.mkdir()
    assert pull.run_clean(cdir, {}, _clean_options(yes=True)) == 0
    assert "nothing to remove" in capsys.readouterr().out


def test_run_clean_selected_share(tmp_path: Path) -> None:
    cdir = tmp_path / "client"
    first = tmp_path / "first"
    second = tmp_path / "second"
    for directory in (cdir, first, second):
        directory.mkdir()
        (directory / ".lanpull.lock").write_text("", encoding="utf-8")
    conf = {"MIRROR_first": str(first), "MIRROR_second": str(second)}
    assert pull.run_clean(cdir, conf, _clean_options(share="first", yes=True)) == 0
    assert not (first / ".lanpull.lock").exists()
    assert (second / ".lanpull.lock").exists()


def test_run_clean_skips_unsafe_mirror(tmp_path: Path) -> None:
    cdir = tmp_path / "client"
    cdir.mkdir()
    state = cdir / "state.json"
    state.write_text("{}", encoding="utf-8")
    assert pull.run_clean(cdir, {"MIRROR_root": "/"}, _clean_options(yes=True)) == 0
    assert not state.exists()


def _manifest(files: list[tuple[str, bytes]]) -> dict[str, object]:
    """Build a whole-file-v1 manifest for the given (path, body) pairs."""
    return {
        "scheme": "whole-file-v1",
        "generated_at": "2026-01-01T00:00:00Z",
        "files": [
            {
                "path": path,
                "size": len(body),
                "mtime": 1.0,
                "sha256": hashlib.sha256(body).hexdigest(),
            }
            for path, body in files
        ],
    }


class _ManifestClient:
    """Minimal client serving one manifest per share and refusing downloads."""

    def __init__(self, manifests: dict[str, dict[str, object]]) -> None:
        self._manifests = manifests

    def get_json(self, path: str, unavailable: str) -> dict[str, object]:
        """Return the manifest for the requested share."""
        for share, manifest in self._manifests.items():
            if pull.manifest_path(share) == path:
                return manifest
        raise pull.FatalError(unavailable)

    def connect(self) -> _FakeConnection:
        """Fail: these tests never download a file."""
        raise AssertionError("connect() should not be called")


def _options(**overrides: object) -> pull.Options:
    """Build a pull Options with the given overrides."""
    values: dict[str, object] = {
        "share": None,
        "check": False,
        "dry_run": False,
        "delete": False,
        "mirror": False,
        "self_update": False,
        "clean": False,
        "yes": False,
        "all": False,
        "quiet": False,
    }
    values.update(overrides)
    return pull.Options(**values)  # type: ignore[arg-type]


def test_run_pull_requires_a_mirror(tmp_path: Path) -> None:
    client = cast("pull.HttpClient", _ManifestClient({}))
    with pytest.raises(pull.FatalError, match="no MIRROR"):
        pull.run_pull(tmp_path, client, {}, _options())


def test_run_pull_rejects_an_unconfigured_share(tmp_path: Path) -> None:
    mirror = tmp_path / "mirror"
    mirror.mkdir()
    client = cast("pull.HttpClient", _ManifestClient({}))
    with pytest.raises(pull.FatalError, match="not configured"):
        pull.run_pull(tmp_path, client, {"MIRROR_reports": str(mirror)}, _options(share="media"))


def test_check_reports_updates_without_writing_state(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    mirror = tmp_path / "mirror"
    client = cast("pull.HttpClient", _ManifestClient({"reports": _manifest([("a.txt", b"hello")])}))
    conf = {"MIRROR_reports": str(mirror)}
    assert pull.run_pull(tmp_path, client, conf, _options(check=True)) == 1
    text = capsys.readouterr().out
    assert "generated_at: 2026-01-01T00:00:00Z" in text
    assert "updates available: 1" in text
    assert not (tmp_path / "state.json").exists()


def test_check_is_clean_when_the_mirror_matches(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    mirror = tmp_path / "mirror"
    mirror.mkdir()
    target = mirror / "a.txt"
    target.write_bytes(b"hello")
    os.utime(target, (1, 1))
    client = cast("pull.HttpClient", _ManifestClient({"reports": _manifest([("a.txt", b"hello")])}))
    conf = {"MIRROR_reports": str(mirror)}
    assert pull.run_pull(tmp_path, client, conf, _options(check=True)) == 0
    assert "updates available: 0" in capsys.readouterr().out


def test_dry_run_prints_the_plan_and_changes_nothing(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    mirror = tmp_path / "mirror"
    client = cast("pull.HttpClient", _ManifestClient({"reports": _manifest([("a.txt", b"hello")])}))
    conf = {"MIRROR_reports": str(mirror)}
    assert pull.run_pull(tmp_path, client, conf, _options(dry_run=True)) == 0
    text = capsys.readouterr().out
    assert "ACTION" in text
    assert "NEED" in text
    assert "a.txt" in text
    assert "planned: 1 need, 0 verify, 0 stale, 0 extra" in text
    assert not (mirror / "a.txt").exists()
    assert not (tmp_path / "state.json").exists()


def test_dry_run_lists_stale_without_deleting(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    mirror = tmp_path / "mirror"
    mirror.mkdir()
    (mirror / "gone.txt").write_text("old", encoding="utf-8")
    pull.save_state(tmp_path / "state.json", {"reports": {"gone.txt"}})
    client = cast("pull.HttpClient", _ManifestClient({"reports": _manifest([])}))
    conf = {"MIRROR_reports": str(mirror)}
    assert pull.run_pull(tmp_path, client, conf, _options(dry_run=True)) == 0
    text = capsys.readouterr().out
    assert "STALE" in text
    assert "gone.txt" in text
    assert (mirror / "gone.txt").exists()
    assert pull.load_state(tmp_path / "state.json") == {"reports": {"gone.txt"}}


def test_delete_removes_only_tracked_stale(tmp_path: Path) -> None:
    mirror = tmp_path / "mirror"
    mirror.mkdir()
    (mirror / "gone.txt").write_text("old", encoding="utf-8")
    (mirror / "mine.txt").write_text("keep", encoding="utf-8")
    pull.save_state(tmp_path / "state.json", {"reports": {"gone.txt"}})
    client = cast("pull.HttpClient", _ManifestClient({"reports": _manifest([])}))
    conf = {"MIRROR_reports": str(mirror)}
    assert pull.run_pull(tmp_path, client, conf, _options(delete=True)) == 0
    assert not (mirror / "gone.txt").exists()
    assert (mirror / "mine.txt").exists()
    assert pull.load_state(tmp_path / "state.json") == {"reports": set()}


def test_format_rows_pads_all_but_last() -> None:
    assert pull.format_rows([["A", "B"], ["long", "c"]]) == ["A     B", "long  c"]
    assert pull.format_rows([]) == []


def test_render_changes_caps_and_all() -> None:
    planned = [pull.Planned(pull.Entry(f"f{i}.txt", 1, 1.0, "x"), pull.NEED) for i in range(60)]
    capped = pull.render_changes(planned, [], [], show_all=False)
    assert len(capped) == 52
    assert "and 10 more" in capped[-1]
    full = pull.render_changes(planned, [], [], show_all=True)
    assert len(full) == 61
    assert pull.render_changes([], [], [], show_all=True) == []


def test_scan_extras_finds_files_and_symlinks(tmp_path: Path) -> None:
    mirror = tmp_path / "mirror"
    (mirror / "sub").mkdir(parents=True)
    (mirror / "sub" / "extra.txt").write_text("x", encoding="utf-8")
    (mirror / "keep.txt").write_text("k", encoding="utf-8")
    (mirror / ".lanpull.partials.json").write_text("{}", encoding="utf-8")
    (mirror / "a.part").write_text("half", encoding="utf-8")
    (mirror / "link").symlink_to(mirror / "keep.txt")
    assert pull.scan_extras(mirror, {"keep.txt"}) == ["link", "sub/extra.txt"]


def test_remove_empty_dirs(tmp_path: Path) -> None:
    root = tmp_path / "root"
    (root / "a" / "b").mkdir(parents=True)
    (root / "a" / "keep").mkdir()
    (root / "a" / "keep" / "f.txt").write_text("x", encoding="utf-8")
    assert pull.remove_empty_dirs(root) == 1
    assert not (root / "a" / "b").exists()
    assert (root / "a").exists()


def test_mirror_deletes_extras_and_empty_dirs(tmp_path: Path) -> None:
    mirror = tmp_path / "mirror"
    (mirror / "junk").mkdir(parents=True)
    (mirror / "keep.txt").write_bytes(b"data")
    os.utime(mirror / "keep.txt", (1, 1))
    (mirror / "operator.txt").write_text("mine", encoding="utf-8")
    (mirror / "junk" / "x.txt").write_text("x", encoding="utf-8")
    pull.save_state(tmp_path / "state.json", {"reports": {"operator.txt"}})
    client = cast(
        "pull.HttpClient", _ManifestClient({"reports": _manifest([("keep.txt", b"data")])})
    )
    conf = {"MIRROR_reports": str(mirror)}
    assert pull.run_pull(tmp_path, client, conf, _options(mirror=True, yes=True)) == 0
    assert (mirror / "keep.txt").exists()
    assert not (mirror / "operator.txt").exists()
    assert not (mirror / "junk").exists()
    assert (mirror / ".lanpull.lock").exists()
    assert pull.load_state(tmp_path / "state.json") == {"reports": {"keep.txt"}}


def test_mirror_prompt_declined_keeps_extras(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    mirror = tmp_path / "mirror"
    mirror.mkdir()
    (mirror / "operator.txt").write_text("mine", encoding="utf-8")
    client = cast("pull.HttpClient", _ManifestClient({"reports": _manifest([])}))
    conf = {"MIRROR_reports": str(mirror)}
    monkeypatch.setattr("builtins.input", lambda: "n")
    assert pull.run_pull(tmp_path, client, conf, _options(mirror=True)) == 0
    assert (mirror / "operator.txt").exists()


def test_mirror_dry_run_lists_extras_without_deleting(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    mirror = tmp_path / "mirror"
    mirror.mkdir()
    (mirror / "operator.txt").write_text("mine", encoding="utf-8")
    client = cast("pull.HttpClient", _ManifestClient({"reports": _manifest([])}))
    conf = {"MIRROR_reports": str(mirror)}
    assert pull.run_pull(tmp_path, client, conf, _options(mirror=True, dry_run=True)) == 0
    text = capsys.readouterr().out
    assert "EXTRA" in text
    assert "operator.txt" in text
    assert (mirror / "operator.txt").exists()


def test_run_share_rejects_unsafe_mirror(tmp_path: Path) -> None:
    client = cast("pull.HttpClient", _ManifestClient({"reports": _manifest([])}))
    with pytest.raises(pull.FatalError, match="unsafe mirror path"):
        pull.run_share(client, "reports", Path("/"), set(), _options())


def test_parse_args_mode_and_modifier_conflicts() -> None:
    for argv in (
        ["--mirror", "--check"],
        ["--mirror", "--self-update"],
        ["--mirror", "--clean"],
        ["--delete", "--mirror"],
        ["--delete", "--dry-run"],
        ["--all"],
        ["--all", "--check"],
        ["--yes"],
        ["--yes", "--dry-run"],
        ["--dry-run", "--check"],
        ["--dry-run", "--self-update"],
        ["--self-update", "reports"],
    ):
        with pytest.raises(SystemExit):
            pull.parse_args(argv)
    options = pull.parse_args(["--mirror", "--dry-run", "--all", "--yes", "reports"])
    assert options.mirror and options.dry_run and options.all and options.yes
    assert options.share == "reports"
    assert pull.parse_args(["--quiet"]).quiet


def test_load_state_tolerates_corrupt_and_partial(tmp_path: Path) -> None:
    path = tmp_path / "state.json"
    assert pull.load_state(path) == {}
    path.write_text("{not json", encoding="utf-8")
    assert pull.load_state(path) == {}
    path.write_text('{"reports": "not-a-list", "media": ["x", 1]}', encoding="utf-8")
    assert pull.load_state(path) == {"media": {"x"}}


def test_main_without_server_url_exits_two(capsys: pytest.CaptureFixture[str]) -> None:
    assert pull.main([]) == 2
    assert "SERVER_URL" in capsys.readouterr().err


def test_parse_args_version_exits_zero() -> None:
    with pytest.raises(SystemExit) as excinfo:
        pull.parse_args(["--version"])
    assert excinfo.value.code == 0
