import os
import shutil
import tempfile
from pathlib import Path

# Must be set before app.main is imported: the engine URL is read lazily but
# the rate limiters are built at import time.
os.environ.setdefault(
    "DATABASE_URL",
    "postgresql+psycopg://postgres:postgres@localhost:5433/xelian_test",
)
os.environ.setdefault("XELIAN_RATE_LIMIT_AUTH", "1000000")
os.environ.setdefault("XELIAN_RATE_LIMIT_PUBLISH", "1000000")

import pytest

import app.main
from app import db
from app.storage import DiskStorage


@pytest.fixture(scope="session", autouse=True)
def _db_schema():
    """Create the schema once. Postgres is required — no SQLite fallback
    (decision of record). CI provides a service container; locally:

        docker run -d --name xelian-postgres -e POSTGRES_PASSWORD=postgres \
          -e POSTGRES_DB=xelian -p 5433:5432 postgres:16-alpine
        docker exec xelian-postgres psql -U postgres -c "CREATE DATABASE xelian_test;"
    """
    try:
        db.create_all()
    except Exception as e:
        pytest.exit(f"Postgres is required for the registry tests: {e}", returncode=3)
    yield


@pytest.fixture(autouse=True)
def test_env(monkeypatch, _db_schema):
    """Empty tables, temp archive storage, and bootstrap credentials."""
    with db.session() as s:
        for table in reversed(db.Base.metadata.sorted_tables):
            s.execute(table.delete())
        s.commit()
    tmp = tempfile.mkdtemp()
    monkeypatch.setattr(app.main, "archive_storage", DiskStorage(Path(tmp)))
    monkeypatch.setenv("XELIAN_REGISTRY_USERNAME", "testuser")
    monkeypatch.setenv("XELIAN_REGISTRY_PASSWORD", "testpass")
    yield
    shutil.rmtree(tmp, ignore_errors=True)
