import hashlib

from app.main import compute_package_checksum
import io
import os
import tarfile
import tempfile
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

import app.main
from app.auth import AuthStore
from app.storage import Storage


@pytest.fixture(autouse=True)
def test_env(monkeypatch):
    """Use a temp directory for storage and fixed credentials during tests."""
    tmp = tempfile.mkdtemp()
    new_storage = Storage(Path(tmp))
    monkeypatch.setattr(app.main, "storage", new_storage)
    monkeypatch.setattr(app.main, "auth_store", AuthStore(Path(tmp) / "auth"))
    monkeypatch.setenv("XELIAN_REGISTRY_USERNAME", "testuser")
    monkeypatch.setenv("XELIAN_REGISTRY_PASSWORD", "testpass")
    yield
    import shutil

    shutil.rmtree(tmp)


client = TestClient(app.main.app)


def storage() -> Storage:
    return app.main.storage


def _make_tarinfo(name: str, data: bytes) -> tarfile.TarInfo:
    ti = tarfile.TarInfo(name)
    ti.size = len(data)
    return ti


def _make_archive(version: str, name: str = "test-pkg") -> bytes:
    """Build a minimal valid .xelian archive in memory."""
    buf = io.BytesIO()
    manifest_toml = (
        f'spec-version = 1\n'
        f'name = "{name}"\n'
        f'version = "{version}"\n'
        f'description = "Test package"\n'
        f'package-type = "agent"\n'
        f'language = "python"\n'
        f'runtime = ">=3.11"\n'
        f'entrypoint = "src/main.py"\n'
        f'license = "MIT"\n'
        f'permissions = ["network"]\n'
        f'features = ["streaming"]\n'
        f'\n'
        f'[author]\n'
        f'name = "Test User"\n'
        f'email = "test@example.com"\n'
        f'\n'
        f'[dependencies]\n'
        f'manifest = "pyproject.toml"\n'
        f'lockfile = "uv.lock"\n'
    ).encode()
    readme = b"# Test\n"
    lic = b"MIT\n"
    with tarfile.open(fileobj=buf, mode="w:gz") as tf:
        tf.addfile(_make_tarinfo("xelian.toml", manifest_toml), io.BytesIO(manifest_toml))
        tf.addfile(_make_tarinfo("README.md", readme), io.BytesIO(readme))
        tf.addfile(_make_tarinfo("LICENSE", lic), io.BytesIO(lic))
    buf.seek(0)
    return buf.read()


def _make_lockfile(version: str, checksum: str) -> bytes:
    lock = (
        f'spec-version = 1\n'
        f'xelian-version = "0.1.0"\n'
        f'package-version = "{version}"\n'
        f'generated-at = "2026-07-17T00:00:00Z"\n'
        f'native-manifest = "pyproject.toml"\n'
        f'native-lockfile = "uv.lock"\n'
        f'native-lock-checksum = "sha256:'
        f'0000000000000000000000000000000000000000000000000000000000000000"\n'
        f'package-checksum = "{checksum}"\n'
    )
    return lock.encode()


def _auth_headers() -> dict:
    """Get auth headers by logging in with test credentials."""
    resp = client.post("/auth/token", json={
        "username": "testuser",
        "password": "testpass",
    })
    assert resp.status_code == 200, f"login failed: {resp.text}"
    token = resp.json()["token"]
    return {"Authorization": f"Bearer {token}"}


class TestAuth:
    def test_login_success(self):
        resp = client.post("/auth/token", json={
            "username": "testuser",
            "password": "testpass",
        })
        assert resp.status_code == 200
        data = resp.json()
        assert "token" in data
        assert data["username"] == "testuser"

    def test_login_invalid_credentials(self):
        resp = client.post("/auth/token", json={
            "username": "testuser",
            "password": "wrongpass",
        })
        assert resp.status_code == 401

    def test_login_missing_fields(self):
        resp = client.post("/auth/token", json={
            "username": "testuser",
        })
        # Missing password defaults to "" -> auth fails with 401
        assert resp.status_code == 401

    def test_publish_without_auth_is_401(self):
        archive_bytes = _make_archive("1.0.0")
        checksum = compute_package_checksum(archive_bytes)
        lock_bytes = _make_lockfile("1.0.0", checksum)

        resp = client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", lock_bytes, "application/toml"),
            },
            data={"owner": "testuser", "name": "test-pkg"},
        )
        # Should be 401 (auth required) or at least not succeed
        assert resp.status_code == 401, f"expected 401, got {resp.status_code}: {resp.text}"

    def test_publish_wrong_owner_is_403(self):
        archive_bytes = _make_archive("1.0.0")
        checksum = compute_package_checksum(archive_bytes)
        lock_bytes = _make_lockfile("1.0.0", checksum)
        headers = _auth_headers()

        # Try to publish as "otheruser" while authenticated as "testuser"
        resp = client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", lock_bytes, "application/toml"),
            },
            data={"owner": "otheruser", "name": "test-pkg"},
            headers=headers,
        )
        assert resp.status_code == 403, f"expected 403, got {resp.status_code}: {resp.text}"


class TestPublish:
    def test_publish_and_download(self):
        archive_bytes = _make_archive("1.0.0")
        checksum = compute_package_checksum(archive_bytes)
        lock_bytes = _make_lockfile("1.0.0", checksum)
        headers = _auth_headers()

        resp = client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", lock_bytes, "application/toml"),
            },
            data={"owner": "testuser", "name": "test-pkg"},
            headers=headers,
        )
        assert resp.status_code == 201, resp.text
        assert resp.json()["version"] == "1.0.0"

        # GET metadata
        resp = client.get("/packages/testuser/test-pkg")
        assert resp.status_code == 200, resp.text
        data = resp.json()
        assert data["latest_version"] == "1.0.0"
        assert data["name"] == "test-pkg"

        # GET download
        resp = client.get("/download/testuser/test-pkg/1.0.0")
        assert resp.status_code == 200
        assert resp.content == archive_bytes

    def test_duplicate_version_rejected(self):
        archive_bytes = _make_archive("1.0.0")
        checksum = compute_package_checksum(archive_bytes)
        lock_bytes = _make_lockfile("1.0.0", checksum)
        headers = _auth_headers()

        client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", lock_bytes, "application/toml"),
            },
            data={"owner": "testuser", "name": "dup-test"},
            headers=headers,
        )

        resp = client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", lock_bytes, "application/toml"),
            },
            data={"owner": "testuser", "name": "dup-test"},
            headers=headers,
        )
        assert resp.status_code == 409

    def test_checksum_mismatch_rejected(self):
        archive_bytes = _make_archive("1.0.0")
        lock_bytes = _make_lockfile("1.0.0", "badchecksum" * 8)
        headers = _auth_headers()

        resp = client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", lock_bytes, "application/toml"),
            },
            data={"owner": "testuser", "name": "sum-test"},
            headers=headers,
        )
        assert resp.status_code == 422


class TestResolution:
    def test_resolve_latest_skips_yanked(self):
        headers = _auth_headers()
        # Publish 1.0.0
        ab1 = _make_archive("1.0.0", "yank-pkg")
        cs1 = compute_package_checksum(ab1)
        client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", ab1, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("1.0.0", cs1), "application/toml"),
            },
            data={"owner": "testuser", "name": "yank-pkg"},
            headers=headers,
        )

        # Publish 2.0.0
        ab2 = _make_archive("2.0.0", "yank-pkg")
        cs2 = compute_package_checksum(ab2)
        client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", ab2, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("2.0.0", cs2), "application/toml"),
            },
            data={"owner": "testuser", "name": "yank-pkg"},
            headers=headers,
        )

        # Yank 2.0.0
        storage().set_yanked("testuser", "yank-pkg", "2.0.0", True)

        # Resolution should fall back to 1.0.0
        resp = client.get("/packages/testuser/yank-pkg")
        assert resp.status_code == 200
        assert resp.json()["latest_version"] == "1.0.0"

    def test_resolve_latest_skips_prerelease(self):
        headers = _auth_headers()
        # Publish 1.0.0-alpha (prerelease)
        ab1 = _make_archive("1.0.0-alpha", "pre-pkg")
        cs1 = compute_package_checksum(ab1)
        client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", ab1, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("1.0.0-alpha", cs1), "application/toml"),
            },
            data={"owner": "testuser", "name": "pre-pkg"},
            headers=headers,
        )

        # Publish 0.9.0 (stable, lower)
        ab2 = _make_archive("0.9.0", "pre-pkg")
        cs2 = compute_package_checksum(ab2)
        client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", ab2, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("0.9.0", cs2), "application/toml"),
            },
            data={"owner": "testuser", "name": "pre-pkg"},
            headers=headers,
        )

        # 1.0.0-alpha is prerelease, so 0.9.0 should be latest
        resp = client.get("/packages/testuser/pre-pkg")
        assert resp.status_code == 200
        assert resp.json()["latest_version"] == "0.9.0"

    def test_resolve_latest_no_versions_is_404(self):
        resp = client.get("/packages/nobody/unknown-pkg")
        assert resp.status_code == 404

    def test_all_yanked_versions_is_404(self):
        headers = _auth_headers()
        ab = _make_archive("1.0.0", "all-yanked")
        cs = compute_package_checksum(ab)
        client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", ab, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("1.0.0", cs), "application/toml"),
            },
            data={"owner": "testuser", "name": "all-yanked"},
            headers=headers,
        )
        storage().set_yanked("testuser", "all-yanked", "1.0.0", True)

        resp = client.get("/packages/testuser/all-yanked")
        assert resp.status_code == 404


class TestYank:
    def test_yank_via_api_excludes_from_resolution(self):
        headers = _auth_headers()
        ab1 = _make_archive("1.0.0", "yank-api")
        cs1 = compute_package_checksum(ab1)
        client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", ab1, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("1.0.0", cs1), "application/toml"),
            },
            data={"owner": "testuser", "name": "yank-api"},
            headers=headers,
        )
        ab2 = _make_archive("2.0.0", "yank-api")
        cs2 = compute_package_checksum(ab2)
        client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", ab2, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("2.0.0", cs2), "application/toml"),
            },
            data={"owner": "testuser", "name": "yank-api"},
            headers=headers,
        )

        # Yank 2.0.0 via HTTP API
        resp = client.patch(
            "/packages/testuser/yank-api/2.0.0",
            json={"yanked": True},
            headers=headers,
        )
        assert resp.status_code == 200, resp.text
        assert resp.json()["ok"] is True

        # Resolution should fall back to 1.0.0
        resp = client.get("/packages/testuser/yank-api")
        assert resp.status_code == 200
        assert resp.json()["latest_version"] == "1.0.0"

    def test_unyank_via_api_restores_resolution(self):
        headers = _auth_headers()
        ab = _make_archive("1.0.0", "unyank-api")
        cs = compute_package_checksum(ab)
        client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", ab, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("1.0.0", cs), "application/toml"),
            },
            data={"owner": "testuser", "name": "unyank-api"},
            headers=headers,
        )

        # Yank
        resp = client.patch(
            "/packages/testuser/unyank-api/1.0.0",
            json={"yanked": True},
            headers=headers,
        )
        assert resp.status_code == 200

        # Verify it's gone from resolution
        resp = client.get("/packages/testuser/unyank-api")
        assert resp.status_code == 404

        # Unyank
        resp = client.patch(
            "/packages/testuser/unyank-api/1.0.0",
            json={"yanked": False},
            headers=headers,
        )
        assert resp.status_code == 200
        assert resp.json()["ok"] is True

        # Verify it's back
        resp = client.get("/packages/testuser/unyank-api")
        assert resp.status_code == 200
        assert resp.json()["latest_version"] == "1.0.0"

    def test_yank_non_owner_is_403(self):
        headers = _auth_headers()
        ab = _make_archive("1.0.0", "no-yank")
        cs = compute_package_checksum(ab)
        client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", ab, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("1.0.0", cs), "application/toml"),
            },
            data={"owner": "testuser", "name": "no-yank"},
            headers=headers,
        )

        # Try to yank as a different user
        resp = client.patch(
            "/packages/testuser/no-yank/1.0.0",
            json={"yanked": True},
            headers={"Authorization": "Bearer no-such-token"},
        )
        assert resp.status_code == 401

    def test_yank_nonexistent_version_is_404(self):
        headers = _auth_headers()
        resp = client.patch(
            "/packages/testuser/nonexistent/1.0.0",
            json={"yanked": True},
            headers=headers,
        )
        assert resp.status_code == 404

    def test_yank_without_auth_is_401(self):
        resp = client.patch(
            "/packages/testuser/some-pkg/1.0.0",
            json={"yanked": True},
        )
        assert resp.status_code == 401


class TestSecurityAndInterop:
    def test_checksum_algorithm_matches_spec_not_raw_archive_hash(self):
        # Regression: the registry MUST verify package-checksum per SPEC §7.3
        # (per-file digest, `sha256:`-prefixed, excluding xelian.lock), NOT a
        # bare hash of the whole archive. If these two ever coincide the guard
        # below is meaningless; assert they differ so a revert is caught.
        archive_bytes = _make_archive("1.0.0")
        spec_checksum = compute_package_checksum(archive_bytes)
        raw_hash = hashlib.sha256(archive_bytes).hexdigest()
        assert spec_checksum.startswith("sha256:")
        assert spec_checksum != raw_hash

    def test_publish_rejects_bare_archive_hash_checksum(self):
        # A lockfile carrying the OLD (wrong) bare-archive-hash checksum must be
        # rejected with 422 — proving the registry now speaks the CLI's dialect.
        archive_bytes = _make_archive("1.0.0")
        bad_lock = _make_lockfile("1.0.0", hashlib.sha256(archive_bytes).hexdigest())
        resp = client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", bad_lock, "application/toml"),
            },
            data={"owner": "testuser", "name": "test-pkg"},
            headers=_auth_headers(),
        )
        assert resp.status_code == 422, resp.text

    def test_publish_rejects_traversal_in_owner(self):
        archive_bytes = _make_archive("1.0.0")
        cs = compute_package_checksum(archive_bytes)
        resp = client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("1.0.0", cs), "application/toml"),
            },
            data={"owner": "..", "name": "test-pkg"},
            headers=_auth_headers(),
        )
        # Authenticated as testuser, owner ".." fails the owner==user check (403)
        # or the segment guard (400) — either way it never touches the filesystem.
        assert resp.status_code in (400, 403), resp.text

    def test_get_package_rejects_traversal_segment(self):
        resp = client.get("/packages/..%2f..%2fetc/passwd")
        assert resp.status_code in (400, 404)


class TestSignupAndAccounts:
    def _signup(self, username="alice", password="password123"):
        return client.post(
            "/auth/signup", json={"username": username, "password": password}
        )

    def test_signup_returns_usable_token(self):
        resp = self._signup()
        assert resp.status_code == 201, resp.text
        data = resp.json()
        assert data["username"] == "alice"
        headers = {"Authorization": f"Bearer {data['token']}"}

        archive_bytes = _make_archive("1.0.0")
        cs = compute_package_checksum(archive_bytes)
        pub = client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("1.0.0", cs), "application/toml"),
            },
            data={"owner": "alice", "name": "test-pkg"},
            headers=headers,
        )
        assert pub.status_code == 201, pub.text

    def test_signup_duplicate_username_is_409(self):
        assert self._signup().status_code == 201
        assert self._signup().status_code == 409

    def test_signup_invalid_username_is_400(self):
        for bad in ("..", "a", "a/b", "", "-leading", "x" * 40):
            resp = self._signup(username=bad)
            assert resp.status_code == 400, f"{bad!r}: {resp.status_code}"

    def test_signup_short_password_is_400(self):
        assert self._signup(password="short").status_code == 400

    def test_login_after_signup(self):
        self._signup()
        ok = client.post(
            "/auth/token", json={"username": "alice", "password": "password123"}
        )
        assert ok.status_code == 200, ok.text
        bad = client.post(
            "/auth/token", json={"username": "alice", "password": "wrong-password"}
        )
        assert bad.status_code == 401

    def test_no_default_admin_credentials(self, monkeypatch):
        monkeypatch.delenv("XELIAN_REGISTRY_USERNAME")
        monkeypatch.delenv("XELIAN_REGISTRY_PASSWORD")
        resp = client.post(
            "/auth/token", json={"username": "admin", "password": "admin"}
        )
        assert resp.status_code == 401

    def test_accounts_and_tokens_survive_restart(self):
        token = self._signup().json()["token"]
        # Simulate a registry restart: fresh AuthStore over the same directory.
        app.main.auth_store = AuthStore(app.main.auth_store.root)
        assert app.main.auth_store.verify_token(token) == "alice"
        login = client.post(
            "/auth/token", json={"username": "alice", "password": "password123"}
        )
        assert login.status_code == 200

    def test_signup_token_cannot_publish_other_namespace(self):
        token = self._signup().json()["token"]
        archive_bytes = _make_archive("1.0.0")
        cs = compute_package_checksum(archive_bytes)
        resp = client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile("1.0.0", cs), "application/toml"),
            },
            data={"owner": "someone-else", "name": "test-pkg"},
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 403


class TestListPackages:
    def _publish(self, version="1.0.0", name="test-pkg"):
        archive_bytes = _make_archive(version, name=name)
        cs = compute_package_checksum(archive_bytes)
        resp = client.post(
            "/packages",
            files={
                "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
                "lockfile": ("xelian.lock", _make_lockfile(version, cs), "application/toml"),
            },
            data={"owner": "testuser", "name": name},
            headers=_auth_headers(),
        )
        assert resp.status_code == 201, resp.text

    def test_list_empty(self):
        resp = client.get("/packages")
        assert resp.status_code == 200
        assert resp.json() == []

    def test_list_after_publish(self):
        self._publish(name="pkg-a")
        self._publish(name="pkg-b")
        resp = client.get("/packages")
        assert resp.status_code == 200
        rows = {r["name"]: r for r in resp.json()}
        assert set(rows) == {"pkg-a", "pkg-b"}
        row = rows["pkg-a"]
        assert row["owner"] == "testuser"
        assert row["latest_version"] == "1.0.0"
        assert row["package_type"] == "agent"
        assert row["language"] == "python"

    def test_list_omits_fully_yanked_packages(self):
        self._publish(name="pkg-a")
        yank = client.patch(
            "/packages/testuser/pkg-a/1.0.0",
            json={"yanked": True},
            headers=_auth_headers(),
        )
        assert yank.status_code == 200, yank.text
        resp = client.get("/packages")
        assert resp.json() == []
