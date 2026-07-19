import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

from .models import PackageMetadata, VersionRecord


class Storage:
    """Filesystem-backed storage for Xelian registry packages."""

    def __init__(self, root: Path):
        self.root = root.resolve()
        self.root.mkdir(parents=True, exist_ok=True)

    def _pkg_dir(self, owner: str, name: str) -> Path:
        # Pure path computation — creating directories here would let
        # unauthenticated GET routes (list_versions/archive_bytes) materialize
        # arbitrary attacker-named directory trees on every 404 lookup. Only the
        # write path (save_*) creates directories, via `_ensure_version_dir`.
        return self.root / "packages" / owner / name

    def _version_dir(self, owner: str, name: str, version: str) -> Path:
        return self._pkg_dir(owner, name) / version

    def _ensure_version_dir(self, owner: str, name: str, version: str) -> Path:
        d = self._version_dir(owner, name, version)
        d.mkdir(parents=True, exist_ok=True)
        return d

    def version_exists(self, owner: str, name: str, version: str) -> bool:
        return self._version_dir(owner, name, version).is_dir()

    def save_archive(self, owner: str, name: str, version: str, archive_bytes: bytes) -> str:
        checksum = hashlib.sha256(archive_bytes).hexdigest()
        vdir = self._ensure_version_dir(owner, name, version)
        (vdir / "archive.xelian").write_bytes(archive_bytes)
        return checksum

    def save_lockfile(self, owner: str, name: str, version: str, lock_bytes: bytes):
        vdir = self._ensure_version_dir(owner, name, version)
        (vdir / "xelian.lock").write_bytes(lock_bytes)

    def save_metadata(self, owner: str, name: str, version: str, meta: PackageMetadata):
        vdir = self._ensure_version_dir(owner, name, version)
        (vdir / "metadata.json").write_text(meta.model_dump_json(indent=2))

    def save_readme(self, owner: str, name: str, version: str, readme: str):
        vdir = self._ensure_version_dir(owner, name, version)
        (vdir / "README.md").write_text(readme)

    def list_versions(self, owner: str, name: str) -> list[VersionRecord]:
        pkg_dir = self._pkg_dir(owner, name)
        if not pkg_dir.is_dir():
            return []
        versions = []
        try:
            entries = sorted(pkg_dir.iterdir())
        except OSError:
            return []
        for entry in entries:
            if not entry.is_dir():
                continue
            meta_file = entry / "metadata.json"
            lock_file = entry / "xelian.lock"
            archive_file = entry / "archive.xelian"
            if not (meta_file.is_file() and lock_file.is_file() and archive_file.is_file()):
                continue
            try:
                meta_dict = json.loads(meta_file.read_text())
            except (json.JSONDecodeError, OSError):
                continue
            try:
                published = datetime.fromisoformat(meta_dict.get("published_at", ""))
            except (ValueError, TypeError):
                published = datetime.now(timezone.utc)
            versions.append(VersionRecord(
                version=meta_dict.get("version", entry.name),
                checksum=meta_dict.get("checksum", ""),
                published_at=published,
                yanked=meta_dict.get("yanked", False),
            ))
        return versions

    def load_metadata(self, owner: str, name: str, version: str) -> Optional[PackageMetadata]:
        vdir = self._version_dir(owner, name, version)
        meta_file = vdir / "metadata.json"
        if not meta_file.is_file():
            return None
        data = json.loads(meta_file.read_text())
        return PackageMetadata(**data)

    def load_readme(self, owner: str, name: str, version: str) -> Optional[str]:
        readme_file = self._version_dir(owner, name, version) / "README.md"
        if readme_file.is_file():
            return readme_file.read_text()
        return None

    def archive_path(self, owner: str, name: str, version: str) -> Optional[Path]:
        p = self._version_dir(owner, name, version) / "archive.xelian"
        return p if p.is_file() else None

    def archive_bytes(self, owner: str, name: str, version: str) -> Optional[bytes]:
        ap = self.archive_path(owner, name, version)
        return ap.read_bytes() if ap else None

    def set_yanked(self, owner: str, name: str, version: str, yanked: bool) -> bool:
        vdir = self._version_dir(owner, name, version)
        meta_file = vdir / "metadata.json"
        if not meta_file.is_file():
            return False
        data = json.loads(meta_file.read_text())
        data["yanked"] = yanked
        data["published_at"] = data.get(
            "published_at", datetime.now(timezone.utc).isoformat()
        )
        meta_file.write_text(json.dumps(data, indent=2))
        return True
