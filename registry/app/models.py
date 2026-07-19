from pydantic import BaseModel
from datetime import datetime
from typing import Optional


class AuthorInfo(BaseModel):
    name: str
    email: str
    homepage: Optional[str] = None


class PackageMetadata(BaseModel):
    spec_version: int
    name: str
    version: str
    description: str
    package_type: str
    language: str
    runtime: str
    entrypoint: str
    license: str
    permissions: list[str]
    features: list[str]
    author: AuthorInfo
    dependencies_manifest: str
    dependencies_lockfile: Optional[str] = None
    os: Optional[list[str]] = None
    homepage: Optional[str] = None
    repository: Optional[str] = None
    primary_model: Optional[str] = None
    tags: Optional[list[str]] = None
    checksum: str = ""
    published_at: str = ""
    yanked: bool = False


class VersionRecord(BaseModel):
    version: str
    checksum: str
    published_at: datetime
    yanked: bool = False


class PublishResponse(BaseModel):
    ok: bool
    name: str
    version: str


class LoginResponse(BaseModel):
    token: str
    username: str


class PackageSummary(BaseModel):
    """One row in the public package listing (website browse/search, §14.9)."""

    owner: str
    name: str
    latest_version: str
    description: str
    package_type: str
    language: str
    license: str
    tags: Optional[list[str]] = None
    published_at: str = ""


class PackageInfo(BaseModel):
    owner: str
    name: str
    latest_version: Optional[str]
    description: str
    package_type: str
    language: str
    runtime: str
    entrypoint: str
    license: str
    permissions: list[str]
    features: list[str]
    author: AuthorInfo
    readme: str
    versions: list[VersionRecord]
