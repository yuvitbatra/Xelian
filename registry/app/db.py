"""Postgres data layer (H-220): SQLAlchemy behind a single DATABASE_URL.

Decision of record (BACKLOG, 2026-07-18): Postgres is the one and only
database — no SQLite fallback anywhere. Local dev and CI run a real Postgres
(`docker run postgres`); production uses Neon's free tier. Archive bytes are
NEVER stored here — object/disk storage only (see storage.py); the database
holds metadata rows.
"""

import os
from datetime import datetime, timezone

from sqlalchemy import (
    Boolean,
    DateTime,
    Float,
    ForeignKey,
    String,
    Text,
    UniqueConstraint,
    create_engine,
)
from sqlalchemy.dialects.postgresql import JSONB
from sqlalchemy.orm import (
    DeclarativeBase,
    Mapped,
    Session,
    mapped_column,
    relationship,
)

_engine = None


def database_url() -> str:
    url = os.environ.get("DATABASE_URL")
    if not url:
        raise RuntimeError(
            "DATABASE_URL is not set. The registry requires Postgres (no "
            "SQLite fallback — decision of record). For local dev:\n"
            "  docker run -d --name xelian-postgres -e POSTGRES_PASSWORD=postgres "
            "-e POSTGRES_DB=xelian -p 5433:5432 postgres:16-alpine\n"
            "  export DATABASE_URL=postgresql+psycopg://postgres:postgres@localhost:5433/xelian"
        )
    # Accept plain postgres:// URLs (Neon/Render dashboards) and route them
    # through the psycopg3 driver.
    if url.startswith("postgres://"):
        url = "postgresql+psycopg://" + url[len("postgres://") :]
    elif url.startswith("postgresql://"):
        url = "postgresql+psycopg://" + url[len("postgresql://") :]
    return url


def get_engine():
    global _engine
    if _engine is None:
        _engine = create_engine(database_url(), pool_pre_ping=True)
    return _engine


def reset_engine():
    """Drop the cached engine (tests point DATABASE_URL somewhere else)."""
    global _engine
    if _engine is not None:
        _engine.dispose()
    _engine = None


def session() -> Session:
    return Session(get_engine())


def utcnow() -> datetime:
    return datetime.now(timezone.utc)


class Base(DeclarativeBase):
    pass


class User(Base):
    __tablename__ = "users"

    id: Mapped[int] = mapped_column(primary_key=True)
    username: Mapped[str] = mapped_column(String(64), unique=True, index=True)
    password_hash: Mapped[str] = mapped_column(String(256))
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=utcnow
    )


class Token(Base):
    __tablename__ = "tokens"

    id: Mapped[int] = mapped_column(primary_key=True)
    # SHA-256 digest of the bearer token — a leaked table leaks nothing usable.
    token_digest: Mapped[str] = mapped_column(String(64), unique=True, index=True)
    username: Mapped[str] = mapped_column(String(64), index=True)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=utcnow
    )
    expires_at: Mapped[float] = mapped_column(Float)  # unix epoch seconds
    revoked: Mapped[bool] = mapped_column(Boolean, default=False)


class Package(Base):
    __tablename__ = "packages"
    __table_args__ = (UniqueConstraint("owner", "name"),)

    id: Mapped[int] = mapped_column(primary_key=True)
    owner: Mapped[str] = mapped_column(String(64), index=True)
    name: Mapped[str] = mapped_column(String(64), index=True)

    versions: Mapped[list["Version"]] = relationship(
        back_populates="package", cascade="all, delete-orphan"
    )


class Version(Base):
    __tablename__ = "versions"
    # The unique constraint is the §19.2 immutability arbiter: concurrent
    # duplicate publishes race on this index and exactly one INSERT wins.
    __table_args__ = (UniqueConstraint("package_id", "version"),)

    id: Mapped[int] = mapped_column(primary_key=True)
    package_id: Mapped[int] = mapped_column(
        ForeignKey("packages.id", ondelete="CASCADE"), index=True
    )
    version: Mapped[str] = mapped_column(String(64))

    description: Mapped[str] = mapped_column(Text, default="")
    package_type: Mapped[str] = mapped_column(String(16), default="")
    language: Mapped[str] = mapped_column(String(16), default="")
    runtime: Mapped[str] = mapped_column(String(128), default="")
    entrypoint: Mapped[str] = mapped_column(String(256), default="")
    license: Mapped[str] = mapped_column(String(64), default="")
    permissions: Mapped[list] = mapped_column(JSONB, default=list)
    features: Mapped[list] = mapped_column(JSONB, default=list)
    author: Mapped[dict] = mapped_column(JSONB, default=dict)
    dependencies_manifest: Mapped[str] = mapped_column(String(256), default="")
    dependencies_lockfile: Mapped[str | None] = mapped_column(
        String(256), nullable=True
    )
    os_list: Mapped[list | None] = mapped_column(JSONB, nullable=True)
    homepage: Mapped[str | None] = mapped_column(String(512), nullable=True)
    repository: Mapped[str | None] = mapped_column(String(512), nullable=True)
    primary_model: Mapped[str | None] = mapped_column(String(128), nullable=True)
    tags: Mapped[list | None] = mapped_column(JSONB, nullable=True)
    checksum: Mapped[str] = mapped_column(String(128), default="")
    readme: Mapped[str] = mapped_column(Text, default="")
    published_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=utcnow
    )
    yanked: Mapped[bool] = mapped_column(Boolean, default=False)

    package: Mapped[Package] = relationship(back_populates="versions")


def create_all():
    Base.metadata.create_all(get_engine())
