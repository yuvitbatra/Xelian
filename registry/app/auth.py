"""Accounts and tokens (H-221), persisted in Postgres.

- Passwords: salted scrypt hashes (stdlib KDF of the same strength class as
  bcrypt/argon2 interactive parameters; parameters are encoded per-hash so
  they can be raised without invalidating accounts).
- Tokens: stored as SHA-256 digests with expiry and revocation — they
  survive restarts/redeploys and a leaked table leaks no usable bearer token.
- There are no server credentials and no default account: identities come
  from signup only. (An explicit XELIAN_REGISTRY_USERNAME/PASSWORD pair is
  honored as a bootstrap account for tests/ops, but only when BOTH are set.)

SPEC.md §14.4: registry-mutating operations require authentication.
"""

import hashlib
import hmac
import os
import re
import secrets
import time
from typing import Optional

from sqlalchemy import select

from . import db

TOKEN_EXPIRY_SECONDS = 86400 * 30  # 30 days

# Username doubles as the publish namespace (§14.4) and as a §19.3-safe path
# segment.
_SAFE_USERNAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{1,38}$")

MIN_PASSWORD_LENGTH = 8

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


def _digest(token: str) -> str:
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def user_exists(username: str) -> bool:
    with db.session() as s:
        return (
            s.scalar(select(db.User).where(db.User.username == username))
            is not None
        )


def create_user(username: str, password: str) -> bool:
    """Create an account. Returns False if the username is taken."""
    from sqlalchemy.exc import IntegrityError

    with db.session() as s:
        user = db.User(username=username, password_hash=hash_password(password))
        s.add(user)
        try:
            s.commit()
        except IntegrityError:
            return False
    return True


def authenticate(username: str, password: str) -> bool:
    env_user = os.environ.get("XELIAN_REGISTRY_USERNAME")
    env_pass = os.environ.get("XELIAN_REGISTRY_PASSWORD")
    if env_user and env_pass:
        if hmac.compare_digest(username, env_user) and hmac.compare_digest(
            password, env_pass
        ):
            return True
    with db.session() as s:
        user = s.scalar(select(db.User).where(db.User.username == username))
    if user is None:
        # Burn equivalent work so response timing does not reveal whether the
        # username exists.
        verify_password(hash_password("timing-equalizer"), password)
        return False
    return verify_password(user.password_hash, password)


def create_token(username: str) -> str:
    token = secrets.token_hex(32)
    with db.session() as s:
        s.add(
            db.Token(
                token_digest=_digest(token),
                username=username,
                expires_at=time.time() + TOKEN_EXPIRY_SECONDS,
            )
        )
        s.commit()
    return token


def verify_token(token: str) -> Optional[str]:
    if token.startswith("Bearer "):
        token = token[7:]
    if not token:
        return None
    with db.session() as s:
        row = s.scalar(
            select(db.Token).where(db.Token.token_digest == _digest(token))
        )
        if row is None or row.revoked:
            return None
        if row.expires_at < time.time():
            s.delete(row)
            s.commit()
            return None
        return row.username


def revoke_token(token: str) -> bool:
    """Revoke a bearer token (H-221). Returns True if it existed."""
    if token.startswith("Bearer "):
        token = token[7:]
    with db.session() as s:
        row = s.scalar(
            select(db.Token).where(db.Token.token_digest == _digest(token))
        )
        if row is None:
            return False
        row.revoked = True
        s.commit()
        return True
