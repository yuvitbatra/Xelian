#!/usr/bin/env python3
"""Regenerate the H-212 golden checksum fixture (tests/golden/).

Deterministic: fixed mtimes/uids and gzip mtime=0, so the archive bytes are
stable across regenerations. The expected checksum is computed with the
registry's implementation; the Rust test asserts its own implementation
produces the same value from the same archive.
"""
import gzip
import io
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "registry"))
from app.main import compute_package_checksum  # noqa: E402

FILES = [
    ("xelian.toml", b'name = "golden"\nversion = "1.0.0"\n'),
    ("README.md", b"# golden fixture\n"),
    ("src/main.py", b'print("golden")\n'),
    # Sorting is by raw byte order of the path: "Zed.txt" (Z=0x5a) sorts
    # before "a.txt" (a=0x61) — this entry exists to pin that rule.
    ("Zed.txt", b"byte-order sort check\n"),
    ("xelian.lock", b'package-checksum = "excluded-from-digest"\n'),
]

buf = io.BytesIO()
with tarfile.open(fileobj=buf, mode="w") as tf:
    for name, data in FILES:
        info = tarfile.TarInfo(name)
        info.size = len(data)
        info.mtime = 0
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        tf.addfile(info, io.BytesIO(data))

archive = gzip.compress(buf.getvalue(), mtime=0)
out = ROOT / "tests" / "golden"
out.mkdir(parents=True, exist_ok=True)
(out / "fixture.xelian").write_bytes(archive)
checksum = compute_package_checksum(archive)
(out / "expected-checksum.txt").write_text(checksum + "\n")
print(f"wrote fixture.xelian ({len(archive)} bytes)")
print(f"expected checksum: {checksum}")
