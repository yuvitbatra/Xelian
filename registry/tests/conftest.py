import os
import shutil
import tempfile
from pathlib import Path

# The test suite TRUNCATES every table before each test (see `test_env`). It
# must therefore never point at a database that isn't a throwaway test DB — an
# operator with DATABASE_URL exported to production who runs `pytest` would
# otherwise wipe it. Two guards enforce this:
#
#   1. The dedicated `XELIAN_TEST_DATABASE_URL` wins over the ambient
#      `DATABASE_URL`, so a production `DATABASE_URL` in the shell is ignored
#      rather than silently used.
#   2. Whichever URL is resolved, its database name MUST look like a test DB
#      (contain "test"), or the suite refuses to start.
#
# This makes accidental destruction of a real database impossible, not merely
# unlikely.
_DEFAULT_TEST_DB = "postgresql+psycopg://postgres:postgres@localhost:5433/xelian_test"
_test_db_url = os.environ.get("XELIAN_TEST_DATABASE_URL") or os.environ.get(
    "DATABASE_URL", _DEFAULT_TEST_DB
)


def _database_name(url: str) -> str:
    """The database name (last path segment, minus any query string)."""
    return url.rsplit("/", 1)[-1].split("?", 1)[0]


_db_name = _database_name(_test_db_url)
if "test" not in _db_name.lower():
    raise RuntimeError(
        f"refusing to run the registry test suite against database {_db_name!r}: "
        "its name does not contain 'test', so it may be a real database and the "
        "suite truncates every table. Set XELIAN_TEST_DATABASE_URL to a throwaway "
        "database whose name contains 'test' (e.g. .../xelian_test)."
    )

# Force the app to use the resolved (verified-safe) test URL, overriding any
# ambient DATABASE_URL. Must happen before app.main is imported: the engine URL
# is read lazily but the rate limiters are built at import time.
os.environ["DATABASE_URL"] = _test_db_url
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
