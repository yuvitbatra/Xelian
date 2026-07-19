import shutil
import tempfile
from pathlib import Path

import pytest

import app.main
from app.auth import AuthStore
from app.storage import Storage


@pytest.fixture(autouse=True)
def test_env(monkeypatch):
    """Use a temp directory for storage and fixed credentials during tests."""
    tmp = tempfile.mkdtemp()
    new_storage = Storage(Path(tmp))
    monkeypatch.setattr(app.main, "storage", new_storage)
    monkeypatch.setattr(app.main, "auth_store", AuthStore(Path(tmp) / "auth"))
    monkeypatch.setenv("XELIAN_REGISTRY_USERNAME", "testuser")
    monkeypatch.setenv("XELIAN_REGISTRY_PASSWORD", "testpass")
    yield
    shutil.rmtree(tmp)
