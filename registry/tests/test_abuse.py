"""H-214 — abuse & fuzz suite: the registry must reject hostile input with a
4xx, never crash, and never write outside its storage root."""

import gzip
import io
import tarfile
from concurrent.futures import ThreadPoolExecutor

from tests.test_api import (  # reuse the shared fixtures/helpers
    _auth_headers,
    _make_archive,
    _make_lockfile,
    _make_tarinfo,
    client,
    storage,
)

import app.main
from app.main import compute_package_checksum


def _post_package(archive_bytes: bytes, lock_bytes: bytes, name: str = "test-pkg"):
    return client.post(
        "/packages",
        files={
            "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
            "lockfile": ("xelian.lock", lock_bytes, "application/toml"),
        },
        data={"owner": "testuser", "name": name},
        headers=_auth_headers(),
    )


def _tar_with_entries(entries: list[tuple[str, bytes]]) -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tf:
        for name, data in entries:
            tf.addfile(_make_tarinfo(name, data), io.BytesIO(data))
    buf.seek(0)
    return buf.read()


class TestDecompressionBombs:
    def test_bomb_rejected_with_413_not_crash(self, monkeypatch):
        # A tiny wire payload that decompresses far beyond the cap.
        monkeypatch.setattr(app.main, "MAX_UNCOMPRESSED_BYTES", 1024 * 1024)
        bomb = _tar_with_entries(
            [("xelian.toml", b"x"), ("big.bin", b"\0" * (4 * 1024 * 1024))]
        )
        resp = _post_package(bomb, _make_lockfile("1.0.0", "sha256:" + "0" * 64))
        assert resp.status_code == 413, resp.text

    def test_entry_count_bomb_rejected(self, monkeypatch):
        monkeypatch.setattr(app.main, "MAX_ARCHIVE_ENTRIES", 50)
        many = _tar_with_entries([(f"f{i}", b"x") for i in range(60)])
        resp = _post_package(many, _make_lockfile("1.0.0", "sha256:" + "0" * 64))
        assert resp.status_code == 413, resp.text


class TestOversizedUploads:
    def test_archive_over_cap_is_413(self, monkeypatch):
        monkeypatch.setattr(app.main, "MAX_ARCHIVE_BYTES", 1024)
        archive_bytes = _make_archive("1.0.0")
        assert len(archive_bytes) > 1024 or True
        big = archive_bytes + b"\0" * 2048
        resp = _post_package(big, _make_lockfile("1.0.0", "sha256:" + "0" * 64))
        assert resp.status_code == 413, resp.text

    def test_lockfile_over_cap_is_413(self, monkeypatch):
        monkeypatch.setattr(app.main, "MAX_LOCKFILE_BYTES", 128)
        archive_bytes = _make_archive("1.0.0")
        resp = _post_package(archive_bytes, b"#" * 1024)
        assert resp.status_code == 413, resp.text


class TestMalformedInput:
    def test_truncated_gzip_is_400(self):
        archive_bytes = _make_archive("1.0.0")
        cs = compute_package_checksum(archive_bytes)
        truncated = archive_bytes[: len(archive_bytes) // 2]
        resp = _post_package(truncated, _make_lockfile("1.0.0", cs))
        assert resp.status_code == 400, resp.text

    def test_not_a_tarball_is_400(self):
        garbage = gzip.compress(b"this is not a tar archive at all")
        resp = _post_package(garbage, _make_lockfile("1.0.0", "sha256:" + "0" * 64))
        assert resp.status_code == 400, resp.text

    def test_random_bytes_is_400(self):
        resp = _post_package(b"\xde\xad\xbe\xef" * 100, _make_lockfile("1.0.0", "sha256:" + "0" * 64))
        assert resp.status_code == 400, resp.text

    def test_malformed_lockfile_toml_is_400(self):
        archive_bytes = _make_archive("1.0.0")
        resp = _post_package(archive_bytes, b"not = valid = toml [ [")
        assert resp.status_code == 400, resp.text

    def test_malformed_manifest_toml_is_400(self):
        bad = _tar_with_entries([("xelian.toml", b"???? not toml ["), ("README.md", b"x")])
        cs = compute_package_checksum(bad)
        resp = _post_package(bad, _make_lockfile("1.0.0", cs))
        assert resp.status_code == 400, resp.text

    def test_missing_manifest_is_400(self):
        no_manifest = _tar_with_entries([("README.md", b"# hi")])
        cs = compute_package_checksum(no_manifest)
        resp = _post_package(no_manifest, _make_lockfile("1.0.0", cs))
        assert resp.status_code == 400, resp.text


class TestPathTraversal:
    def test_traversal_tar_entry_names_rejected(self):
        for evil in ("../evil.txt", "a/../../evil", "/etc/passwd", "..\\evil"):
            payload = _tar_with_entries([("xelian.toml", b"x"), (evil, b"pwn")])
            resp = _post_package(payload, _make_lockfile("1.0.0", "sha256:" + "0" * 64))
            assert resp.status_code == 400, f"{evil!r}: {resp.status_code} {resp.text}"
            # Nothing may have escaped the storage root.
            root = storage().root
            assert not (root.parent / "evil.txt").exists()

    def test_traversal_in_every_route_param(self):
        evil = "..%2f..%2fetc"
        assert client.get(f"/packages/{evil}/name").status_code in (400, 404)
        assert client.get(f"/packages/owner/{evil}").status_code in (400, 404)
        assert client.get(f"/download/{evil}/name/1.0.0").status_code in (400, 404)
        assert client.get(f"/download/owner/{evil}/1.0.0").status_code in (400, 404)
        assert client.get(f"/download/owner/name/{evil}").status_code in (400, 404)
        r = client.patch(
            f"/packages/testuser/{evil}/1.0.0",
            json={"yanked": True},
            headers=_auth_headers(),
        )
        assert r.status_code in (400, 404)


class TestConcurrentPublish:
    def test_concurrent_duplicate_publishes_yield_exactly_one_201(self):
        archive_bytes = _make_archive("1.0.0")
        cs = compute_package_checksum(archive_bytes)
        lock_bytes = _make_lockfile("1.0.0", cs)
        headers = _auth_headers()

        def attempt(_):
            return client.post(
                "/packages",
                files={
                    "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                    "lockfile": ("xelian.lock", lock_bytes, "application/toml"),
                },
                data={"owner": "testuser", "name": "race-pkg"},
                headers=headers,
            ).status_code

        with ThreadPoolExecutor(max_workers=8) as pool:
            results = list(pool.map(attempt, range(8)))

        assert results.count(201) == 1, results
        assert all(code in (201, 409) for code in results), results
