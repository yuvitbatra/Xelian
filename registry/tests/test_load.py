"""H-216 — load sanity: a burst of concurrent downloads + metadata reads
completes with zero 5xx. Guards the streaming/serving paths against
regression."""

from concurrent.futures import ThreadPoolExecutor

from tests.test_api import (
    _auth_headers,
    _make_archive,
    _make_lockfile,
    client,
)

from app.main import compute_package_checksum


def _publish(name: str, payload_kb: int = 256):
    archive_bytes = _make_archive("1.0.0", name=name)
    cs = compute_package_checksum(archive_bytes)
    resp = client.post(
        "/packages",
        files={
            "archive": ("pkg.xelian", archive_bytes, "application/octet-stream"),
            "lockfile": ("xelian.lock", _make_lockfile("1.0.0", cs), "application/toml"),
        },
        data={"owner": "testuser", "name": name},
        headers=_auth_headers(),
    )
    assert resp.status_code == 201, resp.text
    return archive_bytes


def test_burst_of_concurrent_reads_has_zero_5xx():
    archive_bytes = _publish("load-pkg")

    def hit(i: int) -> int:
        if i % 2 == 0:
            r = client.get("/download/testuser/load-pkg/1.0.0")
            if r.status_code == 200:
                assert r.content == archive_bytes
        else:
            r = client.get("/packages/testuser/load-pkg")
            if r.status_code == 200:
                assert r.json()["latest_version"] == "1.0.0"
        return r.status_code

    with ThreadPoolExecutor(max_workers=16) as pool:
        codes = list(pool.map(hit, range(50)))

    assert all(code == 200 for code in codes), codes
    listing = client.get("/packages")
    assert listing.status_code == 200
