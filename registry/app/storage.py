"""Archive byte storage (H-220/H-222/H-225): disk locally, Cloudflare R2 in
production. Metadata lives in Postgres (db.py) — archives are NEVER stored in
the database.

Backend selection is by environment:
- XELIAN_R2_BUCKET + XELIAN_R2_ENDPOINT + XELIAN_R2_ACCESS_KEY_ID +
  XELIAN_R2_SECRET_ACCESS_KEY  -> R2 (S3-compatible; free hosts have
  ephemeral disks, so object storage is REQUIRED in production or published
  packages vanish on redeploy)
- otherwise -> local disk under XELIAN_REGISTRY_ROOT (~/.xelian-registry)

Both backends stream downloads in chunks (H-222) — archives are never
buffered whole on the serving path.
"""

import hashlib
import os
from pathlib import Path
from typing import Iterator, Optional

CHUNK = 1024 * 1024


class DiskStorage:
    """Filesystem-backed archive storage for local development."""

    def __init__(self, root: Path):
        self.root = root.resolve()
        self.root.mkdir(parents=True, exist_ok=True)

    def _version_dir(self, owner: str, name: str, version: str) -> Path:
        return self.root / "packages" / owner / name / version

    def save(
        self, owner: str, name: str, version: str, archive: bytes, lockfile: bytes
    ) -> str:
        vdir = self._version_dir(owner, name, version)
        vdir.mkdir(parents=True, exist_ok=True)
        (vdir / "archive.xelian").write_bytes(archive)
        (vdir / "xelian.lock").write_bytes(lockfile)
        return hashlib.sha256(archive).hexdigest()

    def exists(self, owner: str, name: str, version: str) -> bool:
        return (self._version_dir(owner, name, version) / "archive.xelian").is_file()

    def stream(
        self, owner: str, name: str, version: str
    ) -> Optional[tuple[Iterator[bytes], int]]:
        path = self._version_dir(owner, name, version) / "archive.xelian"
        if not path.is_file():
            return None
        size = path.stat().st_size

        def chunks() -> Iterator[bytes]:
            with open(path, "rb") as f:
                while block := f.read(CHUNK):
                    yield block

        return chunks(), size


class R2Storage:
    """Cloudflare R2 (S3-compatible) archive storage for production."""

    def __init__(self, bucket: str, endpoint: str, access_key: str, secret_key: str):
        import boto3
        from botocore.config import Config

        self.bucket = bucket
        self.client = boto3.client(
            "s3",
            endpoint_url=endpoint,
            aws_access_key_id=access_key,
            aws_secret_access_key=secret_key,
            config=Config(connect_timeout=10, read_timeout=60, retries={"max_attempts": 3}),
        )

    def _key(self, owner: str, name: str, version: str, filename: str) -> str:
        return f"packages/{owner}/{name}/{version}/{filename}"

    def save(
        self, owner: str, name: str, version: str, archive: bytes, lockfile: bytes
    ) -> str:
        self.client.put_object(
            Bucket=self.bucket,
            Key=self._key(owner, name, version, "archive.xelian"),
            Body=archive,
        )
        self.client.put_object(
            Bucket=self.bucket,
            Key=self._key(owner, name, version, "xelian.lock"),
            Body=lockfile,
        )
        return hashlib.sha256(archive).hexdigest()

    def exists(self, owner: str, name: str, version: str) -> bool:
        try:
            self.client.head_object(
                Bucket=self.bucket,
                Key=self._key(owner, name, version, "archive.xelian"),
            )
            return True
        except self.client.exceptions.ClientError:
            return False

    def stream(
        self, owner: str, name: str, version: str
    ) -> Optional[tuple[Iterator[bytes], int]]:
        try:
            obj = self.client.get_object(
                Bucket=self.bucket,
                Key=self._key(owner, name, version, "archive.xelian"),
            )
        except self.client.exceptions.ClientError:
            return None
        size = obj["ContentLength"]
        body = obj["Body"]

        def chunks() -> Iterator[bytes]:
            while block := body.read(CHUNK):
                yield block

        return chunks(), size


def validate_r2_credentials(access_key: str, secret_key: str) -> list[str]:
    """Return a list of human-readable problems with the R2 credentials, empty
    if they look well-formed.

    Cloudflare R2 access key IDs are 32 hex chars and secrets are 64. A value
    truncated during a dashboard copy-paste (e.g. 31 chars) is otherwise only
    discovered as a cryptic `PutObject ... InvalidArgument` 500 on *every*
    publish — this makes the misconfiguration obvious at startup instead."""
    problems = []
    if len(access_key) != 32:
        problems.append(
            f"XELIAN_R2_ACCESS_KEY_ID is {len(access_key)} characters; R2 keys are 32 "
            "(a truncated copy-paste is the usual cause)"
        )
    if len(secret_key) != 64:
        problems.append(
            f"XELIAN_R2_SECRET_ACCESS_KEY is {len(secret_key)} characters; R2 secrets are 64"
        )
    return problems


def from_env():
    bucket = os.environ.get("XELIAN_R2_BUCKET")
    endpoint = os.environ.get("XELIAN_R2_ENDPOINT")
    access_key = os.environ.get("XELIAN_R2_ACCESS_KEY_ID")
    secret_key = os.environ.get("XELIAN_R2_SECRET_ACCESS_KEY")
    if bucket and endpoint and access_key and secret_key:
        # Fail loudly at startup on a malformed credential rather than 500 on
        # every publish. Print to stderr so it lands in the platform's logs.
        problems = validate_r2_credentials(access_key, secret_key)
        if problems:
            import sys

            for p in problems:
                print(f"WARNING: R2 storage misconfigured — {p}", file=sys.stderr)
            print(
                "WARNING: publishes will fail until this is corrected in the "
                "registry's environment variables.",
                file=sys.stderr,
            )
        return R2Storage(bucket, endpoint, access_key, secret_key)
    root = Path(
        os.environ.get("XELIAN_REGISTRY_ROOT", Path.home() / ".xelian-registry")
    )
    return DiskStorage(root)
