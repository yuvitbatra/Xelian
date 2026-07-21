# Deploy the Xelian registry on Render — step by step

Written to be followed literally, click by click. No prior Render experience
assumed. Total time: ~30–45 minutes. Cost: $0 (free tiers throughout).

When you finish, `xelian push` and `xelian run you/package` will work over the
public internet, the same way `xelian add <github-url>` already does.

You need three free accounts. Create them first (links below), then follow the
steps in order.

- **Neon** (free Postgres database) — https://neon.tech
- **Cloudflare** (free R2 file storage) — you already have this; the R2 keys
  are in `registry/.env`.
- **Render** (free web hosting) — https://render.com

---

## Part 1 — Get your database (Neon) — ~10 min

Render's free database expires after 30 days, so we use Neon's free Postgres,
which doesn't.

1. Go to https://neon.tech and click **Sign up**. Sign in with GitHub (easiest).
2. It asks you to create a project. Name it `xelian`. Leave every other setting
   at its default. Click **Create project**.
3. On the next screen you'll see a box titled **Connection string** with a value
   starting `postgresql://`. Click the **copy** icon next to it.
4. Paste it into a scratch note for a moment. It looks like:
   ```
   postgresql://alex:AbC123@ep-cool-name-12345.us-east-2.aws.neon.tech/xelian?sslmode=require
   ```
5. **Change one word:** replace `postgresql://` at the very start with
   `postgresql+psycopg://`. So it becomes:
   ```
   postgresql+psycopg://alex:AbC123@ep-cool-name-12345.us-east-2.aws.neon.tech/xelian?sslmode=require
   ```
   (Xelian's registry uses the `psycopg` driver; this tells it which one.)
6. Keep this final string handy — it is your `DATABASE_URL`. Done with Neon.

---

## Part 2 — Find your R2 storage keys — ~2 min

You already have these. Open the file `registry/.env` in this project. You'll
see five lines you need (the values after the `=`):

```
DATABASE_URL=...              ← ignore this one, use the Neon one from Part 1
XELIAN_R2_BUCKET=...
XELIAN_R2_ENDPOINT=...
XELIAN_R2_ACCESS_KEY_ID=...
XELIAN_R2_SECRET_ACCESS_KEY=...
```

Keep this file open — you'll copy these four `XELIAN_R2_*` values in Part 3.

> These keys already work — verified against your real R2 bucket
> (save/download roundtrip passed). You don't need to change anything in
> Cloudflare.

---

## Part 3 — Deploy on Render — ~15 min

1. Go to https://render.com and click **Get Started** / **Sign up**. Sign in
   with GitHub.
2. If Render asks to install its GitHub app, allow it access to your
   `yuvitbatra/Xelian` repository (you can pick "only select repositories" and
   choose just that one).
3. On your Render dashboard, click the **New +** button (top right) →
   **Web Service**.
4. It shows a list of your GitHub repos. Find **Xelian** and click **Connect**.
5. Render now shows a settings form. Fill it in exactly like this:
   - **Name:** `xelian-registry` (this becomes part of your URL)
   - **Language / Runtime:** it should auto-detect **Docker**. If there's a
     dropdown, choose **Docker**.
   - **Branch:** `main`
   - **Root Directory:** type `registry`  ← **important**, the app lives in that
     subfolder.
   - **Instance Type / Plan:** choose **Free**.
   - Leave "Build Command" and "Start Command" **blank** — the Dockerfile
     handles both.
6. Scroll down to **Environment Variables** (sometimes under "Advanced"). Click
   **Add Environment Variable** and add these **five**, one at a time. For each,
   the **Key** is on the left, the **Value** is what you copy from your notes /
   `.env`:

   | Key | Value (from where) |
   |-----|--------------------|
   | `DATABASE_URL` | the Neon string from Part 1 (the `postgresql+psycopg://…` one) |
   | `XELIAN_R2_BUCKET` | from `registry/.env` |
   | `XELIAN_R2_ENDPOINT` | from `registry/.env` |
   | `XELIAN_R2_ACCESS_KEY_ID` | from `registry/.env` |
   | `XELIAN_R2_SECRET_ACCESS_KEY` | from `registry/.env` |

   Double-check there are no extra spaces before/after each value.
7. Click **Create Web Service** (or **Deploy Web Service**) at the bottom.
8. Render starts building. Watch the **Logs** tab. It takes ~3–5 minutes. You're
   waiting for a line like `Application startup complete` and the status badge
   (top of the page) to turn green and say **Live**.
9. At the top of the page Render shows your URL, like
   `https://xelian-registry.onrender.com`. **Copy it.**

### Check it worked

Open a new browser tab and go to `https://<your-url>/health`. You should see:
```json
{"ok":true}
```
Then try `https://<your-url>/catalog?limit=3` — you should see JSON with a few
packages. If both work, the registry is **live**. 🎉

---

## Part 4 — Point the CLI at your live registry — ~10 min

Right now the published `xelian` binary talks to `localhost`. Two things make
it (and everyone else) use your live registry.

### A. For yourself, right now (per-terminal)
```bash
export XELIAN_REGISTRY_URL=https://<your-url>.onrender.com
```
Now in that terminal, `xelian push` and `xelian run you/pkg` hit your registry.

### B. For everyone (bake it into the binaries) — do this once
So users don't have to set anything, rebuild the release with your URL baked in.
On this machine:
```bash
cd /Users/yuvitbatra/Desktop/School/summer/harbor

# tag a new release with the production URL compiled in
# (edit the release workflow to pass the env var, OR build+upload locally)
XELIAN_DEFAULT_REGISTRY_URL=https://<your-url>.onrender.com \
  cargo build --release -p xelian-cli
```
Then cut a new tag (e.g. `v0.1.1`) so the public binaries carry your URL:
```bash
git tag -a v0.1.1 -m "point CLI at production registry"
git push origin v0.1.1
```
(Ask me to wire `XELIAN_DEFAULT_REGISTRY_URL` into `release.yml` and I'll do it
so every future release is correct automatically.)

---

## Part 5 — Seed & smoke-test — ~10 min

Publish the 16 example packages to your live registry so it isn't empty:
```bash
export XELIAN_REGISTRY_URL=https://<your-url>.onrender.com
# create the official account (pick a strong password, save it in a manager)
curl -s -X POST https://<your-url>.onrender.com/auth/signup \
  -H 'content-type: application/json' \
  -d '{"username":"xelian","password":"CHOOSE-A-STRONG-PASSWORD"}'

echo "CHOOSE-A-STRONG-PASSWORD" | xelian login --username xelian --password-stdin
XELIAN_SEED_PASSWORD="CHOOSE-A-STRONG-PASSWORD" scripts/publish_seed.sh
```
Now the real test — run one from a clean machine (or just a fresh terminal):
```bash
echo '2*(3+4)**2' | xelian run xelian/calc     # should print 98
```
If that prints `98` fetched from your live registry, **you have shipped.**

---

## Keep it awake (optional but recommended)

Render's free tier sleeps a service after ~15 min of no traffic; the next
request then takes ~30s to wake. To avoid that, create a free
https://uptimerobot.com monitor that pings `https://<your-url>/health` every 5
minutes.

---

## If something goes wrong

- **Build fails / "no Dockerfile":** confirm **Root Directory** is `registry`.
- **`/health` works but `/catalog` is empty:** the image didn't include
  `catalog.json` — pull the latest `main` (the Dockerfile now copies it) and
  click **Manual Deploy → Deploy latest commit** on Render.
- **500 errors / "connection refused" in logs:** the `DATABASE_URL` is wrong.
  Re-copy it from Neon and make sure it starts with `postgresql+psycopg://`.
- **`xelian run` says "connection refused":** you didn't set
  `XELIAN_REGISTRY_URL` (Part 4A) in that terminal.
- **Anything else:** copy the last ~20 lines of Render's Logs tab and send them
  to me; I'll tell you the exact fix.
