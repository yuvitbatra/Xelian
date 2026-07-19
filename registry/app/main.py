import hashlib
import io
import json
import tarfile
from datetime import datetime, timezone
from pathlib import Path

from fastapi import Depends, FastAPI, File, Form, HTTPException, UploadFile
from pydantic import BaseModel
from fastapi.responses import Response
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer

from .auth import AuthStore, MIN_PASSWORD_LENGTH, valid_username
from .models import (
    AuthorInfo,
    LoginResponse,
    PackageInfo,
    PackageMetadata,
    PackageSummary,
    PublishResponse,
    VersionRecord,
)
from .resolution import resolve_latest
from .storage import Storage

app = FastAPI(title="Xelian Registry", version="0.1.0")

import os

# The website is a browser client of this same public API (§14.9). Bearer
# tokens (no cookies) make a permissive CORS policy safe here.
from fastapi.middleware.cors import CORSMiddleware

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

STORAGE_ROOT = Path(
    os.environ.get(
        "XELIAN_REGISTRY_ROOT",
        Path.home() / ".xelian-registry",
    )
)
storage = Storage(STORAGE_ROOT)
auth_store = AuthStore(STORAGE_ROOT / "auth")

security = HTTPBearer(auto_error=False)


def require_auth(credentials: HTTPAuthorizationCredentials = Depends(security)):
    """Require a valid auth token. Returns the authenticated username."""
    if credentials is None:
        raise HTTPException(401, detail="Authentication required")
    username = auth_store.verify_token(credentials.credentials)
    if username is None:
        raise HTTPException(401, detail="Invalid or expired token")
    return username


# A package/owner/version path segment must be a single safe filesystem
# component: no separators, no `.`/`..` traversal, no empty value. This mirrors
# the CLI's `is_safe_repo_component` (github.rs) and the §19.3 name charset, and
# is the registry's guard against writing/reading outside its storage root.
_SAFE_SEGMENT = __import__("re").compile(r"^[A-Za-z0-9._+-]+$")


def _validate_segment(value: str, field: str) -> str:
    """Reject a path segment that could escape the storage root (§19.3)."""
    if not value or value in (".", "..") or not _SAFE_SEGMENT.match(value):
        raise HTTPException(400, detail=f"invalid {field}: {value!r}")
    return value


# --- Upload / decompression limits (H-214) ---
# Caps are env-tunable so deployments can raise them without a code change.
MAX_ARCHIVE_BYTES = int(os.environ.get("XELIAN_MAX_ARCHIVE_MB", "100")) * 1024 * 1024
MAX_LOCKFILE_BYTES = 1 * 1024 * 1024
MAX_UNCOMPRESSED_BYTES = int(os.environ.get("XELIAN_MAX_UNCOMPRESSED_MB", "500")) * 1024 * 1024
MAX_ARCHIVE_ENTRIES = 10_000


async def _read_capped(upload: UploadFile, cap: int, field: str) -> bytes:
    """Read an upload, rejecting it with 413 as soon as it exceeds `cap`.

    Chunked so an oversized body is rejected without buffering it whole.
    """
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
                # extra bytes: extractfile respects the header, and total is
                # re-checked against what was actually read.
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
            "package_metadata": "GET /packages/{owner}/{name}",
            "download": "GET /download/{owner}/{name}/{version}",
            "signup": "POST /auth/signup",
            "login": "POST /auth/token",
            "publish": "POST /packages (auth required)",
        },
    }


@app.get("/health")
def health():
    return {"ok": True}


# --- Auth routes (§13.7, §14.4, TODO-15) ---


@app.post("/auth/token")
def login(body: dict):
    """Exchange credentials for an auth token."""
    username = body.get("username", "")
    password = body.get("password", "")
    if not auth_store.authenticate(username, password):
        raise HTTPException(401, detail="Invalid credentials")
    token = auth_store.create_token(username)
    return LoginResponse(token=token, username=username)


@app.post("/auth/signup", response_model=LoginResponse, status_code=201)
def signup(body: dict):
    """Create an account; the username becomes the publish namespace (§14.4).

    Returns a fresh token so signup doubles as the first login.
    """
    username = body.get("username", "")
    password = body.get("password", "")
    if not valid_username(username):
        raise HTTPException(
            400,
            detail=(
                "invalid username: 2-39 chars, letters/digits/._- only, "
                "must start with a letter or digit"
            ),
        )
    if len(password) < MIN_PASSWORD_LENGTH:
        raise HTTPException(
            400,
            detail=f"password must be at least {MIN_PASSWORD_LENGTH} characters",
        )
    if not auth_store.create_user(username, password):
        raise HTTPException(409, detail=f"username '{username}' is taken")
    token = auth_store.create_token(username)
    return LoginResponse(token=token, username=username)


# --- Package routes ---


@app.post("/packages", response_model=PublishResponse, status_code=201)
async def publish(
    archive: UploadFile = File(...),
    lockfile: UploadFile = File(...),
    owner: str = Form(...),
    name: str = Form(...),
    username: str = Depends(require_auth),
):
    """Publish a new package version (§14.8).

    Validates the uploaded archive's checksum against the accompanying
    xelian.lock, enforces immutability (§19.2), and stores the package.

    Requires authentication (§14.4). The authenticated user must match
    the package's owner namespace (§14.4).
    """
    # --- authorization: owner must match authenticated user (§14.4) ---
    if owner != username:
        raise HTTPException(
            403,
            detail=f"User '{username}' cannot publish to namespace '{owner}'",
        )

    # --- reject traversal in namespace/name before any filesystem use (§19.3) ---
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

    # --- extract metadata from archive ---
    manifest_dict, readme = _extract_xelian_toml(archive_bytes)
    version = manifest_dict.get("version")
    if not version:
        raise HTTPException(400, detail="xelian.toml missing version")
    _validate_segment(version, "version")

    # --- immutability check (§19.2): the atomic mkdir is the arbiter, so
    # concurrent duplicate publishes yield exactly one 201 (H-214) ---
    if not storage.reserve_version(owner, name, version):
        raise HTTPException(
            409,
            detail=f"version {version} of {owner}/{name} already published",
        )

    # --- store everything ---
    checksum = storage.save_archive(owner, name, version, archive_bytes)
    storage.save_lockfile(owner, name, version, lock_bytes)

    author_data = manifest_dict.get("author", {})
    deps_data = manifest_dict.get("dependencies", {})
    now_iso = datetime.now(timezone.utc).isoformat()
    meta = PackageMetadata(
        spec_version=manifest_dict.get("spec-version", 1),
        name=name,
        version=version,
        description=manifest_dict.get("description", ""),
        package_type=manifest_dict.get("package-type", ""),
        language=manifest_dict.get("language", ""),
        runtime=manifest_dict.get("runtime", ""),
        entrypoint=manifest_dict.get("entrypoint", ""),
        license=manifest_dict.get("license", ""),
        permissions=manifest_dict.get("permissions", []),
        features=manifest_dict.get("features", []),
        author=AuthorInfo(
            name=author_data.get("name", ""),
            email=author_data.get("email", ""),
            homepage=author_data.get("homepage"),
        ),
        dependencies_manifest=deps_data.get("manifest", ""),
        dependencies_lockfile=deps_data.get("lockfile"),
        os=manifest_dict.get("os"),
        homepage=manifest_dict.get("homepage"),
        repository=manifest_dict.get("repository"),
        primary_model=manifest_dict.get("primary-model"),
        tags=manifest_dict.get("tags"),
        checksum=checksum,
        published_at=now_iso,
        yanked=False,
    )
    storage.save_metadata(owner, name, version, meta)
    storage.save_readme(owner, name, version, readme)

    return PublishResponse(ok=True, name=name, version=version)


@app.get("/packages", response_model=list[PackageSummary])
def list_packages():
    """Public read-only listing of every package's latest resolvable version.

    Serves the website's browse/search surface (§14.9); resolution follows
    §14.3 (highest non-yanked, non-pre-release SemVer). Packages with no
    resolvable version are omitted.
    """
    summaries = []
    for owner, name in storage.list_package_names():
        versions = storage.list_versions(owner, name)
        latest = resolve_latest(versions)
        if latest is None:
            continue
        meta = storage.load_metadata(owner, name, latest.version)
        if meta is None:
            continue
        summaries.append(
            PackageSummary(
                owner=owner,
                name=name,
                latest_version=latest.version,
                description=meta.description,
                package_type=meta.package_type,
                language=meta.language,
                license=meta.license,
                tags=meta.tags,
                published_at=meta.published_at,
            )
        )
    return summaries


@app.get("/packages/{owner}/{package_name}")
def get_package(owner: str, package_name: str):
    """Fetch package metadata for the latest stable version (§14.8).

    Resolution follows §14.3: highest SemVer that is not yanked and not
    a pre-release.
    """
    _validate_segment(owner, "owner")
    _validate_segment(package_name, "package")
    versions = storage.list_versions(owner, package_name)
    if not versions:
        raise HTTPException(
            404,
            detail=f"package {owner}/{package_name} not found",
        )

    latest = resolve_latest(versions)
    if latest is None:
        raise HTTPException(
            404,
            detail=(
                f"no resolvable (non-yanked, non-pre-release) version "
                f"of {owner}/{package_name}"
            ),
        )

    meta = storage.load_metadata(owner, package_name, latest.version)
    readme = storage.load_readme(owner, package_name, latest.version)

    return PackageInfo(
        owner=owner,
        name=package_name,
        latest_version=latest.version,
        description=meta.description if meta else "",
        package_type=meta.package_type if meta else "",
        language=meta.language if meta else "",
        runtime=meta.runtime if meta else "",
        entrypoint=meta.entrypoint if meta else "",
        license=meta.license if meta else "",
        permissions=meta.permissions if meta else [],
        features=meta.features if meta else [],
        author=meta.author if meta else AuthorInfo(name="", email=""),
        readme=readme or "",
        versions=versions,
    )


@app.get("/download/{owner}/{package_name}/{version}")
def download(owner: str, package_name: str, version: str):
    """Download a specific version's archive (§14.8)."""
    _validate_segment(owner, "owner")
    _validate_segment(package_name, "package")
    _validate_segment(version, "version")
    data = storage.archive_bytes(owner, package_name, version)
    if data is None:
        raise HTTPException(
            404,
            detail=f"version {version} of {owner}/{package_name} not found",
        )
    return Response(content=data, media_type="application/octet-stream")


# --- Yank route (Phase 17, H-170 / SPEC.md §14.7, TODO-15) ---


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

    Requires authentication as the package owner (§14.4). Never
    deletes the archive, checksum, or metadata (§14.7.1).
    """
    if owner != username:
        raise HTTPException(
            403,
            detail=f"User '{username}' cannot yank packages in namespace '{owner}'",
        )

    _validate_segment(owner, "owner")
    _validate_segment(package_name, "package")
    _validate_segment(version, "version")

    version_exists = storage.version_exists(owner, package_name, version)
    if not version_exists:
        raise HTTPException(
            404,
            detail=f"version {version} of {owner}/{package_name} not found",
        )

    ok = storage.set_yanked(owner, package_name, version, body.yanked)
    if not ok:
        raise HTTPException(404, detail=f"version {version} metadata not found")

    action = "yanked" if body.yanked else "unyanked"
    return {"ok": True, "detail": f"version {version} {action}"}
