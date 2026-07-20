import os

import uvicorn

if __name__ == "__main__":
    # Honor a platform-provided $PORT (Render/Fly/Cloud Run); default to 8000.
    uvicorn.run("app.main:app", host="0.0.0.0", port=int(os.environ.get("PORT", "8000")))
