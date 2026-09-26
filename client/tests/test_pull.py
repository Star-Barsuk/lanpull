"""Tests for the lanpull pull client."""

from __future__ import annotations

import dataclasses
from pathlib import Path

import pytest

import pull


def test_parse_conf() -> None:
    conf = pull.parse_conf('# comment\nSERVER_URL=https://10.0.0.1:8000\nOUTPUT="/srv/mirror"\n')
    assert conf["SERVER_URL"] == "https://10.0.0.1:8000"
    assert conf["OUTPUT"] == "/srv/mirror"


def test_parse_server_url() -> None:
    assert pull.parse_server_url("https://10.0.0.1:8443/") == ("10.0.0.1", 8443)
    assert pull.parse_server_url("https://server.example") == ("server.example", 443)
    with pytest.raises(pull.FatalError):
        pull.parse_server_url("http://10.0.0.1")


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
    assert pull.remote_file_path("slides/deck v2.pptx") == "/_lanpull/file/slides/deck%20v2.pptx"
    assert pull.remote_file_path("a#b?c%d&e") == "/_lanpull/file/a%23b%3Fc%25d%26e"
    expected = "/_lanpull/file/%D1%84%D0%B0%D0%B9%D0%BB.txt"
    assert pull.remote_file_path("\u0444\u0430\u0439\u043b.txt") == expected


def test_is_ignored_and_protected() -> None:
    assert pull.is_ignored("~lock.a")
    assert pull.is_ignored("dir/a.part")
    assert not pull.is_ignored("a.txt")
    assert pull.is_protected(".lanpull.lock")
    assert pull.is_protected(".manifest.tmp")
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
    pull.save_state(path, {"b.txt", "a.txt"})
    assert pull.load_state(path) == {"a.txt", "b.txt"}
    assert pull.load_state(tmp_path / "missing.json") == set()


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
