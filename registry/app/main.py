import hashlib
import io
import os
import tarfile
import time
from collections import defaultdict, deque
from threading import Lock

from fastapi import Depends, FastAPI, File, Form, HTTPException, Request, UploadFile
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import selectinload

from . import auth, db
from . import storage as storage_backend
from .models import (
    AuthorInfo,
    LoginResponse,
    PackageInfo,
    PackageSummary,
    PublishResponse,
    VersionRecord,
)
from .resolution import resolve_latest

app = FastAPI(title="Xelian Registry", version="0.2.0")

# The website is a browser client of this same public API (§14.9). Bearer
# tokens (no cookies) make a permissive CORS policy safe here.
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

archive_storage = storage_backend.from_env()


@app.on_event("startup")
def _startup():
    db.create_all()


security = HTTPBearer(auto_error=False)


def require_auth(credentials: HTTPAuthorizationCredentials = Depends(security)):
    """Require a valid auth token. Returns the authenticated username."""
    if credentials is None:
        raise HTTPException(401, detail="Authentication required")
    username = auth.verify_token(credentials.credentials)
    if username is None:
        raise HTTPException(401, detail="Invalid or expired token")
    return username


# --- Per-IP rate limiting (H-222): sliding window, in-memory. Fine for the
# single-instance free-tier deployment of record; env-tunable. ---


class RateLimiter:
    def __init__(self, limit: int, window_seconds: float):
        self.limit = limit
        self.window = window_seconds
        self._hits: dict[str, deque] = defaultdict(deque)
        self._lock = Lock()

    def allow(self, key: str) -> bool:
        now = time.monotonic()
        with self._lock:
            hits = self._hits[key]
            while hits and hits[0] < now - self.window:
                hits.popleft()
            if len(hits) >= self.limit:
                return False
            hits.append(now)
            return True


auth_limiter = RateLimiter(int(os.environ.get("XELIAN_RATE_LIMIT_AUTH", "30")), 60)
publish_limiter = RateLimiter(int(os.environ.get("XELIAN_RATE_LIMIT_PUBLISH", "60")), 3600)


def _client_ip(request: Request) -> str:
    fwd = request.headers.get("x-forwarded-for")
    if fwd:
        return fwd.split(",")[0].strip()
    return request.client.host if request.client else "unknown"


def rate_limit_auth(request: Request):
    if not auth_limiter.allow(_client_ip(request)):
        raise HTTPException(429, detail="too many auth attempts — try again in a minute")


def rate_limit_publish(request: Request):
    if not publish_limiter.allow(_client_ip(request)):
        raise HTTPException(429, detail="publish rate limit reached — try again later")


# A package/owner/version path segment must be a single safe filesystem
# component: no separators, no `.`/`..` traversal, no empty value. This mirrors
# the CLI's `is_safe_repo_component` (github.rs) and the §19.3 name charset.
_SAFE_SEGMENT = __import__("re").compile(r"^[A-Za-z0-9._+-]+$")


def _validate_segment(value: str, field: str) -> str:
    """Reject a path segment that could escape a storage root (§19.3)."""
    if not value or value in (".", "..") or not _SAFE_SEGMENT.match(value):
        raise HTTPException(400, detail=f"invalid {field}: {value!r}")
    return value


# --- Upload / decompression limits (H-214/H-222) ---
MAX_ARCHIVE_BYTES = int(os.environ.get("XELIAN_MAX_ARCHIVE_MB", "100")) * 1024 * 1024
MAX_LOCKFILE_BYTES = 1 * 1024 * 1024
MAX_UNCOMPRESSED_BYTES = int(os.environ.get("XELIAN_MAX_UNCOMPRESSED_MB", "500")) * 1024 * 1024
MAX_ARCHIVE_ENTRIES = 10_000


async def _read_capped(upload: UploadFile, cap: int, field: str) -> bytes:
    """Read an upload, rejecting it with 413 as soon as it exceeds `cap`."""
    chunks = []
    total = 0
    while True:
        chunk = await upload.read(1024 * 1024)
        if not chunk:
            break
        total += len(chunk)
        if total > cap:
            raise HTTPException(413, detail=f"{field} exceeds the {cap} byte limit")
        chunks.append(chunk)
    return b"".join(chunks)


def _safe_entry_name(name: str) -> bool:
    """True if a tar entry name stays inside the extraction root (§19.3)."""
    if name.startswith(("/", "\\")):
        return False
    trimmed = name.replace("\\", "/").rstrip("/")
    if not trimmed:
        return False
    return all(p not in ("", ".", "..") for p in trimmed.split("/"))


def _iter_archive_files(archive_bytes: bytes):
    """Yield (name, bytes) for file entries, enforcing bomb/traversal guards.

    Raises HTTPException 400 for malformed archives, traversal entry names,
    and 413 when decompressed size or entry count exceeds the caps — the
    registry must never crash or write outside its root on hostile input.
    """
    total = 0
    count = 0
    try:
        with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode="r:gz") as tf:
            for member in tf:
                count += 1
                if count > MAX_ARCHIVE_ENTRIES:
                    raise HTTPException(
                        413,
                        detail=f"archive has more than {MAX_ARCHIVE_ENTRIES} entries",
                    )
                if not _safe_entry_name(member.name):
                    raise HTTPException(
                        400,
                        detail=f"archive entry has unsafe path: {member.name!r}",
                    )
                if not member.isfile():
                    continue
                total += member.size
                if total > MAX_UNCOMPRESSED_BYTES:
                    raise HTTPException(
                        413,
                        detail="archive decompresses beyond the size limit",
                    )
                fobj = tf.extractfile(member)
                data = fobj.read(MAX_UNCOMPRESSED_BYTES - total + member.size + 1) if fobj else b""
                # A lying tar header (declared size < real size) cannot smuggle
                # extra bytes: total is re-checked against what was read.
                total = total - member.size + len(data)
                if total > MAX_UNCOMPRESSED_BYTES:
                    raise HTTPException(
                        413,
                        detail="archive decompresses beyond the size limit",
                    )
                yield member.name, data
    except HTTPException:
        raise
    except (tarfile.TarError, EOFError, OSError, ValueError) as e:
        raise HTTPException(400, detail=f"malformed archive: {e}")


def compute_package_checksum(archive_bytes: bytes) -> str:
    """Recompute `package-checksum` per SPEC §7.3 for interoperability with the
    CLI's `compute_package_checksum` (crates/xelian-core/src/lockfile.rs).

    The digest is taken over the archive's file entries — sorted by
    archive-relative path (byte order), **excluding `xelian.lock` itself** — as
    the SHA-256 of the concatenation of `<path>\\0sha256:<hex>\\n` for each
    remaining file, rendered `sha256:<hex>`. This is NOT a hash of the raw
    archive bytes, so it must match the CLI byte-for-byte or every real
    `xelian push` is rejected as a checksum mismatch.
    """
    entries: list[tuple[str, bytes]] = [
        (name, data)
        for name, data in _iter_archive_files(archive_bytes)
        if name != "xelian.lock"
    ]
    entries.sort(key=lambda e: e[0].encode("utf-8"))
    concat = bytearray()
    for path, contents in entries:
        file_hash = "sha256:" + hashlib.sha256(contents).hexdigest()
        concat += path.encode("utf-8")
        concat += b"\x00"
        concat += file_hash.encode("utf-8")
        concat += b"\n"
    return "sha256:" + hashlib.sha256(bytes(concat)).hexdigest()


def _extract_xelian_toml(archive_bytes: bytes) -> tuple[dict, str]:
    """Parse xelian.toml and README.md from .xelian archive bytes."""
    readme = ""
    manifest_bytes = None
    for name, data in _iter_archive_files(archive_bytes):
        if name == "xelian.toml":
            manifest_bytes = data
        elif name == "README.md":
            readme = data.decode("utf-8", errors="replace")
    if manifest_bytes is None:
        raise HTTPException(400, detail="archive missing xelian.toml")
    import tomllib

    try:
        return tomllib.loads(manifest_bytes.decode("utf-8")), readme
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as e:
        raise HTTPException(400, detail=f"malformed xelian.toml: {e}")


@app.get("/")
def root():
    """The registry is a JSON API — point humans at the website and docs."""
    return {
        "name": "Xelian Registry",
        "version": app.version,
        "hint": "This is the API the CLI and website talk to. Browse packages on the website (npm run dev in website/), or see interactive API docs at /docs.",
        "endpoints": {
            "health": "GET /health",
            "list_packages": "GET /packages",
            "search": "GET /search?q=",
            "catalog": "GET /catalog?q=&type=mcp|agent",
            "package_metadata": "GET /packages/{owner}/{name}",
            "download": "GET /download/{owner}/{name}/{version}",
            "signup": "POST /auth/signup",
            "login": "POST /auth/token",
            "revoke": "DELETE /auth/token (auth required)",
            "publish": "POST /packages (auth required)",
        },
    }


@app.get("/health")
def health():
    return {"ok": True}


# --- Catalog (discovery index) ---------------------------------------------
#
# The catalog is the *index* of permissively-licensed public repositories that
# run via `xelian add <url>` — Xelian links to and runs them under their own
# license, it does not host their code. It is a static, curated artifact
# (`catalog.json`, produced by scripts/harvest_catalog.py), so it is loaded
# once and served filtered. This is distinct from `/packages`, which lists
# archives actually published to this registry.

import json as _json
from functools import lru_cache
from pathlib import Path as _Path


@lru_cache(maxsize=1)
def _load_catalog() -> dict:
    # Ships next to the app in the image; falls back to the repo path in dev.
    for candidate in (
        _Path(__file__).resolve().parent.parent / "catalog.json",
        _Path(__file__).resolve().parents[2] / "registry" / "catalog.json",
    ):
        if candidate.is_file():
            with candidate.open() as f:
                return _json.load(f)
    return {"counts": {"total": 0, "mcp": 0, "agents": 0}, "packages": []}


@app.get("/catalog/{owner}/{name}")
def catalog_entry(owner: str, name: str):
    """Resolve a single catalog entry by `owner/name` (its GitHub full name).

    Lets `xelian run owner/name` fall back to the discovery index when no
    archive is published under that name: the CLI runs the returned GitHub
    `url` via its import path, so third-party projects run under their own
    license without the registry hosting their code."""
    full = f"{owner}/{name}".lower()
    for entry in _load_catalog()["packages"]:
        if entry.get("full_name", "").lower() == full:
            return entry
    raise HTTPException(404, detail=f"{owner}/{name} is not in the catalog")


@app.get("/catalog")
def catalog(
    q: str | None = None,
    type: str | None = None,
    limit: int = 60,
    offset: int = 0,
):
    """Browse the discovery index. `q` matches name/owner/description; `type`
    filters to `mcp` or `agent`; results are paginated (already star-ranked)."""
    data = _load_catalog()
    rows = data["packages"]

    if type in ("mcp", "agent"):
        rows = [r for r in rows if r.get("type") == type]
    if q:
        needle = q.lower()
        rows = [
            r
            for r in rows
            if needle in r.get("name", "").lower()
            or needle in r.get("owner", "").lower()
            or needle in (r.get("description") or "").lower()
        ]

    total = len(rows)
    page = rows[max(0, offset) : max(0, offset) + max(1, min(limit, 200))]
    return {
        "generated_at": data.get("generated_at"),
        "total": total,
        "counts": data.get("counts", {}),
        "packages": page,
    }


# --- Auth routes (§13.7, §14.4) ---


@app.post("/auth/token", dependencies=[Depends(rate_limit_auth)])
def login(body: dict):
    """Exchange credentials for an auth token."""
    username = body.get("username", "")
    password = body.get("password", "")
    if not auth.authenticate(username, password):
        raise HTTPException(401, detail="Invalid credentials")
    token = auth.create_token(username)
    return LoginResponse(token=token, username=username)


@app.post(
    "/auth/signup",
    response_model=LoginResponse,
    status_code=201,
    dependencies=[Depends(rate_limit_auth)],
)
def signup(body: dict):
    """Create an account; the username becomes the publish namespace (§14.4).

    Returns a fresh token so signup doubles as the first login.
    """
    username = body.get("username", "")
    password = body.get("password", "")
    if not auth.valid_username(username):
        raise HTTPException(
            400,
            detail=(
                "invalid username: 2-39 chars, letters/digits/._- only, "
                "must start with a letter or digit"
            ),
        )
    if len(password) < auth.MIN_PASSWORD_LENGTH:
        raise HTTPException(
            400,
            detail=f"password must be at least {auth.MIN_PASSWORD_LENGTH} characters",
        )
    if not auth.create_user(username, password):
        raise HTTPException(409, detail=f"username '{username}' is taken")
    token = auth.create_token(username)
    return LoginResponse(token=token, username=username)


@app.delete("/auth/token")
def revoke(
    credentials: HTTPAuthorizationCredentials = Depends(security),
    username: str = Depends(require_auth),
):
    """Revoke the presented bearer token (H-221) — logout that sticks."""
    auth.revoke_token(credentials.credentials)
    return {"ok": True, "detail": f"token revoked for {username}"}


# --- Package routes ---


def _version_records(rows: list[db.Version]) -> list[VersionRecord]:
    return [
        VersionRecord(
            version=r.version,
            checksum=r.checksum,
            published_at=r.published_at,
            yanked=r.yanked,
        )
        for r in rows
    ]


def _summary(pkg: db.Package) -> PackageSummary | None:
    latest = resolve_latest(_version_records(pkg.versions))
    if latest is None:
        return None
    row = next(v for v in pkg.versions if v.version == latest.version)
    return PackageSummary(
        owner=pkg.owner,
        name=pkg.name,
        latest_version=row.version,
        description=row.description,
        package_type=row.package_type,
        language=row.language,
        license=row.license,
        tags=row.tags,
        published_at=row.published_at.isoformat(),
    )


@app.post(
    "/packages",
    response_model=PublishResponse,
    status_code=201,
    dependencies=[Depends(rate_limit_publish)],
)
async def publish(
    archive: UploadFile = File(...),
    lockfile: UploadFile = File(...),
    owner: str = Form(...),
    name: str = Form(...),
    username: str = Depends(require_auth),
):
    """Publish a new package version (§14.8).

    Validates the uploaded archive's checksum against the accompanying
    xelian.lock, enforces immutability (§19.2) via the DB unique constraint,
    and stores metadata in Postgres + archive bytes in object/disk storage.
    """
    if owner != username:
        raise HTTPException(
            403,
            detail=f"User '{username}' cannot publish to namespace '{owner}'",
        )

    _validate_segment(owner, "owner")
    _validate_segment(name, "name")

    archive_bytes = await _read_capped(archive, MAX_ARCHIVE_BYTES, "archive")
    lock_bytes = await _read_capped(lockfile, MAX_LOCKFILE_BYTES, "lockfile")

    # --- checksum verification (§14.5, §7.3) ---
    import tomllib

    try:
        lock_data = tomllib.loads(lock_bytes.decode("utf-8"))
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as e:
        raise HTTPException(400, detail=f"malformed xelian.lock: {e}")
    declared_checksum = lock_data.get("package-checksum")
    if not declared_checksum:
        raise HTTPException(400, detail="xelian.lock missing package-checksum")
    actual_checksum = compute_package_checksum(archive_bytes)
    if actual_checksum != declared_checksum:
        raise HTTPException(
            422,
            detail=(
                f"checksum mismatch: declared={declared_checksum}, "
                f"actual={actual_checksum}"
            ),
        )

    manifest_dict, readme = _extract_xelian_toml(archive_bytes)
    version = manifest_dict.get("version")
    if not version:
        raise HTTPException(400, detail="xelian.toml missing version")
    _validate_segment(version, "version")

    author_data = manifest_dict.get("author", {})
    deps_data = manifest_dict.get("dependencies", {})

    # --- immutability (§19.2): the DB unique constraint is the arbiter, so
    # concurrent duplicate publishes yield exactly one 201 (H-214) ---
    with db.session() as s:
        pkg = s.scalar(
            select(db.Package).where(
                db.Package.owner == owner, db.Package.name == name
            )
        )
        if pkg is None:
            pkg = db.Package(owner=owner, name=name)
            s.add(pkg)
            try:
                s.flush()
            except IntegrityError:
                s.rollback()
                pkg = s.scalar(
                    select(db.Package).where(
                        db.Package.owner == owner, db.Package.name == name
                    )
                )
        row = db.Version(
            package_id=pkg.id,
            version=version,
            description=manifest_dict.get("description", ""),
            package_type=manifest_dict.get("package-type", ""),
            language=manifest_dict.get("language", ""),
            runtime=manifest_dict.get("runtime", ""),
            entrypoint=manifest_dict.get("entrypoint", ""),
            license=manifest_dict.get("license", ""),
            permissions=manifest_dict.get("permissions", []),
            features=manifest_dict.get("features", []),
            author={
                "name": author_data.get("name", ""),
                "email": author_data.get("email", ""),
                "homepage": author_data.get("homepage"),
            },
            dependencies_manifest=deps_data.get("manifest", ""),
            dependencies_lockfile=deps_data.get("lockfile"),
            os_list=manifest_dict.get("os"),
            homepage=manifest_dict.get("homepage"),
            repository=manifest_dict.get("repository"),
            primary_model=manifest_dict.get("primary-model"),
            tags=manifest_dict.get("tags"),
            checksum=hashlib.sha256(archive_bytes).hexdigest(),
            readme=readme,
        )
        s.add(row)
        try:
            s.commit()
        except IntegrityError:
            raise HTTPException(
                409,
                detail=f"version {version} of {owner}/{name} already published",
            )
        version_id = row.id

    # Metadata row exists (we won the race) — now persist the bytes. If that
    # fails, roll the row back so a retry can succeed.
    try:
        archive_storage.save(owner, name, version, archive_bytes, lock_bytes)
    except Exception as e:
        with db.session() as s:
            row = s.get(db.Version, version_id)
            if row is not None:
                s.delete(row)
                s.commit()
        raise HTTPException(500, detail=f"archive storage failed: {e}")

    return PublishResponse(ok=True, name=name, version=version)


@app.get("/packages", response_model=list[PackageSummary])
def list_packages():
    """Public read-only listing of every package's latest resolvable version.

    Serves the website's browse surface (§14.9); resolution follows §14.3.
    Packages with no resolvable version are omitted.
    """
    with db.session() as s:
        pkgs = s.scalars(
            select(db.Package)
            .options(selectinload(db.Package.versions))
            .order_by(db.Package.owner, db.Package.name)
        ).all()
        return [summary for pkg in pkgs if (summary := _summary(pkg))]


@app.get("/search", response_model=list[PackageSummary])
def search(q: str = ""):
    """Search packages by name/owner/description/tags (H-224, SQL ILIKE)."""
    q = q.strip()
    if not q:
        return []
    pattern = f"%{q}%"
    with db.session() as s:
        ids = s.scalars(
            select(db.Package.id)
            .join(db.Version)
            .where(
                db.Package.name.ilike(pattern)
                | db.Package.owner.ilike(pattern)
                | db.Version.description.ilike(pattern)
                | db.Version.tags.cast(db.Text).ilike(pattern)
            )
            .distinct()
        ).all()
        if not ids:
            return []
        pkgs = s.scalars(
            select(db.Package)
            .where(db.Package.id.in_(ids))
            .options(selectinload(db.Package.versions))
            .order_by(db.Package.owner, db.Package.name)
        ).all()
        return [summary for pkg in pkgs if (summary := _summary(pkg))]


@app.get("/packages/{owner}/{package_name}")
def get_package(owner: str, package_name: str):
    """Fetch package metadata for the latest stable version (§14.8).

    Resolution follows §14.3: highest SemVer that is not yanked and not
    a pre-release.
    """
    _validate_segment(owner, "owner")
    _validate_segment(package_name, "package")
    with db.session() as s:
        pkg = s.scalar(
            select(db.Package)
            .where(db.Package.owner == owner, db.Package.name == package_name)
            .options(selectinload(db.Package.versions))
        )
        if pkg is None or not pkg.versions:
            raise HTTPException(
                404,
                detail=f"package {owner}/{package_name} not found",
            )

        records = _version_records(pkg.versions)
        latest = resolve_latest(records)
        if latest is None:
            raise HTTPException(
                404,
                detail=(
                    f"no resolvable (non-yanked, non-pre-release) version "
                    f"of {owner}/{package_name}"
                ),
            )
        row = next(v for v in pkg.versions if v.version == latest.version)

        return PackageInfo(
            owner=owner,
            name=package_name,
            latest_version=row.version,
            description=row.description,
            package_type=row.package_type,
            language=row.language,
            runtime=row.runtime,
            entrypoint=row.entrypoint,
            license=row.license,
            permissions=row.permissions or [],
            features=row.features or [],
            author=AuthorInfo(**(row.author or {"name": "", "email": ""})),
            readme=row.readme,
            versions=records,
        )


@app.get("/download/{owner}/{package_name}/{version}")
def download(owner: str, package_name: str, version: str):
    """Download a specific version's archive, streamed in chunks (H-222)."""
    _validate_segment(owner, "owner")
    _validate_segment(package_name, "package")
    _validate_segment(version, "version")
    result = archive_storage.stream(owner, package_name, version)
    if result is None:
        raise HTTPException(
            404,
            detail=f"version {version} of {owner}/{package_name} not found",
        )
    chunks, size = result
    return StreamingResponse(
        chunks,
        media_type="application/octet-stream",
        headers={"Content-Length": str(size)},
    )


# --- Yank route (§14.7) ---


class YankRequest(BaseModel):
    yanked: bool = True


@app.patch("/packages/{owner}/{package_name}/{version}")
def yank_version(
    owner: str,
    package_name: str,
    version: str,
    body: YankRequest,
    username: str = Depends(require_auth),
):
    """Mark a version as yanked or unyanked (§14.7.1).

    Requires authentication as the package owner (§14.4). Never deletes the
    archive, checksum, or metadata (§14.7.1).
    """
    if owner != username:
        raise HTTPException(
            403,
            detail=f"User '{username}' cannot yank packages in namespace '{owner}'",
        )

    _validate_segment(owner, "owner")
    _validate_segment(package_name, "package")
    _validate_segment(version, "version")

    with db.session() as s:
        row = s.scalar(
            select(db.Version)
            .join(db.Package)
            .where(
                db.Package.owner == owner,
                db.Package.name == package_name,
                db.Version.version == version,
            )
        )
        if row is None:
            raise HTTPException(
                404,
                detail=f"version {version} of {owner}/{package_name} not found",
            )
        row.yanked = body.yanked
        s.commit()

    action = "yanked" if body.yanked else "unyanked"
    return {"ok": True, "detail": f"version {version} {action}"}
