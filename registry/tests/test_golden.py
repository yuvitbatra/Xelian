"""H-212 — cross-implementation golden fixture: the same archive + expected
digest are asserted by the Rust tests (lockfile.rs). Any drift in either
implementation fails CI. Regenerate with scripts/make_golden_fixture.py."""

from pathlib import Path

from app.main import compute_package_checksum

GOLDEN = Path(__file__).resolve().parents[2] / "tests" / "golden"


def test_golden_fixture_checksum_matches_rust_implementation():
    archive_bytes = (GOLDEN / "fixture.xelian").read_bytes()
    expected = (GOLDEN / "expected-checksum.txt").read_text().strip()
    assert compute_package_checksum(archive_bytes) == expected
