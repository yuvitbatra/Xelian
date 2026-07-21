FROM python:3.12-slim

WORKDIR /app
COPY pyproject.toml uv.lock ./
RUN pip install --no-cache-dir uv && uv export --frozen --no-dev -o requirements.txt \
    && pip install --no-cache-dir -r requirements.txt

COPY app ./app
# The discovery index served by GET /catalog. Without it the Explore page is
# empty in production (the app looks for catalog.json next to /app).
COPY catalog.json ./catalog.json

# Required at runtime:
#   DATABASE_URL                  Neon Postgres URL (no SQLite fallback)
#   XELIAN_R2_BUCKET / XELIAN_R2_ENDPOINT /
#   XELIAN_R2_ACCESS_KEY_ID / XELIAN_R2_SECRET_ACCESS_KEY
#                                 R2 archive storage (REQUIRED in production —
#                                 free-tier disks are ephemeral)
# Bind to the platform-provided $PORT (Render/Fly/Cloud Run inject it); default
# to 8000 for plain `docker run`. Shell form so $PORT is expanded at runtime.
EXPOSE 8000
CMD uvicorn app.main:app --host 0.0.0.0 --port "${PORT:-8000}" --timeout-keep-alive 20
