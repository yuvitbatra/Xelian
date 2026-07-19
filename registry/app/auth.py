import hashlib
import hmac
import json
import os
import re
import secrets
import time
from pathlib import Path
from typing import Optional

TOKEN_EXPIRY_SECONDS = 86400 * 30  # 30 days

# Username doubles as the publish namespace (§14.4) and as a storage path
# segment, so it must satisfy the §19.3 safe-segment charset. Length is
# additionally bounded to keep namespaces readable.
_SAFE_USERNAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{1,38}$")

MIN_PASSWORD_LENGTH = 8

# scrypt parameters (N, r, p) — interactive-login strength; encoded into each
# hash so they can be raised later without invalidating existing accounts.
_SCRYPT_N = 2**14
_SCRYPT_R = 8
_SCRYPT_P = 1


def _scrypt(password: str, salt: bytes, n: int, r: int, p: int) -> bytes:
    return hashlib.scrypt(
        password.encode("utf-8"), salt=salt, n=n, r=r, p=p, dklen=32
    )


def hash_password(password: str) -> str:
    salt = secrets.token_bytes(16)
    digest = _scrypt(password, salt, _SCRYPT_N, _SCRYPT_R, _SCRYPT_P)
    return "$".join(
        [
            "scrypt",
            str(_SCRYPT_N),
            str(_SCRYPT_R),
            str(_SCRYPT_P),
            salt.hex(),
            digest.hex(),
        ]
    )


def verify_password(stored: str, password: str) -> bool:
    try:
        scheme, n, r, p, salt_hex, digest_hex = stored.split("$")
        if scheme != "scrypt":
            return False
        expected = bytes.fromhex(digest_hex)
        actual = _scrypt(password, bytes.fromhex(salt_hex), int(n), int(r), int(p))
    except (ValueError, TypeError):
        return False
    return hmac.compare_digest(expected, actual)


def valid_username(username: str) -> bool:
    return bool(_SAFE_USERNAME.match(username))


class AuthStore:
    """Disk-backed accounts and tokens for the Xelian registry.

    - Passwords are stored as salted scrypt hashes (stdlib — no extra deps).
    - Tokens are stored as SHA-256 digests, so a leaked store never leaks a
      usable bearer token, and they survive registry restarts.
    - Optional bootstrap account via XELIAN_REGISTRY_USERNAME/PASSWORD, honored
      only when BOTH are explicitly set (there is no default admin/admin).

    SPEC.md §14.4: registry-mutating operations require authentication.
    """

    def __init__(self, root: Path):
        self.root = Path(root)

    @property
    def _users_file(self) -> Path:
        return self.root / "users.json"

    @property
    def _tokens_file(self) -> Path:
        return self.root / "tokens.json"

    def _load(self, path: Path) -> dict:
        try:
            return json.loads(path.read_text())
        except (OSError, json.JSONDecodeError):
            return {}

    def _save(self, path: Path, data: dict) -> None:
        self.root.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, indent=2))
        tmp.replace(path)

    # --- accounts ---

    def user_exists(self, username: str) -> bool:
        return username in self._load(self._users_file)

    def create_user(self, username: str, password: str) -> bool:
        """Create an account. Returns False if the username is taken."""
        users = self._load(self._users_file)
        if username in users:
            return False
        users[username] = {
            "password_hash": hash_password(password),
            "created_at": time.time(),
        }
        self._save(self._users_file, users)
        return True

    def authenticate(self, username: str, password: str) -> bool:
        env_user = os.environ.get("XELIAN_REGISTRY_USERNAME")
        env_pass = os.environ.get("XELIAN_REGISTRY_PASSWORD")
        if env_user and env_pass:
            if hmac.compare_digest(username, env_user) and hmac.compare_digest(
                password, env_pass
            ):
                return True
        record = self._load(self._users_file).get(username)
        if record is None:
            # Burn the same work as a real check so response timing does not
            # reveal whether the username exists.
            verify_password(hash_password("timing-equalizer"), password)
            return False
        return verify_password(record.get("password_hash", ""), password)

    # --- tokens ---

    def create_token(self, username: str) -> str:
        token = secrets.token_hex(32)
        digest = hashlib.sha256(token.encode("utf-8")).hexdigest()
        tokens = self._load(self._tokens_file)
        now = time.time()
        # Lazy purge of expired tokens keeps the file bounded.
        tokens = {k: v for k, v in tokens.items() if v.get("expires_at", 0) > now}
        tokens[digest] = {
            "username": username,
            "expires_at": now + TOKEN_EXPIRY_SECONDS,
        }
        self._save(self._tokens_file, tokens)
        return token

    def verify_token(self, token: str) -> Optional[str]:
        if token.startswith("Bearer "):
            token = token[7:]
        if not token:
            return None
        digest = hashlib.sha256(token.encode("utf-8")).hexdigest()
        tokens = self._load(self._tokens_file)
        data = tokens.get(digest)
        if data is None:
            return None
        if data.get("expires_at", 0) < time.time():
            tokens.pop(digest, None)
            self._save(self._tokens_file, tokens)
            return None
        return data["username"]
