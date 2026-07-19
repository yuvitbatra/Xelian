import os
import secrets
import time
from typing import Optional

TOKEN_EXPIRY_SECONDS = 86400 * 30  # 30 days


class AuthStore:
    """Simple token-based auth store for the Harbor registry.

    Accounts are configured via environment variables (V1 simplicity).
    Tokens are generated on login and stored in-memory.

    SPEC.md §14.4: registry-mutating operations require authentication.
    SPEC.md TODO-15: auth routes are open; this implements a token/callback
    pair as sketched in the spec.
    """

    def __init__(self):
        self._tokens: dict[str, dict] = {}

    def authenticate(self, username: str, password: str) -> bool:
        admin_username = os.environ.get("HARBOR_REGISTRY_USERNAME", "admin")
        admin_password = os.environ.get("HARBOR_REGISTRY_PASSWORD", "admin")
        return secrets.compare_digest(username, admin_username) and \
               secrets.compare_digest(password, admin_password)

    def create_token(self, username: str) -> str:
        token = secrets.token_hex(32)
        expires_at = time.time() + TOKEN_EXPIRY_SECONDS
        self._tokens[token] = {"username": username, "expires_at": expires_at}
        return token

    def verify_token(self, token: str) -> Optional[str]:
        if token.startswith("Bearer "):
            token = token[7:]
        if not token:
            return None
        data = self._tokens.get(token)
        if data is None:
            return None
        if data.get("expires_at", 0) < time.time():
            self._tokens.pop(token, None)
            return None
        return data["username"]


auth_store = AuthStore()
